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
pub const EVIDENCE_SCHEMA_VERSION: &str = "24.0";

/// Schema-major token embedded in `JetStream` stream identity so a
/// schema bump provisions fresh, coexisting streams and leaves prior
/// streams untouched. Must equal `"v" + major(EVIDENCE_SCHEMA_VERSION)`;
/// a unit test enforces that relationship.
pub const EVIDENCE_SCHEMA_MAJOR: &str = "v24";

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
/// Hourly REST replenishment: ~4,079 calls/wave against ~5,000/hour.
/// Previous 900s demanded ~16,316/hour (3.26x quota), exhausting budget and
/// pausing for [`API_BUDGET_WAIT_SECS`]. Hourly waves use 81.6%, leaving ~921 calls.
///
/// Trade-off (FLO-0012:R1): staleness rises 15→60 minutes; `CollectionRunStale`
/// (2x interval, `report::view_model`) detects wedging at 2h, formerly 30min.
/// Shorter periods cause longer quota stalls; materially below ~2,500 calls/wave
/// would permit 1800s again.
///
/// Frequency is cut, information is not: every repository, control and
/// field is still collected on every tick.
pub const COLLECTION_INTERVAL_SECS: u64 = 3_600;

/// Interval (seconds) between collection retries after an incomplete or failed
/// collection run.
///
/// Harmonic with [`COLLECTION_INTERVAL_SECS`] per FLO-0002:R2:
/// `3_600 / 60 = 60`.
pub const COLLECTION_RETRY_INTERVAL_SECS: u64 = 60;

/// Wall-clock baseline reuse bound, even with matching `updated_at`:
/// protection/ruleset changes do not bump it (`infra::baseline::should_reuse`).
/// Forces one evaluation/repo/window; accepted settings-only drift is ≤24h
/// (ghr-4fabk); ghr-d1176f2a would decouple settings collection.
///
/// Cost evidence (ghr-1lyih): 4,079 calls/744 pending repos, 81.6% hourly quota;
/// ~4,854 is a shared epoch, NOT run cost; 771 org repos is environmental.
/// 24h reduces forced waves 6/day→1/day versus 4h; evaluation fan-out follows
/// changed repos, but total cost does NOT: paginated inventory and org alerts
/// precede filtering every tick. Scales with org size/scrape depth.
///
/// FLO-0002:R2: `86_400` / [`COLLECTION_INTERVAL_SECS`] = 24 cycles;
/// cadence retuning changes cycles, not this wall-clock bound.
pub const BASELINE_MAX_AGE_SECS: u64 = 86_400;

/// Daily team-refresh interval, independent of repo cycles/raciness:
/// `TeamStateCaptured` persists on its own cadence (ghr-3fda2878,
/// ghr-b562fe02 §E Phase 3 T1). Membership follows hiring/offboarding, not CI;
/// prior 1800s fetched `T + 1` sets 48/day against 81.6%-consumed quota.
/// FLO-0002:R2: 86400/3600 = 24.
///
/// Coverage unchanged: every CODEOWNERS team's full roster and org-members
/// cross-check remain. Removing `role=maintainer` halved per-team requests to
/// `T`, not fields: `role=all` supplies roles.
/// [`crate::app::daemon`] refreshes at STARTUP, avoiding 24h empty/rehydrated-only
/// rosters after Cloud Run revision. GND-0011:R6: owner-detail pages report
/// roster age derived at render time from persisted `TeamStateCaptured.fetched_at`.
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

/// Hold-down window the partial publisher observes after a render
/// completes, before it will render again.
///
/// Canonical 120-second value (COM-0027:R1/R3/R4); CHE-0068:R3 owns matching
/// semantics, not a second hand-maintained value or call-site literal.
///
/// Trigger: budget-gate epoch-pause hook, NOT `RepoEvaluated`;
/// `crate::app::collect` wires `set_budget_pause_notify`, fired by
/// `cherry_pit_wq::budget` after an evaluated chunk (new information).
///
/// Semantics are leading-edge: a signal arriving while idle renders
/// immediately, and the hold-down is measured from render COMPLETION,
/// so a slow render can never overlap itself. Signals arriving during
/// the hold-down are coalesced into exactly one follow-up render and
/// are never dropped.
///
/// Harmonic with [`COLLECTION_INTERVAL_SECS`] per FLO-0002:R2:
/// 3600 / 120 = 30, an integer. Asserted below.
pub const PARTIAL_RENDER_HOLD_DOWN: std::time::Duration = std::time::Duration::from_secs(120);

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
///
/// This is the production default carried by [`SweepTimeout::default`], not
/// a value the sweep reads directly — the sweep takes its budget from
/// `RuntimeConfig::sweep_timeout` so a test can inject a small one
/// (SEC-0004:R2, clocks and time budgets passed explicitly, never globals).
pub const SWEEP_TIMEOUT_SECS: u64 = 7_200;

