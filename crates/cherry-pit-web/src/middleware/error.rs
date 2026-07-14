//! Error → HTTP response mapping.
//!
//! Realises **CHE-0049 R4 + R10**: translates
//! [`cherry_pit_core::DispatchError`], [`cherry_pit_core::StoreError`],
//! and [`cherry_pit_core::BusError`] into `(StatusCode, HeaderMap, ErrorBody)`
//! triples for `IntoResponse`.
//!
//! ## Mapping (canonical — drives the unit tests)
//!
//! | Source                                 | Status | Headers       | ADR         |
//! |-----------------------------------------|--------|---------------|-------------|
//! | `Rejected(E)`                           | 422    | —             | CHE-0015    |
//! | `ConcurrencyConflict` (dispatch/store)  | 409    | —             | CHE-0041:R3 |
//! | `AggregateNotFound`                     | 404    | —             | R10         |
//! | `Infrastructure` (dispatch/store)       | 503    | `Retry-After` | R10, below  |
//! | `StoreLocked`                           | 503    | `Retry-After` | R10         |
//! | `CorruptData`                           | 500    | —             | R10         |
//! | `BusError`                               | 503    | `Retry-After` | CHE-0021:R3 |
//! | post-persist cancellation                | 202    | —             | CHE-0046:R5 |
//!
//! `DispatchError::Infrastructure` sits at the dispatch layer, not the
//! store layer R10 enumerates, but maps to the same signal by the
//! same retryable reasoning; 500 stays reserved for terminal
//! `CorruptData`.
//!
//! `Rejected(E)`'s `Display` is preserved in full via
//! [`ErrorBody::message`] (CHE-0015): a `Serialize` bound on the
//! gateway error generic would tighten CHE-0049 R1.

use std::error::Error;
use std::fmt::Display;

use axum::http::{HeaderMap, HeaderValue, StatusCode, header::RETRY_AFTER};
use cherry_pit_core::{BusError, CorrelationContext, DispatchError, StoreError};
use serde::Serialize;

/// Conservative default `Retry-After` (seconds) for retryable
/// infrastructure failures. v0.1 uses a fixed value; adaptive backoff
/// hints are out of scope until the gateway exposes per-call signal.
const RETRY_AFTER_SECONDS: &str = "1";

/// Stable string codes for [`ErrorBody::code`]. These are part of the
/// HTTP response contract and must remain stable across patches.
mod code {
    pub(super) const REJECTED: &str = "rejected";
    pub(super) const CONCURRENCY_CONFLICT: &str = "concurrency_conflict";
    pub(super) const AGGREGATE_NOT_FOUND: &str = "aggregate_not_found";
    pub(super) const INFRASTRUCTURE: &str = "infrastructure";
    pub(super) const STORE_LOCKED: &str = "store_locked";
    pub(super) const CORRUPT_DATA: &str = "corrupt_data";
    pub(super) const BUS: &str = "bus";
    pub(super) const ACCEPTED_UNKNOWN: &str = "accepted_unknown";
}

