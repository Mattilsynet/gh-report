//! Configuration and constants for the gh-report application.

pub mod dashboard;
pub mod org;
pub mod runtime;

/// Paths checked for a SECURITY.md file, in precedence order.
pub const SECURITY_POLICY_PATHS: &[&str] =
    &["SECURITY.md", ".github/SECURITY.md", "docs/SECURITY.md"];

/// Conforming CODEOWNERS location (`.github/CODEOWNERS`).
pub const CONFORMING_CODEOWNERS_PATH: &str = ".github/CODEOWNERS";

/// Non-conforming CODEOWNERS location (root `CODEOWNERS`).
pub const NON_CONFORMING_CODEOWNERS_PATH: &str = "CODEOWNERS";

/// Non-conforming CODEOWNERS location (`docs/CODEOWNERS`), GitHub's third
/// search location alongside `.github/` and root. Classified the same as
/// [`NON_CONFORMING_CODEOWNERS_PATH`] — no new [`CodeownersStatus`] variant.
///
/// [`CodeownersStatus`]: crate::domain::checks::CodeownersStatus
pub const DOCS_CODEOWNERS_PATH: &str = "docs/CODEOWNERS";

/// Current inventory schema version.
pub const INVENTORY_SCHEMA_VERSION: &str = "1.0";

/// Current evidence/checkpoint schema version.
///
/// Bump when metadata fields are added/removed, check field shapes change,
/// CODEOWNERS conformance semantics change, or a check's computation
/// changes in a way that makes prior projection evidence incomparable to
/// new output. OPERATIONS.md § Scoring Contract → Stability and § Schema
/// Versions → When to bump are the prose authority for this rule; this
/// constant is the value authority — keep both in sync (COM-0027).
pub const EVIDENCE_SCHEMA_VERSION: &str = "19.0";

/// Schema-major token embedded in `JetStream` stream identity so a
/// schema bump provisions fresh, coexisting streams and leaves prior
/// streams untouched. Must equal `"v" + major(EVIDENCE_SCHEMA_VERSION)`;
/// a unit test enforces that relationship.
pub const EVIDENCE_SCHEMA_MAJOR: &str = "v19";

/// Default page size for GitHub API list endpoints.
pub const DEFAULT_PAGE_SIZE: u32 = 100;

/// Default maximum concurrent workers.
pub const DEFAULT_MAX_WORKERS: usize = 16;

/// Minimum concurrent workers.
pub const MIN_WORKERS: usize = 2;

/// Default GitHub API base URL.
pub const DEFAULT_GITHUB_API_BASE_URL: &str = "https://api.github.com";

/// Default GitHub web base URL for constructing repository links.
///
/// Used by the report renderer to build clickable links back to repositories
/// (e.g., `https://github.com/{org}/{repo}`).
pub const DEFAULT_GITHUB_WEB_BASE_URL: &str = "https://github.com";

/// GitHub API version header value.
pub const GITHUB_API_VERSION: &str = "2022-11-28";

/// User-Agent string for API requests.
pub const USER_AGENT: &str = concat!("gh-report/", env!("GH_REPORT_VERSION"));

/// Default HTTP connect timeout in seconds.
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Default HTTP request timeout in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Maximum retry attempts for retryable failures.
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// Maximum pages to follow during pagination (SSRF / OOM protection).
pub const MAX_PAGINATION_PAGES: usize = 500;

/// Maximum concurrent workers upper bound.
pub const MAX_WORKERS: usize = 128;

/// Maximum recursion depth for fnmatch pattern matching (`ReDoS` protection).
///
/// Bounds the recursive wildcard expansion in `collector::ref_matching` to
/// prevent CPU exhaustion from adversarial patterns (e.g., deeply nested `**`
/// or repeated `*`). 256 is sufficient for any realistic branch name pattern
/// while limiting worst-case stack depth. GitHub branch names are naturally
/// bounded to ~256 characters.
pub const FNMATCH_MAX_RECURSION_DEPTH: usize = 256;

