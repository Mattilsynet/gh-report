//! API rate limit tracking.
//!
//! Reads and reacts to `X-RateLimit-*` headers from API responses.
//!
//! `is_near_limit()` is advisory and uses `Relaxed` ordering — a stale
//! value merely causes observability lag, not correctness issues.
//!
//! `should_halt()` is a hard gate and uses `Acquire`/`Release` ordering
//! to ensure the halt decision is promptly visible across threads.

use http::HeaderMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use tracing::warn;

/// Sentinel value for `remaining` field indicating "not yet known."
///
/// Uses `u32::MAX` (4,294,967,295). This collides with a theoretically valid
/// header value, but in practice API rate limits are far smaller (e.g.,
/// GitHub allows at most 5,000/hour authenticated), so `u32::MAX` will
/// never appear as a real `x-ratelimit-remaining` value.
const REMAINING_UNKNOWN: u32 = u32::MAX;

/// Hard halt threshold. Collection stops when `remaining` drops below this.
pub const HALT_THRESHOLD: u32 = 50;

/// Advisory warning threshold. A log warning is emitted when `remaining`
/// drops below this value.
pub const WARN_THRESHOLD: u32 = 100;

/// Tracks API rate limit state from response headers.
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
    ///
    /// Uses 0 as the sentinel for "not yet known". Unix timestamp 0
    /// corresponds to 1970-01-01T00:00:00Z, which will never be a valid
    /// rate-limit reset time from any real API.
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
    fn default() -> Self {
        Self {
            limit: AtomicU32::new(0),
            remaining: AtomicU32::new(REMAINING_UNKNOWN),
            reset: AtomicU64::new(0),
            warned_near_limit: AtomicBool::new(false),
            halt_threshold: HALT_THRESHOLD,
            warn_threshold: WARN_THRESHOLD,
        }
    }
}

impl RateLimitState {
    /// Create a new `RateLimitState` with custom thresholds.
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