/// JSON response body for cherry-pit-web error responses.
///
/// Carries a stable machine-readable [`code`](Self::code), a full
/// human-readable [`message`](Self::message) (the source error's
/// `Display`), and optional [`correlation_id`](Self::correlation_id).
///
/// Per CHE-0049 R4 + CHE-0015: stable error shape with lossless
/// `Display` propagation; `correlation_id` is elided when absent
/// (CHE-0039 R2 — never synthesise).
///
/// # Example
///
/// ```
/// use cherry_pit_web::errors::ErrorBody;
///
/// let body = ErrorBody {
///     code: "rejected",
///     message: "invariant violated".to_string(),
///     correlation_id: None,
/// };
/// let json = serde_json::to_string(&body).unwrap();
/// assert!(json.contains(r#""code":"rejected""#));
/// // correlation_id elided when None per CHE-0039 R2.
/// assert!(!json.contains("correlation_id"));
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    /// Stable, machine-readable error code, e.g. `"rejected"`,
    /// `"concurrency_conflict"`, `"aggregate_not_found"`.
    pub code: &'static str,

    /// Human-readable detail — the source error's `Display` output.
    /// For `DispatchError::Rejected(E)`, this preserves the full
    /// `Display` of the typed domain error `E` per CHE-0015.
    pub message: String,

    /// Correlation identifier echoed from the request (CHE-0049 R5).
    /// `None` until wired by S4; serialized only when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl ErrorBody {
    fn new(code: &'static str, message: impl Display) -> Self {
        Self {
            code,
            message: message.to_string(),
            correlation_id: None,
        }
    }

    /// Populate [`Self::correlation_id`] from a [`CorrelationContext`]
    /// (CHE-0049 R5).
    ///
    /// When `ctx.correlation_id()` is `Some`, the field is set to its
    /// 36-char hyphenated string form. When `None`, the field stays
    /// `None` and is elided from the serialized JSON via the
    /// `skip_serializing_if` attribute — preserving CHE-0039 R2 (no
    /// synthesised correlation).
    #[must_use]
    pub fn with_correlation(mut self, ctx: &CorrelationContext) -> Self {
        self.correlation_id = ctx.correlation_id().map(|id| id.to_string());
        self
    }
}

/// Triple returned by every mapping function: status + headers + body.
///
/// Kept as a tuple to stay axum-`IntoResponse`-compatible without
/// introducing a wrapper newtype this sub-mission doesn't yet need.
pub(crate) type ErrorResponse = (StatusCode, HeaderMap, ErrorBody);

/// Public alias for the `(status, headers, body)` triple cherry-pit-web
/// handlers convert into an axum response.
///
/// CHE-0050 names this concept `ErrorEnvelope` on the
/// [`CommandRouter::dispatch`](crate::CommandRouter::dispatch) return
/// type. Rather than introduce a new struct (the brief explicitly
/// forbids inventing new error types), the alias re-exports the
/// existing internal triple under the ADR's canonical name. Routers
/// build values via the public `map_dispatch_error` /
/// `map_store_error` / `map_bus_error` helpers; cherry-pit-web's
/// handlers attach the `X-Correlation-ID` echo header
/// (CHE-0049 R5) and convert into `IntoResponse` at the HTTP edge.
///
/// # Example
///
/// ```
/// use axum::http::StatusCode;
/// use cherry_pit_core::BusError;
/// use cherry_pit_web::errors::{ErrorEnvelope, map_bus_error};
///
/// let err = BusError::new("publish failed");
/// let envelope: ErrorEnvelope = map_bus_error(&err);
/// let (status, _headers, body) = envelope;
/// assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
/// assert_eq!(body.code, "bus");
/// ```
pub type ErrorEnvelope = ErrorResponse;

/// Build a `Retry-After: 1` single-header map.
///
/// The header value is a fixed ASCII numeric string and constructing
/// `HeaderValue` from it is infallible.
fn retry_after_headers() -> HeaderMap {
    let mut h = HeaderMap::with_capacity(1);
    h.insert(RETRY_AFTER, HeaderValue::from_static(RETRY_AFTER_SECONDS));
    h
}

fn no_headers() -> HeaderMap {
    HeaderMap::new()
}

