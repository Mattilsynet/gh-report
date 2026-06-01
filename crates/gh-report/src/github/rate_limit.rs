//! GitHub-specific rate-limit policy and header adapter.
//!
//! Wraps the generic [`cherry_pit_wq::RateLimitState`] observer with
//! GitHub's `x-ratelimit-*` REST API conventions and the thresholds
//! gh-report uses to drive worker-pool halts. Per CHE-0055 G5 these
//! GitHub-shaped concerns live here, not in `cherry-pit-wq`.

use http::HeaderMap;

pub use cherry_pit_wq::{RateLimitObservation, RateLimitState};

/// Hard halt threshold. Collection stops when `remaining` drops below this.
pub const HALT_THRESHOLD: u32 = 50;

/// Advisory warning threshold. A log warning is emitted when `remaining`
/// drops below this value.
pub const WARN_THRESHOLD: u32 = 100;

/// Construct a [`RateLimitState`] configured with gh-report's GitHub
/// REST defaults ([`HALT_THRESHOLD`] / [`WARN_THRESHOLD`]).
#[must_use]
pub fn new_default() -> RateLimitState {
    RateLimitState::with_thresholds(HALT_THRESHOLD, WARN_THRESHOLD)
}

/// Update `state` from the `x-ratelimit-*` headers on a GitHub REST
/// response. Missing or malformed headers leave the corresponding
/// field unchanged.
pub fn update_from_headers(state: &RateLimitState, headers: &HeaderMap) {
    state.observe(RateLimitObservation {
        limit: parse_header::<u32>(headers, "x-ratelimit-limit"),
        remaining: parse_header::<u32>(headers, "x-ratelimit-remaining"),
        reset: parse_header::<u64>(headers, "x-ratelimit-reset"),
    });
}

fn parse_header<T: std::str::FromStr>(headers: &HeaderMap, name: &str) -> Option<T> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::{HeaderMap, HeaderValue};

    #[test]
    fn update_from_headers_populates_state() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4999"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1700000000"));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), Some(5000));
        assert_eq!(state.load_remaining(), Some(4999));
        assert_eq!(state.load_reset(), Some(1_700_000_000));
        assert!(!state.is_near_limit());
    }

    #[test]
    fn near_limit_uses_default_thresholds() {
        let state = new_default();
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("50"));
        update_from_headers(&state, &headers);
        assert!(state.is_near_limit());

        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("100"));
        update_from_headers(&state, &headers);
        assert!(!state.is_near_limit());
    }

    #[test]
    fn out_of_range_header_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("5000000000"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("4999"));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), None);
        assert_eq!(state.load_remaining(), Some(4999));
    }

    #[test]
    fn u32_max_boundary_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("4294967295"));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), Some(u32::MAX));
    }

    #[test]
    fn u32_max_plus_one_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("4294967296"));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), None);
    }

    #[test]
    fn empty_header_value_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static(""));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), None);
    }

    #[test]
    fn non_numeric_header_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("abc"));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), None);
    }

    #[test]
    fn partial_headers_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("42"));

        let state = new_default();
        update_from_headers(&state, &headers);

        assert_eq!(state.load_limit(), None);
        assert_eq!(state.load_remaining(), Some(42));
        assert_eq!(state.load_reset(), None);
    }

    #[test]
    fn should_halt_uses_default_threshold() {
        let state = new_default();
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("49"));
        update_from_headers(&state, &headers);
        assert!(state.should_halt());
    }
}
