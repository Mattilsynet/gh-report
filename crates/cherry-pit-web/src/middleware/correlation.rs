//! HTTP-side correlation extraction.
//!
//! Realises **CHE-0049 R5** (correlation propagation) and
//! **CHE-0049 R6** (consumer-supplied idempotency key) at the inbound
//! edge. Pure header → value functions; no I/O, no allocation beyond
//! the resulting `Uuid` / `String` carriers.
//!
//! ## R5 — `traceparent` primary, `X-Correlation-ID` fallback
//!
//! [`extract_correlation`] inspects the request `HeaderMap`:
//!
//! 1. If a syntactically valid W3C `traceparent` is present with both
//!    `trace-id` and `parent-span-id` non-zero, return
//!    [`CorrelationContext::new(trace, parent)`]. The 64-bit
//!    parent-span-id is left-padded with zeros into a 128-bit
//!    [`Uuid`] (lossless up-cast — the low 64 bits carry the span id;
//!    the high 64 bits are zero by construction).
//! 2. Otherwise, if `X-Correlation-ID` parses as a [`Uuid`], return
//!    [`CorrelationContext::correlated(uuid)`].
//! 3. Otherwise, return [`CorrelationContext::none()`] per
//!    **CHE-0039 R2** (forgetting correlation is a conscious choice;
//!    no `Default` impl, no synthesis).
//!
//! Malformed `traceparent` does **not** reject the request — the W3C
//! spec instructs receivers to treat it as absent and continue.
//!
//! ## R6 — `Idempotency-Key` is consumer-supplied
//!
//! [`extract_idempotency_key`] returns `Some(IdempotencyKey)` only if
//! the header is present and non-empty. **It never auto-generates**:
//! per CHE-0046 R3 + CHE-0049 R6 the consumer-supplied stability is
//! the entire semantic guarantee — synthesising a key here would
//! silently break replay safety.
//!
//! [`CorrelationContext::new(trace, parent)`]: cherry_pit_core::CorrelationContext::new
//! [`CorrelationContext::correlated(uuid)`]: cherry_pit_core::CorrelationContext::correlated
//! [`CorrelationContext::none()`]: cherry_pit_core::CorrelationContext::none

use axum::http::HeaderMap;
use cherry_pit_core::CorrelationContext;
use uuid::Uuid;

// Pass-through re-export so `middleware::mod.rs` can keep its single
// canonical `pub use correlation::{IdempotencyKey, ...}` line. The
// type itself lives in `cherry-pit-core` (per CHE-0046 R3 + CHE-0049 R6
// the never-synthesise invariant is now structurally enforced — the
// public constructor `IdempotencyKey::from_header_value` is the only
// path that yields `Some` from outside core).
pub use cherry_pit_core::IdempotencyKey;

/// Canonical request/response header names, hard-coded ASCII.
pub(crate) const TRACEPARENT: &str = "traceparent";
pub(crate) const X_CORRELATION_ID: &str = "x-correlation-id";
pub(crate) const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// Extract a [`CorrelationContext`] from inbound HTTP headers.
///
/// See module docs for the precedence rules. Always returns a value;
/// the absent case is `CorrelationContext::none()`.
///
/// Per CHE-0049 R5 + CHE-0039 R2: never synthesises a correlation id;
/// absence is preserved end-to-end.
///
/// # Example
///
/// ```
/// use axum::http::{HeaderMap, HeaderValue};
/// use cherry_pit_web::correlation::extract_correlation;
/// use uuid::Uuid;
///
/// let id = Uuid::now_v7();
/// let mut headers = HeaderMap::new();
/// headers.insert(
///     "x-correlation-id",
///     HeaderValue::from_str(&id.to_string()).unwrap(),
/// );
///
/// let ctx = extract_correlation(&headers);
/// assert_eq!(ctx.correlation_id(), Some(id));
/// ```
#[must_use]
pub fn extract_correlation(headers: &HeaderMap) -> CorrelationContext {
    if let Some(ctx) = headers
        .get(TRACEPARENT)
        .and_then(|h| h.to_str().ok())
        .and_then(parse_traceparent)
    {
        return ctx;
    }

    if let Some(uuid) = headers
        .get(X_CORRELATION_ID)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
    {
        return CorrelationContext::correlated(uuid);
    }

    CorrelationContext::none()
}