/// Maximum response body size in bytes per API response (50 MB).
///
/// Prevents OOM from unexpectedly large responses. Applied via streaming
/// reads that abort early when the limit is exceeded.
pub const MAX_RESPONSE_BODY_BYTES: usize = 50 * 1024 * 1024;

/// Maximum cumulative items across all pages of a paginated response.
///
/// Combined with `MAX_PAGINATION_PAGES`, this bounds total memory usage
/// from paginated API calls.
pub const MAX_PAGINATED_ITEMS: usize = 500_000;

/// Default web server bind address (loopback — safe for local development).
///
/// Container and cloud deployments should set `BIND_ADDRESS=0.0.0.0` to
/// accept traffic on all interfaces.
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";

/// Fixed interval between collection runs (seconds). Timer starts after
/// the previous collection completes.
///
/// One hour, aligned to GitHub's hourly REST quota replenishment. A full
/// refresh wave costs ~4,079 calls against ~5,000 replenished per hour.
/// At the previous 900s cadence that demanded up to four waves per hour
/// (~16,316 calls/h), 3.26x more than the quota supplies — so the
/// collector necessarily exhausted its budget and stalled for
/// [`API_BUDGET_WAIT_SECS`]; the observed exhaust-then-pause cycle was
/// the arithmetic consequence of the cadence, not a fault. At 3600s one
/// wave consumes 81.6% of an hour's quota, leaving ~921 calls of
/// headroom.
///
/// The cost of a longer period (FLO-0012:R1 — both sides, not one): peak
/// staleness of a report rises from 15 to 60 minutes, and the
/// `CollectionRunStale` red flag (derived as 2x this interval in
/// `report::view_model`) now takes 2h rather than 30min to surface a
/// wedged daemon. The cost of a shorter period is quota exhaustion and
/// the stall above, which suppresses collection far more than the
/// cadence nominally schedules. The observation that would shift this
/// optimum is the per-wave call count: materially below ~2,500 calls
/// would make a 1800s cadence affordable again.
///
/// Frequency is cut, information is not: every repository, control and
/// field is still collected on every tick.
pub const COLLECTION_INTERVAL_SECS: u64 = 3_600;

/// Maximum age (seconds) a baseline entry may be reused for, even when
/// the repository's `updated_at` still matches the baseline's recorded
/// value.
///
/// GitHub does not bump a repository's `updated_at` when its
/// branch-protection rules change, so an `updated_at` match alone cannot
/// prove a cached branch-protection verdict is still correct
/// (`infra::baseline::should_reuse`). This bound forces periodic
/// re-collection regardless of `updated_at`, at the known cost of one
/// extra evaluation per repository per window.
///
/// The bound is wall-clock, not cycle-count: it caps how long a stale
/// branch-protection verdict may be served, independent of how often the
/// collector ticks. 4 hours is a risk-based choice, not the
/// previously-used 24h figure: a full day between forced re-checks is
/// too permissive for a reporting-integrity control whose entire purpose
/// is bounding that staleness window (adr-fmt-glprg review round 2). At
/// the current [`COLLECTION_INTERVAL_SECS`] it spans 4 collection
/// cycles, so an unchanged repository still avoids re-collection on 3 of
/// every 4 sweeps — most of the quota saving baseline reuse exists for —
/// while the worst-case staleness window stays well within a single
/// working day. Retuning the collection cadence changes the cycle count
/// but not this bound, which is why the value is unchanged.
pub const BASELINE_MAX_AGE_SECS: u64 = 14_400;