/// Map a [`DispatchError<E>`] to an HTTP response triple.
///
/// Realises CHE-0049 R4 + R10. The `E` parameter's information is
/// preserved via `Display` only — see module docs for the lossless-body
/// contract and why `Serialize` is not required on `E`.
///
/// # Example
///
/// ```
/// use std::num::NonZeroU64;
/// use axum::http::StatusCode;
/// use cherry_pit_core::{AggregateId, DispatchError};
/// use cherry_pit_web::errors::map_dispatch_error;
///
/// let err: DispatchError<std::convert::Infallible> =
///     DispatchError::AggregateNotFound {
///         aggregate_id: AggregateId::new(NonZeroU64::new(42).unwrap()),
///     };
/// let (status, _headers, body) = map_dispatch_error(&err);
/// assert_eq!(status, StatusCode::NOT_FOUND);
/// assert_eq!(body.code, "aggregate_not_found");
/// ```
#[must_use]
pub fn map_dispatch_error<E>(err: &DispatchError<E>) -> ErrorResponse
where
    E: Error + Send + Sync,
{
    match err {
        DispatchError::Rejected(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            no_headers(),
            ErrorBody::new(code::REJECTED, err),
        ),
        DispatchError::ConcurrencyConflict { .. } => (
            StatusCode::CONFLICT,
            no_headers(),
            ErrorBody::new(code::CONCURRENCY_CONFLICT, err),
        ),
        DispatchError::AggregateNotFound { .. } => (
            StatusCode::NOT_FOUND,
            no_headers(),
            ErrorBody::new(code::AGGREGATE_NOT_FOUND, err),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            retry_after_headers(),
            ErrorBody::new(code::INFRASTRUCTURE, err),
        ),
    }
}

/// Map a [`StoreError`] to an HTTP response triple.
///
/// Realises CHE-0049 R4 + R10. Concurrency conflicts return 409;
/// retryable infrastructure failures (locked / generic infrastructure)
/// return 503 with `Retry-After`; corrupt data returns 500.
///
/// # Example
///
/// ```
/// use axum::http::{StatusCode, header::RETRY_AFTER};
/// use cherry_pit_core::StoreError;
/// use cherry_pit_web::errors::map_store_error;
///
/// let err = StoreError::StoreLocked { path: "/data/store".into() };
/// let (status, headers, body) = map_store_error(&err);
/// assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
/// assert!(headers.contains_key(RETRY_AFTER));
/// assert_eq!(body.code, "store_locked");
/// ```
#[must_use]
pub fn map_store_error(err: &StoreError) -> ErrorResponse {
    match err {
        StoreError::ConcurrencyConflict { .. } => (
            StatusCode::CONFLICT,
            no_headers(),
            ErrorBody::new(code::CONCURRENCY_CONFLICT, err),
        ),
        StoreError::StoreLocked { .. } => (
            StatusCode::SERVICE_UNAVAILABLE,
            retry_after_headers(),
            ErrorBody::new(code::STORE_LOCKED, err),
        ),
        StoreError::CorruptData(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            no_headers(),
            ErrorBody::new(code::CORRUPT_DATA, err),
        ),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            retry_after_headers(),
            ErrorBody::new(code::INFRASTRUCTURE, err),
        ),
    }
}

/// Map a [`BusError`] to an HTTP response triple.
///
/// `BusError` is always retryable per its type contract (CHE-0021:R3);
/// surfaces from a successful command whose post-persist publication
/// failed. Mapped to 503 + `Retry-After`.
///
/// # Example
///
/// ```
/// use axum::http::{StatusCode, header::RETRY_AFTER};
/// use cherry_pit_core::BusError;
/// use cherry_pit_web::errors::map_bus_error;
///
/// let err = BusError::new("publish failed");
/// let (status, headers, body) = map_bus_error(&err);
/// assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
/// assert!(headers.contains_key(RETRY_AFTER));
/// assert_eq!(body.code, "bus");
/// ```
#[must_use]
pub fn map_bus_error(err: &BusError) -> ErrorResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        retry_after_headers(),
        ErrorBody::new(code::BUS, err),
    )
}