/// Extract a consumer-supplied [`IdempotencyKey`] from inbound headers.
///
/// Returns `None` when the header is absent, non-ASCII, or empty after
/// trimming. **Never** auto-generates a key (CHE-0046 R3 + CHE-0049 R6).
///
/// # Example
///
/// ```
/// use axum::http::HeaderMap;
/// use cherry_pit_web::correlation::extract_idempotency_key;
///
/// // Absent header → None. R6 forbids auto-generation.
/// assert!(extract_idempotency_key(&HeaderMap::new()).is_none());
/// ```
#[must_use]
pub fn extract_idempotency_key(headers: &HeaderMap) -> Option<IdempotencyKey> {
    headers
        .get(IDEMPOTENCY_KEY)?
        .to_str()
        .ok()
        .and_then(IdempotencyKey::from_header_value)
}

/// Parse a W3C Trace Context `traceparent` header value.
///
/// Format: `00-<32 hex>-<16 hex>-<2 hex>` (55 chars total). Returns
/// `None` for any deviation, including all-zero `trace-id` or
/// `parent-span-id` per the W3C spec ("MUST treat as invalid"). The
/// caller's job is to fall through to fallback or `none()`.
fn parse_traceparent(value: &str) -> Option<CorrelationContext> {
    // Total length is fixed for version 00. Future versions are
    // explicitly out of scope (we only know how to interpret 00).
    if value.len() != 55 {
        return None;
    }
    let mut parts = value.split('-');
    let version = parts.next()?;
    let trace_id_hex = parts.next()?;
    let parent_span_hex = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_some() {
        return None;
    }

    if version != "00" {
        return None;
    }
    if trace_id_hex.len() != 32 || parent_span_hex.len() != 16 || flags.len() != 2 {
        return None;
    }

    let trace_id = u128::from_str_radix(trace_id_hex, 16).ok()?;
    let parent_span = u64::from_str_radix(parent_span_hex, 16).ok()?;
    // flags must be valid hex but are otherwise unused at this layer.
    u8::from_str_radix(flags, 16).ok()?;

    if trace_id == 0 || parent_span == 0 {
        return None;
    }

    let correlation = Uuid::from_u128(trace_id);
    // Lossless up-cast: 64-bit span id occupies the low 64 bits of a
    // 128-bit Uuid; the high 64 bits are zero by construction. The
    // mapping is total and reversible (`uuid.as_u128() as u64`).
    let causation = Uuid::from_u128(u128::from(parent_span));
    Some(CorrelationContext::new(correlation, causation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn hm() -> HeaderMap {
        HeaderMap::new()
    }

    fn with(name: &'static str, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(value).unwrap());
        h
    }

    // ── extract_correlation ────────────────────────────────────────────

    #[test]
    fn absent_headers_yield_none() {
        let ctx = extract_correlation(&hm());
        assert_eq!(ctx, CorrelationContext::none());
    }

    #[test]
    fn valid_traceparent_populates_both_ids() {
        // trace-id = 0x0af7651916cd43dd8448eb211c80319c
        // parent-span = 0x00f067aa0ba902b7
        let tp = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";
        let ctx = extract_correlation(&with(TRACEPARENT, tp));
        let expected_corr = Uuid::from_u128(0x0af7_6519_16cd_43dd_8448_eb21_1c80_319c);
        let expected_cause = Uuid::from_u128(u128::from(0x00f0_67aa_0ba9_02b7_u64));
        assert_eq!(ctx.correlation_id(), Some(expected_corr));
        assert_eq!(ctx.causation_id(), Some(expected_cause));
    }

    #[test]
    fn x_correlation_id_uuid_yields_correlated() {
        let id = Uuid::now_v7();
        let ctx = extract_correlation(&with(X_CORRELATION_ID, &id.to_string()));
        assert_eq!(ctx, CorrelationContext::correlated(id));
        assert!(ctx.causation_id().is_none());
    }

    #[test]
    fn malformed_traceparent_falls_through_to_fallback() {
        // Garbage traceparent + valid X-Correlation-ID → fallback wins.
        let mut h = HeaderMap::new();
        h.insert(TRACEPARENT, HeaderValue::from_static("not-a-traceparent"));
        let id = Uuid::now_v7();
        h.insert(
            X_CORRELATION_ID,
            HeaderValue::from_str(&id.to_string()).unwrap(),
        );
        let ctx = extract_correlation(&h);
        assert_eq!(ctx, CorrelationContext::correlated(id));
    }

    #[test]
    fn malformed_traceparent_and_no_fallback_yields_none() {
        let h = with(TRACEPARENT, "00-XXXX-YYYY-01");
        let ctx = extract_correlation(&h);
        assert_eq!(ctx, CorrelationContext::none());
    }

    #[test]
    fn unsupported_traceparent_version_is_treated_as_absent() {
        // Version "01" — we only speak "00". Fall through to none().
        let tp = "01-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";
        let ctx = extract_correlation(&with(TRACEPARENT, tp));
        assert_eq!(ctx, CorrelationContext::none());
    }

    #[test]
    fn all_zero_trace_id_is_treated_as_absent() {
        let tp = "00-00000000000000000000000000000000-00f067aa0ba902b7-01";
        let ctx = extract_correlation(&with(TRACEPARENT, tp));
        assert_eq!(ctx, CorrelationContext::none());
    }

    #[test]
    fn all_zero_parent_span_is_treated_as_absent() {
        let tp = "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01";
        let ctx = extract_correlation(&with(TRACEPARENT, tp));
        assert_eq!(ctx, CorrelationContext::none());
    }

    #[test]
    fn malformed_x_correlation_id_falls_through_to_none() {
        let ctx = extract_correlation(&with(X_CORRELATION_ID, "not-a-uuid"));
        assert_eq!(ctx, CorrelationContext::none());
    }

    #[test]
    fn traceparent_takes_precedence_over_x_correlation_id() {
        let tp = "00-0af7651916cd43dd8448eb211c80319c-00f067aa0ba902b7-01";
        let mut h = HeaderMap::new();
        h.insert(TRACEPARENT, HeaderValue::from_static(tp));
        h.insert(
            X_CORRELATION_ID,
            HeaderValue::from_str(&Uuid::now_v7().to_string()).unwrap(),
        );
        let ctx = extract_correlation(&h);
        // causation_id is populated only by traceparent path.
        assert!(ctx.causation_id().is_some());
    }

    #[test]
    fn wrong_length_traceparent_rejected() {
        let ctx = extract_correlation(&with(TRACEPARENT, "00-abc-def-01"));
        assert_eq!(ctx, CorrelationContext::none());
    }

    // ── extract_idempotency_key ────────────────────────────────────────

    #[test]
    fn idempotency_key_present_returns_some() {
        let key = extract_idempotency_key(&with(IDEMPOTENCY_KEY, "client-supplied-key-42"));
        assert_eq!(
            key.as_ref().map(IdempotencyKey::as_str),
            Some("client-supplied-key-42")
        );
    }

    #[test]
    fn idempotency_key_absent_returns_none() {
        // The whole point of R6: never auto-generated.
        assert_eq!(extract_idempotency_key(&hm()), None);
    }

    #[test]
    fn idempotency_key_empty_string_returns_none() {
        assert_eq!(extract_idempotency_key(&with(IDEMPOTENCY_KEY, "   ")), None);
    }

    #[test]
    fn idempotency_key_trims_surrounding_whitespace() {
        let key = extract_idempotency_key(&with(IDEMPOTENCY_KEY, "  abc  "));
        assert_eq!(key.unwrap().as_str(), "abc");
    }
}