/// How long a sweep batch may run before the saga declares timeout failure.
///
/// Zero has no inhabitant here: a zero-second sweep budget can never let any
/// batch drain, so it is a guaranteed-failure configuration rather than a
/// short one. Construct through [`SweepTimeout::new`], or take the
/// production budget from [`SweepTimeout::default`].
///
/// The seconds are held as a `u32`, so conversion to
/// [`jiff::SignedDuration`] is total and needs no fallible narrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepTimeout(u32);

impl Default for SweepTimeout {
    fn default() -> Self {
        Self(
            u32::try_from(SWEEP_TIMEOUT_SECS)
                .expect("SWEEP_TIMEOUT_SECS is a compile-time constant that fits u32"),
        )
    }
}

impl SweepTimeout {
    /// Build a sweep timeout from a whole number of seconds.
    ///
    /// Returns `None` for zero.
    #[must_use]
    pub const fn new(secs: u32) -> Option<Self> {
        if secs == 0 { None } else { Some(Self(secs)) }
    }

    /// The budget in whole seconds.
    #[must_use]
    pub const fn as_secs(self) -> u32 {
        self.0
    }

    /// The budget as a [`std::time::Duration`].
    #[must_use]
    pub const fn as_duration(self) -> std::time::Duration {
        std::time::Duration::from_secs(self.0 as u64)
    }

    /// The budget as a [`jiff::SignedDuration`], for scheduling a fire-at.
    #[must_use]
    pub const fn as_signed_duration(self) -> jiff::SignedDuration {
        jiff::SignedDuration::from_secs(self.0 as i64)
    }

    /// The elapsed-milliseconds figure recorded on a timeout payload.
    #[must_use]
    pub const fn elapsed_ms(self) -> u64 {
        (self.0 as u64).saturating_mul(1_000)
    }

    /// The operator-facing message published when this budget is exceeded.
    #[must_use]
    pub fn timed_out_error(self) -> String {
        format!("sweep timed out after {}s", self.0)
    }
}

/// Default maximum distinct repositories admitted per collection sweep.
pub const DEFAULT_MAX_REPOS: usize = 1000;

/// Bound on the number of distinct repositories admitted per collection sweep.
///
/// Guaranteed to be non-zero to ensure at least one repository can be collected.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct MaxRepos(std::num::NonZeroUsize);

impl Default for MaxRepos {
    fn default() -> Self {
        Self(std::num::NonZeroUsize::new(DEFAULT_MAX_REPOS).expect("DEFAULT_MAX_REPOS is non-zero"))
    }
}

impl MaxRepos {
    /// Construct a `MaxRepos` bound from a repository count.
    ///
    /// Returns `None` if `val == 0`.
    #[must_use]
    pub const fn new(val: usize) -> Option<Self> {
        match std::num::NonZeroUsize::new(val) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    /// Return the maximum number of repositories as a `usize`.
    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }
}

impl std::fmt::Display for MaxRepos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for MaxRepos {
    type Err = std::num::ParseIntError;