/// Response for post-persist cancellation per CHE-0046:R5.
///
/// The command succeeded (events persisted) but the caller's wait was
/// cancelled before the gateway could return the envelopes. The client
/// should treat the outcome as unknown but safe to replay using its
/// idempotency key.
///
/// # Example
///
/// ```
/// use axum::http::StatusCode;
/// use cherry_pit_web::errors::post_persist_cancellation_response;
///
/// let (status, headers, body) = post_persist_cancellation_response();
/// assert_eq!(status, StatusCode::ACCEPTED);
/// assert!(headers.is_empty());
/// assert_eq!(body.code, "accepted_unknown");
/// ```
#[must_use]
pub fn post_persist_cancellation_response() -> ErrorResponse {
    (
        StatusCode::ACCEPTED,
        no_headers(),
        ErrorBody::new(
            code::ACCEPTED_UNKNOWN,
            "command persisted; outcome delivery cancelled — replay is safe",
        ),
    )
}

#[cfg(test)]
mod tests {
    //! Unit tests — one row per mapping table entry plus invariants.
    //!
    //! Tests assert the (status, header-presence, body.code) tuple
    //! and where applicable the lossless `Display` propagation.

    use super::*;
    use cherry_pit_core::AggregateId;
    use std::fmt;
    use std::num::NonZeroU64;