/// Fixed interval between team-refresh collector ticks (seconds),
/// deliberately decoupled from [`COLLECTION_INTERVAL_SECS`] (ghr-3fda2878,
/// roadmap ghr-b562fe02 §E Phase 3 T1: decoupled/eventual default). This
/// severs the repo-snapshot↔roster-fetch coupling that caused
/// unresolved-by-timing raciness: the team-refresh writer persists
/// `TeamStateCaptured` on its own cadence, independent of whether a repo
/// collect cycle is in flight.
///
/// 24 hours. Team membership changes on a human hiring/offboarding
/// timescale, not a CI timescale, so a daily roster sweep is the
/// business-appropriate period; the previous 1800s spent a full
/// `T + 1` fetch set (T = CODEOWNERS-referenced teams) 48 times a day
/// against a quota [`COLLECTION_INTERVAL_SECS`] already consumes 81.6%
/// of. FLO-0002:R2 harmonicity holds: 86400/3600 = 24, an integer
/// number of collection cycles.
///
/// A longer PERIOD must not become a LOSS OF INFORMATION. Two things
/// keep it from being one, and both are load-bearing:
///
/// - Every tick still fetches every CODEOWNERS-referenced team's full
///   roster and the same org-members cross-check. The per-team cost
///   halved from two paginated fetch sets to `T` when the redundant
///   `role=maintainer` fetch was deleted, but that removed a REQUEST,
///   not a field: role now comes from the `role=all` response, which
///   always carried it. Frequency is cut and cost is cut; coverage is
///   not.
/// - [`crate::app::daemon`] runs one refresh at STARTUP before entering
///   this period. Without it a Cloud Run revision would serve an empty
///   or rehydrated-only roster for a full 24 hours.
///
/// Per GND-0011:R6 a lag bound is not design intent unless it is
/// observed and reported: the owner-detail page renders each team's
/// roster age, derived at render time from the persisted
/// `TeamStateCaptured.fetched_at`.
pub const TEAM_REFRESH_INTERVAL_SECS: u64 = 86_400;

/// Interval between polls for the lazily-initialised GitHub client while
/// the team-refresh loop's startup tick waits for it to exist.
///
/// The client is created on the first repo collect, so the startup
/// refresh cannot assume one at spawn time. Polling — rather than
/// skipping — is what keeps a not-yet-ready client from silently
/// costing a full [`TEAM_REFRESH_INTERVAL_SECS`] of roster data.
pub const TEAM_REFRESH_CLIENT_POLL_SECS: u64 = 5;

/// Fallback API budget ceiling used only before the first GitHub API
/// response of a fresh process (`RateLimitState::load_remaining()` is
/// `None`). Every subsequent run sizes its ceiling live from the
/// observed `remaining` count minus a 100-call buffer instead — see
/// `crate::app::collect::effective_budget_ceiling`.
pub const API_BUDGET_LIMIT: u64 = 4000;

/// Duration to wait when budget is exhausted (seconds).
pub const API_BUDGET_WAIT_SECS: u64 = 3600;

/// Work queue capacity (max pending jobs). 10x headroom over typical org size.
pub const WORK_QUEUE_CAPACITY: usize = 10_000;

/// Default maximum visible staleness for the partial-render coalescing
/// window, per CHE-0068:R3.
///
/// The partial publisher coalesces `RepoEvaluated`-driven render
/// triggers into at most one render per `PARTIAL_RENDER_MAX_STALENESS`
/// interval. CHE-0068 picks one second as the starting heuristic
/// balancing user-perceived freshness against render and broadcast
/// cost; revisit on load data.
pub const PARTIAL_RENDER_MAX_STALENESS: std::time::Duration = std::time::Duration::from_secs(1);

/// Secret alert age bucket definitions: (label, `min_days`, `max_days`).
///
/// `max_days` of `None` means unbounded.
pub const SECRET_ALERT_AGE_BUCKETS: &[(&str, u64, Option<u64>)] = &[
    ("0_7_days", 0, Some(7)),
    ("8_30_days", 8, Some(30)),
    ("31_90_days", 31, Some(90)),
    ("91_plus_days", 91, None),
];

/// Bucket label for alerts with unparseable creation dates.
pub const SECRET_ALERT_UNKNOWN_AGE_BUCKET: &str = "unknown";