    /// Parse a repository limit from a string.
    ///
    /// # Errors
    ///
    /// Returns [`std::num::ParseIntError`] if `s` is not a valid non-zero integer.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let n: std::num::NonZeroUsize = s.parse()?;
        Ok(Self(n))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BASELINE_MAX_AGE_SECS, COLLECTION_INTERVAL_SECS, EVIDENCE_SCHEMA_MAJOR,
        EVIDENCE_SCHEMA_VERSION, PARTIAL_RENDER_HOLD_DOWN, SWEEP_TIMEOUT_SECS,
        TEAM_REFRESH_INTERVAL_SECS, USER_AGENT,
    };
    use std::time::Duration;

    #[test]
    fn partial_render_hold_down_is_two_minutes() {
        assert_eq!(PARTIAL_RENDER_HOLD_DOWN, Duration::from_secs(120));
    }

    #[test]
    fn partial_render_hold_down_is_an_integer_divisor_of_the_collection_interval() {
        let hold_down_secs = PARTIAL_RENDER_HOLD_DOWN.as_secs();
        assert_ne!(hold_down_secs, 0);
        assert_eq!(COLLECTION_INTERVAL_SECS % hold_down_secs, 0);
        assert_eq!(COLLECTION_INTERVAL_SECS / hold_down_secs, 30);
    }

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
    fn max_repos_default_is_one_thousand() {
        assert_eq!(super::MaxRepos::default().get(), 1000);
        assert_eq!(super::DEFAULT_MAX_REPOS, 1000);
        assert_eq!(super::MaxRepos::default().get(), super::DEFAULT_MAX_REPOS);
    }

    #[test]
    fn max_repos_accepts_custom_values() {
        let max = super::MaxRepos::new(500).expect("500 is non-zero");
        assert_eq!(max.get(), 500);
        assert_eq!(max.to_string(), "500");
    }

    #[test]
    fn max_repos_rejects_zero() {
        assert!(super::MaxRepos::new(0).is_none());
    }

    #[test]
    fn max_repos_from_str_valid_and_errors() {
        assert_eq!("1000".parse::<super::MaxRepos>().unwrap().get(), 1000);
        assert_eq!("42".parse::<super::MaxRepos>().unwrap().get(), 42);
        assert!("0".parse::<super::MaxRepos>().is_err());
        assert!("invalid".parse::<super::MaxRepos>().is_err());
        assert!("-5".parse::<super::MaxRepos>().is_err());
        assert!("".parse::<super::MaxRepos>().is_err());
    }

    #[test]
    fn max_repos_serde_roundtrip() {
        let original = super::MaxRepos::new(42).unwrap();
        let json = serde_json::to_string(&original).expect("serialize");
        assert_eq!(json, "42");
        let deserialized: super::MaxRepos = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized, original);

        assert!(serde_json::from_str::<super::MaxRepos>("0").is_err());
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
    fn collection_retry_interval_is_sixty_seconds_harmonic_with_collection_interval() {
        assert_eq!(super::COLLECTION_RETRY_INTERVAL_SECS, 60);
        assert_eq!(
            COLLECTION_INTERVAL_SECS % super::COLLECTION_RETRY_INTERVAL_SECS,
            0
        );
        assert_eq!(
            COLLECTION_INTERVAL_SECS / super::COLLECTION_RETRY_INTERVAL_SECS,
            60
        );
    }

    #[test]
    fn baseline_max_age_is_twenty_four_hours() {
        assert_eq!(BASELINE_MAX_AGE_SECS, 86_400);
        assert_eq!(
            Duration::from_secs(BASELINE_MAX_AGE_SECS),
            Duration::from_hours(24)
        );
    }

    #[test]
    fn baseline_max_age_is_an_integer_multiple_of_the_collection_interval() {
        assert_eq!(BASELINE_MAX_AGE_SECS % COLLECTION_INTERVAL_SECS, 0);
        assert_ne!(BASELINE_MAX_AGE_SECS / COLLECTION_INTERVAL_SECS, 0);
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

    #[test]
    fn sweep_timeout_default_is_the_two_hour_production_budget() {
        assert_eq!(super::SweepTimeout::default().as_secs(), 7_200);
        assert_eq!(
            u64::from(super::SweepTimeout::default().as_secs()),
            SWEEP_TIMEOUT_SECS
        );
    }

    #[test]
    fn sweep_timeout_has_no_zero_inhabitant() {
        assert!(super::SweepTimeout::new(0).is_none());
    }

    #[test]
    fn sweep_timeout_carries_a_test_sized_budget() {
        let timeout = super::SweepTimeout::new(1).expect("1s is a valid sweep timeout");
        assert_eq!(timeout.as_secs(), 1);
        assert_eq!(timeout.as_duration(), Duration::from_secs(1));
    }

    #[test]
    fn sweep_timeout_error_message_is_derived_from_the_injected_budget() {
        let timeout = super::SweepTimeout::new(1).expect("1s is a valid sweep timeout");
        assert_eq!(timeout.timed_out_error(), "sweep timed out after 1s");
        assert_eq!(
            super::SweepTimeout::default().timed_out_error(),
            "sweep timed out after 7200s"
        );
    }

    #[test]
    fn sweep_timeout_signed_duration_is_total_over_every_inhabitant() {
        for secs in [1_u32, 7_200, u32::MAX] {
            let timeout = super::SweepTimeout::new(secs).expect("non-zero is constructible");
            assert_eq!(
                timeout.as_signed_duration(),
                jiff::SignedDuration::from_secs(i64::from(secs))
            );
        }
    }
}
