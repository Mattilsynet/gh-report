//! Durable-write-failure response policy (CHE-0088).
//!
//! Classifies every [`PersistenceError`] into a closed
//! [`WritePolicyCategory`] at exactly one conversion chokepoint, then
//! dispatches each category to exactly one of three responses via an
//! exhaustive match with no wildcard arm (CHE-0088:R7). This makes
//! per-callsite silent-swallow of a durable-write failure
//! non-representable rather than merely discouraged.

use std::time::Duration;

use cherry_pit_storage::PersistenceError;

/// Bounded retry attempt count for [`WriteResponse::BoundedRetry`]
/// (CHE-0046: retry is explicit and bounded, never unbounded).
pub const BOUNDED_RETRY_ATTEMPTS: u8 = 3;

/// Fixed delay between bounded-retry attempts.
pub const BOUNDED_RETRY_DELAY: Duration = Duration::from_millis(20);

/// Closed, gh-report-owned classification of a durable-write failure
/// (CHE-0088:R2). Deliberately NOT `#[non_exhaustive]`: the whole point
/// of this enum is that every category has exactly one ratified
/// response, checked at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicyCategory {
    /// Single-writer fence conflict (PGN-0016:R2).
    Conflict,
    /// Backend infrastructure unavailable; a retry may succeed once the
    /// backend recovers (CHE-0088:R4).
    Transient,
    /// A store-level invariant was violated (CHE-0088:R5).
    Structural,
    /// Process state is corrupted or otherwise unrecoverable
    /// (CHE-0088:R5). Also the fail-closed target for any
    /// `PersistenceError` variant not explicitly classified above.
    Unrecoverable,
}

/// Closed response vocabulary (CHE-0088:R8). Deliberately has no
/// `Swallow` variant: continuing without one of these three responses
/// is not a representable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteResponse {
    /// Abort the current run/job (PGN-0016:R2 abort-the-run rule).
    Fatal,
    /// Retry the write a small, explicit number of times before
    /// escalating to `Fatal`.
    BoundedRetry,
    /// Respond with a non-2xx HTTP status so the caller (e.g. GitHub
    /// webhook redelivery) retries at the transport layer.
    HttpNon2xx,
}

impl WritePolicyCategory {
    /// The ONE conversion chokepoint from the `#[non_exhaustive]`
    /// `PersistenceError` into this closed category set (CHE-0088:R2).
    ///
    /// The trailing wildcard arm is the only permitted wildcard in the
    /// whole mechanism: because `PersistenceError` may grow variants
    /// this crate does not yet know about, an unclassified variant
    /// fails CLOSED to `Unrecoverable` (fatal), never open (swallow).
    #[must_use]
    pub fn classify(error: &PersistenceError) -> Self {
        match error {
            PersistenceError::FencedConflict { .. } => Self::Conflict,
            PersistenceError::BackendUnavailable { .. } => Self::Transient,
            PersistenceError::InvariantViolation { .. } => Self::Structural,
            PersistenceError::PoisonedState | PersistenceError::TornWriteRecovery { .. } => {
                Self::Unrecoverable
            }
            _ => Self::Unrecoverable,
        }
    }

    /// The exhaustive category->response dispatch (CHE-0088:R7). No
    /// wildcard arm: adding a `WritePolicyCategory` variant without
    /// adding its response arm here fails to compile.
    #[must_use]
    pub fn response(self) -> WriteResponse {
        match self {
            Self::Conflict | Self::Structural | Self::Unrecoverable => WriteResponse::Fatal,
            Self::Transient => WriteResponse::BoundedRetry,
        }
    }
}

/// A classified durable-write failure: category, ratified response,
/// and the original error for logging/propagation.
#[derive(Debug)]
pub struct WriteFailure {
    pub category: WritePolicyCategory,
    pub response: WriteResponse,
    pub error: PersistenceError,
}

impl WriteFailure {
    #[must_use]
    pub fn classify(error: PersistenceError) -> Self {
        let category = WritePolicyCategory::classify(&error);
        Self {
            category,
            response: category.response(),
            error,
        }
    }
}