/// Create an empty age-bucket map with all standard labels initialised to
/// `T::default()` (typically `0`).
///
/// Works for both `u32` (metrics summary) and `u64` (org-level collection).
#[must_use]
pub fn empty_age_buckets<T: Default>() -> std::collections::HashMap<String, T> {
    let mut buckets = std::collections::HashMap::with_capacity(SECRET_ALERT_AGE_BUCKETS.len() + 1);
    for &(label, _, _) in SECRET_ALERT_AGE_BUCKETS {
        buckets.insert(label.to_string(), T::default());
    }
    buckets.insert(SECRET_ALERT_UNKNOWN_AGE_BUCKET.to_string(), T::default());
    buckets
}

/// TTL for cross-run repository detail cache entries (hours).
pub const REPO_CACHE_TTL_HOURS: u64 = 24;

/// Default webhook debounce window (seconds).
pub const DEFAULT_WEBHOOK_DEBOUNCE_SECS: u64 = 5;

/// Maximum webhook request body size (bytes).
pub const MAX_WEBHOOK_BODY_BYTES: usize = 1_024 * 1024;

/// Replay protection cache capacity.
pub const REPLAY_CACHE_CAPACITY: u64 = 100_000;

/// Replay protection cache TTL (seconds).
pub const REPLAY_CACHE_TTL_SECS: u64 = 3_600;

/// Maximum time to wait for a sweep batch to drain before declaring
/// timeout failure (seconds). The saga emits `SweepFailed` if exceeded.
pub const SWEEP_TIMEOUT_SECS: u64 = 7_200;

#[cfg(test)]
mod tests {
    use super::{
        BASELINE_MAX_AGE_SECS, COLLECTION_INTERVAL_SECS, EVIDENCE_SCHEMA_MAJOR,
        EVIDENCE_SCHEMA_VERSION, TEAM_REFRESH_INTERVAL_SECS, USER_AGENT,
    };
    use std::time::Duration;

    #[test]
    fn team_refresh_interval_is_twenty_four_hours() {
        assert_eq!(TEAM_REFRESH_INTERVAL_SECS, 86_400);
        assert_eq!(
            Duration::from_secs(TEAM_REFRESH_INTERVAL_SECS),
            Duration::from_hours(24)
        );
    }

    #[test]
    fn team_refresh_interval_is_an_integer_multiple_of_the_collection_interval() {
        assert_eq!(TEAM_REFRESH_INTERVAL_SECS % COLLECTION_INTERVAL_SECS, 0);
        assert_eq!(TEAM_REFRESH_INTERVAL_SECS / COLLECTION_INTERVAL_SECS, 24);
    }

    #[test]
    fn collection_interval_is_one_hour_aligned_to_quota_replenishment() {
        assert_eq!(COLLECTION_INTERVAL_SECS, 3_600);
        assert_eq!(
            Duration::from_secs(COLLECTION_INTERVAL_SECS),
            Duration::from_hours(1)
        );
    }

    #[test]
    fn baseline_max_age_is_four_hours_and_an_integer_multiple_of_the_collection_interval() {
        assert_eq!(BASELINE_MAX_AGE_SECS, 14_400);
        assert_eq!(BASELINE_MAX_AGE_SECS % COLLECTION_INTERVAL_SECS, 0);
        assert_eq!(BASELINE_MAX_AGE_SECS / COLLECTION_INTERVAL_SECS, 4);
    }

    #[test]
    fn gh_report_version_is_non_empty() {
        assert!(!env!("GH_REPORT_VERSION").is_empty());
    }

    #[test]
    fn schema_major_tracks_evidence_schema_version_major() {
        let major = EVIDENCE_SCHEMA_VERSION
            .split('.')
            .next()
            .expect("schema version has a major component");
        assert_eq!(EVIDENCE_SCHEMA_MAJOR, format!("v{major}"));
    }

    #[test]
    fn user_agent_interpolates_build_stamped_version() {
        assert_eq!(USER_AGENT, concat!("gh-report/", env!("GH_REPORT_VERSION")));
    }
}