    /// Update rate limit state from response headers.
    ///
    /// `remaining` uses `Release` ordering so that `should_halt()` (which
    /// loads with `Acquire`) sees the update promptly. `limit` and `reset`
    /// are advisory and use `Relaxed`.
    pub fn update_from_headers(&self, headers: &HeaderMap) {
        if let Some(limit) = Self::parse_header::<u32>(headers, "x-ratelimit-limit") {
            self.limit.store(limit, Ordering::Relaxed);
        }
        if let Some(remaining) = Self::parse_header::<u32>(headers, "x-ratelimit-remaining") {
            self.remaining.store(remaining, Ordering::Release);
            if remaining < self.warn_threshold {
                // Emit the warning only once per breach to avoid log flooding
                // during large collection runs.  Resets when remaining climbs
                // back above the threshold (e.g., after a rate-limit window
                // reset).
                if !self.warned_near_limit.swap(true, Ordering::Relaxed) {
                    warn!(
                        remaining,
                        limit = self.limit.load(Ordering::Relaxed),
                        "API rate limit is near exhaustion"
                    );
                }
            } else {
                // Reset the flag once remaining climbs back above threshold.
                self.warned_near_limit.store(false, Ordering::Relaxed);
            }
        }
        if let Some(reset) = Self::parse_header::<u64>(headers, "x-ratelimit-reset") {
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

    /// Check if we are close to the rate limit (advisory).
    ///
    /// Returns `true` when `remaining` is below the configured
    /// `warn_threshold` (default: [`WARN_THRESHOLD`]).
    #[must_use]
    pub fn is_near_limit(&self) -> bool {
        matches!(self.load_remaining(), Some(r) if r < self.warn_threshold)
    }

    /// Hard halt check: returns `true` when remaining drops below
    /// [`HALT_THRESHOLD`] and we have a known remaining value.
    ///
    /// Returns `false` when remaining is unknown (sentinel) — we cannot
    /// halt on missing data because that would block runs that haven't
    /// made any API call yet.
    #[must_use]
    pub fn should_halt(&self) -> bool {
        matches!(self.load_remaining(), Some(r) if r < self.halt_threshold)
    }

    fn parse_header<T: std::str::FromStr>(headers: &HeaderMap, name: &str) -> Option<T> {
        headers.get(name)?.to_str().ok()?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderMap, HeaderValue};

    #[test]
    fn update_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4999"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1700000000"));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), Some(5000));
        assert_eq!(state.load_remaining(), Some(4999));
        assert_eq!(state.load_reset(), Some(1_700_000_000));
        assert!(!state.is_near_limit());
    }

    #[test]
    fn near_limit_detection() {
        let state = RateLimitState::default();
        state.remaining.store(50, Ordering::Relaxed);
        assert!(state.is_near_limit());

        state.remaining.store(100, Ordering::Relaxed);
        assert!(!state.is_near_limit());
    }

    #[test]
    fn out_of_range_header_ignored() {
        let mut headers = HeaderMap::new();
        // 5_000_000_000 exceeds u32::MAX — must be rejected, not silently truncated.
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000000000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4999"));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), None);
        assert_eq!(state.load_remaining(), Some(4999));
    }

    #[test]
    fn u32_max_boundary_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("4294967295"));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        // u32::MAX is a valid limit value (even though it collides with the
        // remaining sentinel — limit uses 0 as its sentinel, not u32::MAX).
        assert_eq!(state.load_limit(), Some(u32::MAX));
    }

    #[test]
    fn u32_max_plus_one_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("4294967296"));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), None);
    }

    #[test]
    fn empty_header_value_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static(""));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), None);
    }

    #[test]
    fn non_numeric_header_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("abc"));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), None);
    }

    #[test]
    fn partial_headers_accepted() {
        let mut headers = HeaderMap::new();
        // Only remaining is set — limit and reset should stay None.
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("42"));

        let state = RateLimitState::default();
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), None);
        assert_eq!(state.load_remaining(), Some(42));
        assert_eq!(state.load_reset(), None);
    }

    #[test]
    fn sentinel_round_trip() {
        let state = RateLimitState::default();
        // Before any update, all fields report None.
        assert_eq!(state.load_limit(), None);
        assert_eq!(state.load_remaining(), None);
        assert_eq!(state.load_reset(), None);

        // Update with values.
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4999"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1700000000"));
        state.update_from_headers(&headers);

        assert_eq!(state.load_limit(), Some(5000));
        assert_eq!(state.load_remaining(), Some(4999));
        assert_eq!(state.load_reset(), Some(1_700_000_000));
    }

    #[test]
    fn is_near_limit_with_unknown_remaining() {
        // When remaining is unknown (sentinel), is_near_limit should be false.
        let state = RateLimitState::default();
        assert!(!state.is_near_limit());
    }

    #[test]
    fn should_halt_below_threshold() {
        let state = RateLimitState::default();
        state.remaining.store(49, Ordering::Relaxed);
        assert!(state.should_halt());
    }

    #[test]
    fn should_halt_at_threshold_returns_false() {
        let state = RateLimitState::default();
        state.remaining.store(HALT_THRESHOLD, Ordering::Relaxed);
        assert!(!state.should_halt());
    }

    #[test]
    fn should_halt_unknown_remaining_returns_false() {
        let state = RateLimitState::default();
        assert!(!state.should_halt());
    }

    #[test]
    fn should_halt_at_zero() {
        let state = RateLimitState::default();
        state.remaining.store(0, Ordering::Relaxed);
        assert!(state.should_halt());
    }

    #[test]
    fn warned_near_limit_resets_on_recovery() {
        let state = RateLimitState::default();

        // Drop below warn threshold — should set warned flag.
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("50"));
        state.update_from_headers(&headers);
        assert!(state.warned_near_limit.load(Ordering::Relaxed));

        // Remaining climbs back above threshold — flag should reset.
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("200"));
        state.update_from_headers(&headers);
        assert!(!state.warned_near_limit.load(Ordering::Relaxed));

        // Drop again — should warn again (flag was reset).
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("30"));
        state.update_from_headers(&headers);
        assert!(state.warned_near_limit.load(Ordering::Relaxed));
    }

    #[test]
    fn custom_thresholds() {
        let state = RateLimitState::with_thresholds(10, 50);
        state.remaining.store(40, Ordering::Relaxed);
        assert!(state.is_near_limit()); // 40 < 50
        assert!(!state.should_halt()); // 40 >= 10

        state.remaining.store(5, Ordering::Relaxed);
        assert!(state.should_halt()); // 5 < 10
    }
}
