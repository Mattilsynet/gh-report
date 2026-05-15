//! Date/time parsing and staleness utilities for domain logic.
//!
//! Pure functions with no I/O. Used by collectors, metrics aggregation,
//! and report view models to classify repository activity.

use std::borrow::Cow;

use jiff::Timestamp;

/// Staleness threshold: repos with `updated_at` more than this many days
/// before the report timestamp are considered stale.
pub const STALE_THRESHOLD_DAYS: i64 = 730; // ~2 years

/// Dependabot inactivity threshold: repos with `pushed_at` more than this
/// many days before the run timestamp are considered inactive.
///
/// GitHub may auto-pause Dependabot on repositories with no code activity.
/// The API does not reliably surface this state, so we infer it from the
/// `pushed_at` timestamp as a proxy metric.
pub const DEPENDABOT_INACTIVITY_THRESHOLD_DAYS: i64 = 90;

/// Determine whether a repository is stale.
///
/// A repo is stale when it has an `updated_at` timestamp that is more than
/// [`STALE_THRESHOLD_DAYS`] before the `run_timestamp`. Returns `false`
/// when `updated_at` is `None` (unknown ≠ stale).
#[must_use]
pub fn is_repo_stale(updated_at: Option<&str>, run_timestamp: &str) -> bool {
    let Some(updated) = updated_at.and_then(parse_iso8601) else {
        return false;
    };
    let Some(generated) = parse_iso8601(run_timestamp) else {
        return false;
    };
    let age_days = (generated.duration_since(updated).as_secs() / 86_400).max(0);
    age_days > STALE_THRESHOLD_DAYS
}

/// Determine whether a repository is inactive for Dependabot purposes.
///
/// Returns `true` when `pushed_at` is more than
/// [`DEPENDABOT_INACTIVITY_THRESHOLD_DAYS`] before `run_timestamp`.
/// Returns `false` when `pushed_at` is `None` or unparseable (unknown ≠
/// inactive — conservative to avoid false positives).
#[must_use]
pub fn is_dependabot_inactive(pushed_at: Option<&str>, run_timestamp: &str) -> bool {
    let Some(pushed) = pushed_at.and_then(parse_iso8601) else {
        return false;
    };
    let Some(generated) = parse_iso8601(run_timestamp) else {
        return false;
    };
    let age_days = (generated.duration_since(pushed).as_secs() / 86_400).max(0);
    age_days > DEPENDABOT_INACTIVITY_THRESHOLD_DAYS
}

/// Parse an ISO 8601 timestamp, normalizing trailing `Z` to `+00:00`
/// as defense-in-depth (jiff handles both natively).
#[must_use]
pub fn parse_iso8601(value: &str) -> Option<Timestamp> {
    let normalized: Cow<'_, str> = if let Some(stripped) = value.strip_suffix('Z') {
        Cow::Owned(format!("{stripped}+00:00"))
    } else {
        Cow::Borrowed(value)
    };
    normalized.parse::<Timestamp>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_suffix() {
        let ts = parse_iso8601("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(ts.to_string(), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn offset() {
        let ts = parse_iso8601("2026-01-01T02:00:00+02:00").unwrap();
        assert_eq!(ts.to_string(), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn invalid() {
        assert!(parse_iso8601("invalid").is_none());
    }

    #[test]
    fn fractional_seconds() {
        let ts = parse_iso8601("2026-01-15T10:30:00.123Z").unwrap();
        assert_eq!(
            ts.strftime("%Y-%m-%dT%H:%M:%S").to_string(),
            "2026-01-15T10:30:00"
        );
    }

    #[test]
    fn negative_offset() {
        let ts = parse_iso8601("2026-01-15T07:30:00-05:00").unwrap();
        assert_eq!(ts.to_string(), "2026-01-15T12:30:00Z");
    }

    #[test]
    fn empty_string() {
        assert!(parse_iso8601("").is_none());
    }

    #[test]
    fn space_separator_accepted() {
        let ts = parse_iso8601("2026-04-12 14:30:05+00:00").unwrap();
        assert_eq!(ts.to_string(), "2026-04-12T14:30:05Z");
    }

    // ── is_repo_stale tests ────────────────────────────────────

    #[test]
    fn is_repo_stale_none_updated_at_returns_false() {
        assert!(!is_repo_stale(None, "2026-04-09T12:00:00+00:00"));
    }

    #[test]
    fn is_repo_stale_recent_repo_returns_false() {
        assert!(!is_repo_stale(
            Some("2025-04-09T12:00:00+00:00"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    #[test]
    fn is_repo_stale_old_repo_returns_true() {
        assert!(is_repo_stale(
            Some("2023-04-09T12:00:00+00:00"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    #[test]
    fn is_repo_stale_exactly_at_threshold_returns_false() {
        assert!(!is_repo_stale(
            Some("2024-04-09T12:00:00+00:00"),
            "2026-04-08T12:00:00+00:00",
        ));
    }

    #[test]
    fn is_repo_stale_just_past_threshold_returns_true() {
        assert!(is_repo_stale(
            Some("2024-04-09T12:00:00+00:00"),
            "2026-04-10T12:00:00+00:00",
        ));
    }

    #[test]
    fn is_repo_stale_subday_at_threshold_returns_false() {
        assert!(!is_repo_stale(
            Some("2024-04-10T00:00:00+00:00"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    #[test]
    fn is_repo_stale_invalid_run_timestamp_returns_false() {
        assert!(!is_repo_stale(
            Some("2023-04-09T12:00:00+00:00"),
            "not-a-date",
        ));
    }

    #[test]
    fn is_repo_stale_invalid_updated_at_returns_false() {
        assert!(!is_repo_stale(
            Some("not-a-date"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    // ── is_dependabot_inactive tests ───────────────────────────

    #[test]
    fn dependabot_inactive_none_pushed_at_returns_false() {
        assert!(!is_dependabot_inactive(None, "2026-04-09T12:00:00+00:00"));
    }

    #[test]
    fn dependabot_inactive_recent_push_returns_false() {
        assert!(!is_dependabot_inactive(
            Some("2026-03-10T12:00:00+00:00"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    #[test]
    fn dependabot_inactive_old_push_returns_true() {
        assert!(is_dependabot_inactive(
            Some("2025-12-11T12:00:00+00:00"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    #[test]
    fn dependabot_inactive_exactly_at_threshold_returns_false() {
        assert!(!is_dependabot_inactive(
            Some("2026-01-09T12:00:00+00:00"),
            "2026-04-09T12:00:00+00:00",
        ));
    }

    #[test]
    fn dependabot_inactive_invalid_pushed_at_returns_false() {
        assert!(!is_dependabot_inactive(
            Some("not-a-date"),
            "2026-04-09T12:00:00+00:00",
        ));
    }
}
