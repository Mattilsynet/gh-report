//! GitHub-specific rate-limit policy and header adapter.
//!
//! Wraps the generic [`cherry_pit_wq::RateLimitState`] observer with
//! GitHub's `x-ratelimit-*` REST API conventions and the thresholds
//! gh-report uses to drive worker-pool halts. Per CHE-0055 G5 these
//! GitHub-shaped concerns live here, not in `cherry-pit-wq`.
//!
//! [`secondary_limit_resume_at`] parses the secondary-rate-limit /
//! abuse-detection signal (429, or 403 with GitHub's secondary-limit body
//! marker) plus a `Retry-After` header into a resume-at [`Instant`], per
//! adr-fmt-egsrk / CHE-0046 inheritance. This is a DISTINCT path from
//! `update_from_headers`'s primary-limit (`x-ratelimit-remaining:0`)
//! tracking above — not a duplicate of it.

use std::time::{Duration, Instant, SystemTime};

use cherry_pit_wq::RateLimitObservation;
use http::HeaderMap;

pub use cherry_pit_wq::RateLimitState;

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

/// GitHub's abuse-detection body marker for a secondary-rate-limit 403
/// (case-insensitive substring match against the JSON `message` field, e.g.
/// `"You have exceeded a secondary rate limit. Please wait ..."`).
const SECONDARY_LIMIT_BODY_MARKER: &str = "secondary rate limit";

/// Parse a server-authoritative resume-at instant from a secondary-limit
/// signal: a 429 response, or a 403 whose body contains GitHub's
/// secondary-rate-limit marker (distinguishing it from a plain
/// 403-permission-denied, which returns `None` here and is handled as a
/// terminal, non-retryable failure per CHE-0046:R2).
///
/// Returns `None` when the status is not a secondary-limit signal, or when
/// no `Retry-After` header is present (CHE-0046 fallback: the caller falls
/// back to jittered-exponential backoff bounded by its own deadline — this
/// function does not invent a wait when the server did not specify one).
///
/// Does not duplicate the primary-limit path
/// (`x-ratelimit-remaining:0`/`x-ratelimit-reset`), which is handled by
/// [`update_from_headers`] feeding `RateLimitState`/`halted_until`.
#[must_use]
pub fn secondary_limit_resume_at(status: u16, headers: &HeaderMap, body: &str) -> Option<Instant> {
    let is_secondary_signal = status == 429
        || (status == 403
            && body
                .to_ascii_lowercase()
                .contains(SECONDARY_LIMIT_BODY_MARKER));
    if !is_secondary_signal {
        return None;
    }
    parse_retry_after(headers)
}

/// Parse a `Retry-After` header value (seconds-integer or HTTP-date) into a
/// resume-at [`Instant`], rounding UP so the computed instant never
/// under-honors the server's wait (the outage-class safety rule: never
/// shorten a server-authoritative wait).
///
/// Returns `None` both when the header is absent and when it is present but
/// unparseable; the latter case logs a warning (CHE-0046 fallback still
/// applies, but a malformed value on a genuine secondary-limit signal should
/// not fail open silently).
fn parse_retry_after(headers: &HeaderMap) -> Option<Instant> {
    let raw = headers.get("retry-after")?.to_str().ok()?.trim();

    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Instant::now() + Duration::from_secs(secs));
    }

    if let Ok(target) = httpdate::parse_http_date(raw) {
        let now = SystemTime::now();
        let wait = target.duration_since(now).unwrap_or(Duration::ZERO);
        let rounded_secs = ceil_to_whole_secs(wait);
        return Some(Instant::now() + Duration::from_secs(rounded_secs));
    }

    tracing::warn!(
        retry_after = raw,
        "Retry-After header present but unparseable as seconds or HTTP-date; \
         falling back to CHE-0046 jittered-exponential backoff"
    );
    None
}

/// Round `d` up to the next whole second (a partial second still counts as
/// a full second), so a `Retry-After` wait derived from `d` is never
/// under-honored by sub-second truncation.
fn ceil_to_whole_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
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

    #[test]
    fn secondary_limit_resume_at_parses_429_with_seconds_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("30"));

        let before = Instant::now();
        let resume_at = secondary_limit_resume_at(429, &headers, "").expect("429 is a signal");
        assert!(
            resume_at >= before + Duration::from_secs(30),
            "resume_at must be at least now+30s, never shortened"
        );
        assert!(
            resume_at < before + Duration::from_secs(31),
            "resume_at must not massively overshoot the server value either"
        );
    }

    #[test]
    fn secondary_limit_resume_at_parses_403_secondary_body_marker() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("5"));
        let body = r#"{"message":"You have exceeded a secondary rate limit. Please wait."}"#;

        let before = Instant::now();
        let resume_at =
            secondary_limit_resume_at(403, &headers, body).expect("secondary-limit 403 body");
        assert!(resume_at >= before + Duration::from_secs(5));
    }

    #[test]
    fn secondary_limit_resume_at_none_for_plain_403_permission_denied() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("5"));
        let body = r#"{"message":"Must have admin rights to Repository."}"#;

        assert_eq!(
            secondary_limit_resume_at(403, &headers, body),
            None,
            "a plain 403 permission-denied body must not be treated as a secondary-limit signal"
        );
    }

    #[test]
    fn secondary_limit_resume_at_none_without_retry_after_header() {
        let headers = HeaderMap::new();
        assert_eq!(
            secondary_limit_resume_at(429, &headers, ""),
            None,
            "absent Retry-After falls back to CHE-0046 jittered-exponential at the call site"
        );
    }

    #[test]
    fn secondary_limit_resume_at_none_for_unrelated_status() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("30"));
        assert_eq!(secondary_limit_resume_at(500, &headers, ""), None);
    }

    #[test]
    fn secondary_limit_resume_at_none_for_malformed_retry_after_falls_back_open() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "retry-after",
            HeaderValue::from_static("not-a-duration-or-date"),
        );
        assert_eq!(
            secondary_limit_resume_at(429, &headers, ""),
            None,
            "an unparseable Retry-After must fail open to CHE-0046 fallback, not panic \
             or silently invent a wait"
        );
    }

    #[test]
    fn secondary_limit_resume_at_parses_http_date_retry_after_rounded_up() {
        let target = SystemTime::now() + Duration::from_millis(2500);
        let http_date = httpdate::fmt_http_date(target);
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_str(&http_date).unwrap());

        let before = Instant::now();
        let resume_at = secondary_limit_resume_at(429, &headers, "").expect("HTTP-date parses");
        assert!(
            resume_at >= before + Duration::from_secs(2),
            "HTTP-date wait must round up, never truncate below the server's instant"
        );
    }
}