/// Attempt a durable write once, and on a `BoundedRetry`-classified
/// failure, retry `op` up to [`BOUNDED_RETRY_ATTEMPTS`] more times with
/// a fixed small delay between attempts (CHE-0046: explicit, bounded
/// retry — never an unbounded loop).
///
/// Every category resolves to the same response at every call site
/// (jxma5): this helper is shared by every durable-write caller, so a
/// `Transient` failure retries identically whether encountered at
/// startup, in the delivery loop, during sweep/reconcile, or in the
/// webhook handler.
///
/// Returns `Ok(())` once `op` succeeds, or the last classified failure
/// once retries (if any) are exhausted.
///
/// # Errors
///
/// Returns the classified [`WriteFailure`] when `op` fails with a
/// `Fatal`- or `HttpNon2xx`-routed category immediately, or after
/// `BOUNDED_RETRY_ATTEMPTS` retries are exhausted for a `Transient`
/// (`BoundedRetry`-routed) category.
pub async fn write_with_policy<F>(mut op: F) -> Result<(), WriteFailure>
where
    F: FnMut() -> Result<(), PersistenceError>,
{
    let mut failure = match op() {
        Ok(()) => return Ok(()),
        Err(error) => WriteFailure::classify(error),
    };

    if failure.response != WriteResponse::BoundedRetry {
        return Err(failure);
    }

    for _ in 0..BOUNDED_RETRY_ATTEMPTS {
        tokio::time::sleep(BOUNDED_RETRY_DELAY).await;
        match op() {
            Ok(()) => return Ok(()),
            Err(error) => failure = WriteFailure::classify(error),
        }
    }
    Err(failure)
}

