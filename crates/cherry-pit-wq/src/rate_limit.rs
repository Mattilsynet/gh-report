//! Generic rate-limit observer state.
//!
//! Tracks remaining-budget snapshots reported by an upstream resource
//! adapter (e.g. an HTTP client wrapping `x-ratelimit-*` headers,
//! a database connection pool, a `SaaS` quota probe). The observer is
//! transport-agnostic: callers translate their source-specific signal
//! into a `(remaining, limit, reset)` triple and call
//! [`RateLimitState::observe`].
//!
//! `is_near_limit()` is advisory and uses `Relaxed` ordering — a stale
//! value merely causes observability lag, not correctness issues.
//!
//! `should_halt()` is a hard gate and uses `Acquire`/`Release` ordering
//! to ensure the halt decision is promptly visible across threads.
//!
//! Policy-specific thresholds (e.g. GitHub's `5_000/h` budget) belong
//! to the adapter crate that owns the source semantics. Construct via
//! [`RateLimitState::with_thresholds`]; the [`Default`] instance never
//! halts and never warns (both thresholds zero).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use tracing::warn;

/// Sentinel value for `remaining` field indicating "not yet known."
///
/// Uses `u32::MAX`. Collides with a theoretically valid observation,
/// but in practice rate-limit windows are far smaller, so `u32::MAX`
/// will never appear as a real remaining value.
const REMAINING_UNKNOWN: u32 = u32::MAX;

/// A single snapshot of rate-limit signal reported by an upstream
/// resource adapter.
///
/// Fields are all optional: an adapter populates only the fields it
/// observed in its source (e.g. an HTTP header set, a quota probe
/// response). Pass to [`RateLimitState::observe`] to merge into the
/// observer state.
///
/// `Copy` and cheaply constructible by struct literal or via the
/// chained `with_*` helpers:
///
/// ```
/// use cherry_pit_wq::RateLimitObservation;
///
/// let full = RateLimitObservation {
///     limit: Some(5000),
///     remaining: Some(4999),
///     reset: Some(1_700_000_000),
/// };
/// let partial = RateLimitObservation::new().with_remaining(42);
/// ```
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitObservation {
    /// Total requests allowed in the current window, if observed.
    pub limit: Option<u32>,
    /// Remaining requests in the current window, if observed.
    pub remaining: Option<u32>,
    /// Unix timestamp when the rate limit resets, if observed.
    pub reset: Option<u64>,
}

impl RateLimitObservation {
    /// Empty observation (all fields `None`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the `limit` field.
    #[must_use]
    pub fn with_limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Set the `remaining` field.
    #[must_use]
    pub fn with_remaining(mut self, remaining: u32) -> Self {
        self.remaining = Some(remaining);
        self
    }

    /// Set the `reset` field.
    #[must_use]
    pub fn with_reset(mut self, reset: u64) -> Self {
        self.reset = Some(reset);
        self
    }
}

/// Tracks rate-limit observations from an upstream resource adapter.
///
/// Thread-safe via atomics; no locking required.
#[derive(Debug)]
#[non_exhaustive]
pub struct RateLimitState {
    /// Total requests allowed in the current window (0 = unknown).
    limit: AtomicU32,
    /// Remaining requests in the current window (`u32::MAX` = unknown).
    remaining: AtomicU32,
    /// Unix timestamp when the rate limit resets (0 = unknown).
    reset: AtomicU64,
    /// Whether the near-exhaustion warning has already been emitted.
    /// Resets when `remaining` climbs back above the advisory threshold.
    warned_near_limit: AtomicBool,
    /// Remaining count below which [`should_halt`](Self::should_halt) returns `true`.
    halt_threshold: u32,
    /// Remaining count below which a warning is emitted.
    warn_threshold: u32,
}

impl Default for RateLimitState {
    /// Never-halt, never-warn observer.
    ///
    /// Use [`with_thresholds`](Self::with_thresholds) to attach policy.
    fn default() -> Self {
        Self {
            limit: AtomicU32::new(0),
            remaining: AtomicU32::new(REMAINING_UNKNOWN),
            reset: AtomicU64::new(0),
            warned_near_limit: AtomicBool::new(false),
            halt_threshold: 0,
            warn_threshold: 0,
        }
    }
}

impl RateLimitState {
    /// Create a new `RateLimitState` with the given thresholds.
    ///
    /// - `halt_threshold`: [`should_halt`](Self::should_halt) returns `true`
    ///   when `remaining` drops below this value.
    /// - `warn_threshold`: a warning is emitted when `remaining` drops below
    ///   this value.
    #[must_use]
    pub fn with_thresholds(halt_threshold: u32, warn_threshold: u32) -> Self {
        Self {
            halt_threshold,
            warn_threshold,
            ..Self::default()
        }
    }

