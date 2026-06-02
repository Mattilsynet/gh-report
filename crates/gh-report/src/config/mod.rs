//! Configuration and constants for the gh-report application.

pub mod dashboard;
pub mod runtime;

/// Paths checked for a SECURITY.md file, in precedence order.
pub const SECURITY_POLICY_PATHS: &[&str] =
    &["SECURITY.md", ".github/SECURITY.md", "docs/SECURITY.md"];

/// Conforming CODEOWNERS location (`.github/CODEOWNERS`).
pub const CONFORMING_CODEOWNERS_PATH: &str = ".github/CODEOWNERS";

/// Non-conforming CODEOWNERS location (root `CODEOWNERS`).
pub const NON_CONFORMING_CODEOWNERS_PATH: &str = "CODEOWNERS";

/// Current inventory schema version.
pub const INVENTORY_SCHEMA_VERSION: &str = "1.0";

/// Current evidence/checkpoint schema version.
///
/// Bump when metadata fields are added/removed, check field shapes change,
/// or CODEOWNERS conformance semantics change.
pub const EVIDENCE_SCHEMA_VERSION: &str = "15.0";

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
pub const USER_AGENT: &str = concat!("gh-report/", env!("CARGO_PKG_VERSION"));

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

/// Maximum checkpoint file size in bytes (100 MB).
///
/// Prevents OOM when loading a corrupt or unexpectedly large checkpoint file.
pub const MAX_CHECKPOINT_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Default web server bind address (loopback — safe for local development).
///
/// Container and cloud deployments should set `BIND_ADDRESS=0.0.0.0` to
/// accept traffic on all interfaces.
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";

/// Fixed interval between collection runs (seconds). Timer starts after
/// the previous collection completes.
pub const COLLECTION_INTERVAL_SECS: u64 = 10_800; // 3 hours

/// Maximum API calls per budget epoch before pausing.
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
pub const MAX_WEBHOOK_BODY_BYTES: usize = 1_024 * 1024; // 1 MB

/// Replay protection cache capacity.
pub const REPLAY_CACHE_CAPACITY: u64 = 100_000;

/// Replay protection cache TTL (seconds).
pub const REPLAY_CACHE_TTL_SECS: u64 = 3_600; // 1 hour

/// Maximum time to wait for a sweep batch to drain before declaring
/// timeout failure (seconds). The saga emits `SweepFailed` if exceeded.
pub const SWEEP_TIMEOUT_SECS: u64 = 7_200; // 2 hours