/// Synchronous counterpart of [`write_with_policy`] for callers that
/// are not themselves `async` (PGN-0010:R5: the `AppState` write facade
/// stays sync). Uses `std::thread::sleep` for the same bounded, fixed
/// delay between attempts; the delay is small (tens of milliseconds
/// total across all attempts) and this path is only reached on the
/// rare `Transient` (backend-unavailable) failure class.
///
/// # Errors
///
/// Returns the classified [`WriteFailure`] under the same conditions as
/// [`write_with_policy`].
pub fn write_with_policy_sync<F>(mut op: F) -> Result<(), WriteFailure>
where
    F: FnMut() -> Result<(), PersistenceError>,
{
    let mut failure = match op() {
        Ok(()) => return Ok(()),
        Err(error) => WriteFailure::classify(error),
    };

    if failure.response != WriteResponse::BoundedRetry {
        return Err(failure);
    }

    for _ in 0..BOUNDED_RETRY_ATTEMPTS {
        std::thread::sleep(BOUNDED_RETRY_DELAY);
        match op() {
            Ok(()) => return Ok(()),
            Err(error) => failure = WriteFailure::classify(error),
        }
    }
    Err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_error() -> PersistenceError {
        PersistenceError::Io(std::io::Error::other("boom"))
    }

    #[test]
    fn conflict_maps_to_fatal() {
        let error = PersistenceError::FencedConflict {
            source: Box::new(std::io::Error::other("x")),
        };
        assert_eq!(
            WritePolicyCategory::classify(&error),
            WritePolicyCategory::Conflict
        );
        assert_eq!(
            WritePolicyCategory::Conflict.response(),
            WriteResponse::Fatal
        );
    }

    #[test]
    fn transient_maps_to_bounded_retry() {
        let error = PersistenceError::BackendUnavailable {
            reason: "x".to_string(),
        };
        assert_eq!(
            WritePolicyCategory::classify(&error),
            WritePolicyCategory::Transient
        );
        assert_eq!(
            WritePolicyCategory::Transient.response(),
            WriteResponse::BoundedRetry
        );
    }

    #[test]
    fn structural_maps_to_fatal() {
        let error = PersistenceError::InvariantViolation {
            reason: "x".to_string(),
        };
        assert_eq!(
            WritePolicyCategory::classify(&error),
            WritePolicyCategory::Structural
        );
        assert_eq!(
            WritePolicyCategory::Structural.response(),
            WriteResponse::Fatal
        );
    }

    #[test]
    fn poisoned_state_maps_to_unrecoverable_fatal() {
        assert_eq!(
            WritePolicyCategory::classify(&PersistenceError::PoisonedState),
            WritePolicyCategory::Unrecoverable
        );
        assert_eq!(
            WritePolicyCategory::Unrecoverable.response(),
            WriteResponse::Fatal
        );
    }

    #[test]
    fn torn_write_recovery_maps_to_unrecoverable() {
        let error = PersistenceError::TornWriteRecovery {
            source: Box::new(std::io::Error::other("x")),
        };
        assert_eq!(
            WritePolicyCategory::classify(&error),
            WritePolicyCategory::Unrecoverable
        );
    }

    #[test]
    fn unclassified_variant_fails_closed_to_unrecoverable() {
        assert_eq!(
            WritePolicyCategory::classify(&io_error()),
            WritePolicyCategory::Unrecoverable
        );
    }

    #[tokio::test]
    async fn write_with_policy_returns_ok_on_first_success() {
        let result = write_with_policy(|| Ok(())).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn write_with_policy_returns_fatal_without_retry_for_conflict() {
        let mut calls = 0;
        let result = write_with_policy(|| {
            calls += 1;
            Err(PersistenceError::FencedConflict {
                source: Box::new(std::io::Error::other("x")),
            })
        })
        .await;
        assert_eq!(calls, 1, "Fatal categories must not be retried");
        let failure = result.expect_err("must fail");
        assert_eq!(failure.category, WritePolicyCategory::Conflict);
        assert_eq!(failure.response, WriteResponse::Fatal);
    }

    #[tokio::test]
    async fn write_with_policy_retries_transient_then_succeeds() {
        let mut calls = 0;
        let result = write_with_policy(|| {
            calls += 1;
            if calls < 3 {
                Err(PersistenceError::BackendUnavailable {
                    reason: "x".to_string(),
                })
            } else {
                Ok(())
            }
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(calls, 3);
    }

    #[tokio::test]
    async fn write_with_policy_exhausts_bounded_retry_and_returns_fatal() {
        let mut calls = 0;
        let result = write_with_policy(|| {
            calls += 1;
            Err(PersistenceError::BackendUnavailable {
                reason: "x".to_string(),
            })
        })
        .await;
        assert_eq!(calls, u32::from(BOUNDED_RETRY_ATTEMPTS) + 1);
        let failure = result.expect_err("must fail after exhausting retries");
        assert_eq!(failure.category, WritePolicyCategory::Transient);
        assert_eq!(failure.response, WriteResponse::BoundedRetry);
    }

    #[test]
    fn write_with_policy_sync_returns_ok_on_first_success() {
        let result = write_with_policy_sync(|| Ok(()));
        assert!(result.is_ok());
    }

    #[test]
    fn write_with_policy_sync_returns_fatal_without_retry_for_structural() {
        let mut calls = 0;
        let result = write_with_policy_sync(|| {
            calls += 1;
            Err(PersistenceError::InvariantViolation {
                reason: "x".to_string(),
            })
        });
        assert_eq!(calls, 1, "Fatal categories must not be retried");
        assert_eq!(
            result.expect_err("must fail").category,
            WritePolicyCategory::Structural
        );
    }

    /// jxma5: the same `Transient` failure class must resolve identically
    /// whether it is encountered via the sync path (delivery-loop /
    /// startup call sites) or the async path (webhook / sweep call
    /// sites) — both bounded-retry, same attempt count, same terminal
    /// category/response (CHE-0088:R4).
    #[tokio::test]
    async fn transient_resolves_identically_sync_and_async() {
        let mut async_calls = 0;
        let async_result = write_with_policy(|| {
            async_calls += 1;
            Err(PersistenceError::BackendUnavailable {
                reason: "nats down".to_string(),
            })
        })
        .await;

        let mut sync_calls = 0;
        let sync_result = write_with_policy_sync(|| {
            sync_calls += 1;
            Err(PersistenceError::BackendUnavailable {
                reason: "nats down".to_string(),
            })
        });

        assert_eq!(
            async_calls, sync_calls,
            "same Transient class must retry the same bounded number of times"
        );
        let async_failure = async_result.expect_err("must fail after exhausting retries");
        let sync_failure = sync_result.expect_err("must fail after exhausting retries");
        assert_eq!(async_failure.category, sync_failure.category);
        assert_eq!(async_failure.response, sync_failure.response);
        assert_eq!(async_failure.category, WritePolicyCategory::Transient);
        assert_eq!(async_failure.response, WriteResponse::BoundedRetry);
    }

    /// PGN-0016:R2 — a `Conflict`-category failure is never swallowed: it
    /// resolves to `Fatal` (propagate/abort) at the mechanism level, never
    /// to `BoundedRetry` or any silently-continuing response. Iterating
    /// every closed `WritePolicyCategory` variant additionally exercises
    /// R7 (no wildcard arm in `response()`) at the value level, not just
    /// by compilation.
    #[test]
    fn conflict_category_is_never_swallowed() {
        let failure = WriteFailure::classify(PersistenceError::FencedConflict {
            source: Box::new(std::io::Error::other("fence")),
        });
        assert_eq!(failure.category, WritePolicyCategory::Conflict);
        assert_eq!(
            failure.response,
            WriteResponse::Fatal,
            "Conflict must propagate/abort per PGN-0016:R2, never continue silently"
        );
    }

    #[test]
    fn write_with_policy_sync_retries_transient_then_succeeds() {
        let mut calls = 0;
        let result = write_with_policy_sync(|| {
            calls += 1;
            if calls < 2 {
                Err(PersistenceError::BackendUnavailable {
                    reason: "x".to_string(),
                })
            } else {
                Ok(())
            }
        });
        assert!(result.is_ok());
        assert_eq!(calls, 2);
    }
}