    /// Record an observation from the upstream adapter.
    ///
    /// `remaining` uses `Release` ordering so that `should_halt()` (which
    /// loads with `Acquire`) sees the update promptly. `limit` and `reset`
    /// are advisory and use `Relaxed`. Each field is only updated when
    /// the observation carries a value, so adapters can omit fields they
    /// did not observe.
    pub fn observe(&self, obs: RateLimitObservation) {
        if let Some(limit) = obs.limit {
            self.limit.store(limit, Ordering::Relaxed);
        }
        if let Some(remaining) = obs.remaining {
            self.remaining.store(remaining, Ordering::Release);
            if self.warn_threshold > 0 && remaining < self.warn_threshold {
                if !self.warned_near_limit.swap(true, Ordering::Relaxed) {
                    warn!(
                        remaining,
                        limit = self.limit.load(Ordering::Relaxed),
                        "rate limit is near exhaustion"
                    );
                }
            } else {
                self.warned_near_limit.store(false, Ordering::Relaxed);
            }
        }
        if let Some(reset) = obs.reset {
            self.reset.store(reset, Ordering::Relaxed);
        }
    }

    /// Load the current limit, or `None` if not yet known.
    #[must_use]
    pub fn load_limit(&self) -> Option<u32> {
        match self.limit.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v),
        }
    }

    /// Load the current remaining count, or `None` if not yet known.
    #[must_use]
    pub fn load_remaining(&self) -> Option<u32> {
        match self.remaining.load(Ordering::Acquire) {
            REMAINING_UNKNOWN => None,
            v => Some(v),
        }
    }

    /// Load the reset timestamp, or `None` if not yet known.
    #[must_use]
    pub fn load_reset(&self) -> Option<u64> {
        match self.reset.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v),
        }
    }

    /// Configured halt threshold (zero ⇒ never halts).
    #[must_use]
    pub fn halt_threshold(&self) -> u32 {
        self.halt_threshold
    }

    /// Configured warn threshold (zero ⇒ never warns).
    #[must_use]
    pub fn warn_threshold(&self) -> u32 {
        self.warn_threshold
    }

    /// Check if we are close to the rate limit (advisory).
    ///
    /// Returns `true` when `remaining` is below the configured warn threshold.
    #[must_use]
    pub fn is_near_limit(&self) -> bool {
        if self.warn_threshold == 0 {
            return false;
        }
        matches!(self.load_remaining(), Some(r) if r < self.warn_threshold)
    }

    /// Hard halt check: returns `true` when remaining drops below the
    /// configured halt threshold and we have a known remaining value.
    ///
    /// Returns `false` when remaining is unknown (sentinel) — we cannot
    /// halt on missing data because that would block runs that haven't
    /// observed any signal yet.
    #[must_use]
    pub fn should_halt(&self) -> bool {
        if self.halt_threshold == 0 {
            return false;
        }
        matches!(self.load_remaining(), Some(r) if r < self.halt_threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_populates_all_fields() {
        let state = RateLimitState::default();
        state.observe(RateLimitObservation {
            limit: Some(5000),
            remaining: Some(4999),
            reset: Some(1_700_000_000),
        });
        assert_eq!(state.load_limit(), Some(5000));
        assert_eq!(state.load_remaining(), Some(4999));
        assert_eq!(state.load_reset(), Some(1_700_000_000));
    }

    #[test]
    fn observe_omitted_fields_are_unchanged() {
        let state = RateLimitState::default();
        state.observe(
            RateLimitObservation::new()
                .with_limit(100)
                .with_remaining(42),
        );
        state.observe(RateLimitObservation::new().with_reset(1));
        assert_eq!(state.load_limit(), Some(100));
        assert_eq!(state.load_remaining(), Some(42));
        assert_eq!(state.load_reset(), Some(1));
    }

    #[test]
    fn default_never_halts_or_warns() {
        let state = RateLimitState::default();
        state.observe(RateLimitObservation::new().with_remaining(0));
        assert!(!state.should_halt());
        assert!(!state.is_near_limit());
    }

    #[test]
    fn should_halt_below_threshold() {
        let state = RateLimitState::with_thresholds(50, 100);
        state.observe(RateLimitObservation::new().with_remaining(49));
        assert!(state.should_halt());
    }

    #[test]
    fn should_halt_at_threshold_returns_false() {
        let state = RateLimitState::with_thresholds(50, 100);
        state.observe(RateLimitObservation::new().with_remaining(50));
        assert!(!state.should_halt());
    }

    #[test]
    fn should_halt_unknown_remaining_returns_false() {
        let state = RateLimitState::with_thresholds(50, 100);
        assert!(!state.should_halt());
    }

    #[test]
    fn should_halt_at_zero() {
        let state = RateLimitState::with_thresholds(50, 100);
        state.observe(RateLimitObservation::new().with_remaining(0));
        assert!(state.should_halt());
    }

    #[test]
    fn warned_near_limit_resets_on_recovery() {
        let state = RateLimitState::with_thresholds(50, 100);
        state.observe(RateLimitObservation::new().with_remaining(50));
        assert!(state.warned_near_limit.load(Ordering::Relaxed));
        state.observe(RateLimitObservation::new().with_remaining(200));
        assert!(!state.warned_near_limit.load(Ordering::Relaxed));
        state.observe(RateLimitObservation::new().with_remaining(30));
        assert!(state.warned_near_limit.load(Ordering::Relaxed));
    }

    #[test]
    fn custom_thresholds() {
        let state = RateLimitState::with_thresholds(10, 50);
        state.observe(RateLimitObservation::new().with_remaining(40));
        assert!(state.is_near_limit());
        assert!(!state.should_halt());
        state.observe(RateLimitObservation::new().with_remaining(5));
        assert!(state.should_halt());
    }
}