    #[derive(Debug)]
    struct DomainErr(&'static str);
    impl fmt::Display for DomainErr {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.0)
        }
    }
    impl Error for DomainErr {}

    fn agg_id(n: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(n).unwrap())
    }

    fn has_retry_after(h: &HeaderMap) -> bool {
        h.get(RETRY_AFTER) == Some(&HeaderValue::from_static(RETRY_AFTER_SECONDS))
    }

    #[test]
    fn dispatch_rejected_maps_to_422_with_lossless_body() {
        let err: DispatchError<DomainErr> =
            DispatchError::Rejected(DomainErr("invariant X violated"));
        let (status, headers, body) = map_dispatch_error(&err);

        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(headers.is_empty(), "422 carries no special headers");
        assert_eq!(body.code, "rejected");
        assert!(
            body.message.contains("invariant X violated"),
            "Rejected body must preserve domain error Display losslessly: {}",
            body.message
        );
        assert!(body.correlation_id.is_none());
    }

    #[test]
    fn dispatch_concurrency_conflict_maps_to_409() {
        let err: DispatchError<DomainErr> = DispatchError::ConcurrencyConflict {
            aggregate_id: agg_id(7),
            expected_sequence: NonZeroU64::new(3).unwrap(),
            actual_sequence: 4,
        };
        let (status, headers, body) = map_dispatch_error(&err);

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            !has_retry_after(&headers),
            "409 from caller-intent concurrency carries no Retry-After"
        );
        assert_eq!(body.code, "concurrency_conflict");
    }

    #[test]
    fn dispatch_aggregate_not_found_maps_to_404() {
        let err: DispatchError<DomainErr> = DispatchError::AggregateNotFound {
            aggregate_id: agg_id(99),
        };
        let (status, _h, body) = map_dispatch_error(&err);

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body.code, "aggregate_not_found");
        assert!(body.message.contains("99"));
    }

    #[test]
    fn dispatch_infrastructure_maps_to_503_retryable() {
        let err: DispatchError<DomainErr> = DispatchError::Infrastructure("gateway timeout".into());
        let (status, headers, body) = map_dispatch_error(&err);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            has_retry_after(&headers),
            "Retryable infrastructure must carry Retry-After"
        );
        assert_eq!(body.code, "infrastructure");
    }

    #[test]
    fn store_concurrency_conflict_maps_to_409() {
        let err = StoreError::ConcurrencyConflict {
            aggregate_id: agg_id(1),
            expected_sequence: NonZeroU64::new(5).unwrap(),
            actual_sequence: 6,
        };
        let (status, headers, body) = map_store_error(&err);

        assert_eq!(status, StatusCode::CONFLICT);
        assert!(!has_retry_after(&headers));
        assert_eq!(body.code, "concurrency_conflict");
    }

    #[test]
    fn store_locked_maps_to_503_with_retry_after() {
        let err = StoreError::StoreLocked {
            path: "/data/store".into(),
        };
        let (status, headers, body) = map_store_error(&err);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(has_retry_after(&headers));
        assert_eq!(body.code, "store_locked");
        assert!(body.message.contains("/data/store"));
    }

    #[test]
    fn store_corrupt_data_maps_to_500() {
        let err = StoreError::CorruptData("checksum mismatch".into());
        let (status, headers, body) = map_store_error(&err);

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            !has_retry_after(&headers),
            "Terminal corrupt data is not retryable"
        );
        assert_eq!(body.code, "corrupt_data");
    }

    #[test]
    fn store_infrastructure_maps_to_503_with_retry_after() {
        let err = StoreError::Infrastructure("disk full".into());
        let (status, headers, body) = map_store_error(&err);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(has_retry_after(&headers));
        assert_eq!(body.code, "infrastructure");
    }

    #[test]
    fn bus_error_maps_to_503_with_retry_after() {
        let err = BusError::new("publish failed");
        let (status, headers, body) = map_bus_error(&err);

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(has_retry_after(&headers));
        assert_eq!(body.code, "bus");
    }

    #[test]
    fn post_persist_cancellation_maps_to_202() {
        let (status, headers, body) = post_persist_cancellation_response();

        assert_eq!(status, StatusCode::ACCEPTED);
        assert!(headers.is_empty());
        assert_eq!(body.code, "accepted_unknown");
    }

    #[test]
    fn http_409_is_reserved_for_concurrency_conflict() {
        let dispatch_others: Vec<DispatchError<DomainErr>> = vec![
            DispatchError::Rejected(DomainErr("x")),
            DispatchError::AggregateNotFound {
                aggregate_id: agg_id(1),
            },
            DispatchError::Infrastructure("io".into()),
        ];
        for e in &dispatch_others {
            let (status, _, _) = map_dispatch_error(e);
            assert_ne!(
                status,
                StatusCode::CONFLICT,
                "non-ConcurrencyConflict DispatchError must not produce 409: {e}"
            );
        }

        let store_others = [
            StoreError::StoreLocked { path: "/x".into() },
            StoreError::CorruptData("bad".into()),
            StoreError::Infrastructure("io".into()),
        ];
        for e in &store_others {
            let (status, _, _) = map_store_error(e);
            assert_ne!(
                status,
                StatusCode::CONFLICT,
                "non-ConcurrencyConflict StoreError must not produce 409: {e}"
            );
        }

        let (bus_status, _, _) = map_bus_error(&BusError::new("x"));
        assert_ne!(bus_status, StatusCode::CONFLICT);
        let (cancel_status, _, _) = post_persist_cancellation_response();
        assert_ne!(cancel_status, StatusCode::CONFLICT);
    }

    #[test]
    fn error_body_serializes_without_correlation_when_absent() {
        let body = ErrorBody {
            code: "rejected",
            message: "boom".to_string(),
            correlation_id: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(r#""code":"rejected""#));
        assert!(json.contains(r#""message":"boom""#));
        assert!(
            !json.contains("correlation_id"),
            "correlation_id must be skipped when None: {json}"
        );
    }

    #[test]
    fn error_body_serializes_with_correlation_when_present() {
        let body = ErrorBody {
            code: "rejected",
            message: "boom".to_string(),
            correlation_id: Some("trace-abc".to_string()),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(r#""correlation_id":"trace-abc""#));
    }

    #[test]
    fn with_correlation_populates_when_id_present() {
        let id = uuid::Uuid::now_v7();
        let body = ErrorBody::new("rejected", "boom")
            .with_correlation(&CorrelationContext::correlated(id));
        assert_eq!(
            body.correlation_id.as_deref(),
            Some(id.to_string()).as_deref()
        );
    }

    #[test]
    fn with_correlation_leaves_none_when_context_empty() {
        let body = ErrorBody::new("rejected", "boom").with_correlation(&CorrelationContext::none());
        assert!(body.correlation_id.is_none());
    }
}
