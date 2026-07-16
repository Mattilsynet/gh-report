//! GitHub REST API client with connection pooling, retry, rate-limit handling,
//! credential refresh, and startup capability probes.
//!
//! # Thread safety
//!
//! `GitHubClient` uses interior mutability for all mutable state:
//! - `ArcSwap<HeaderValue>` for lock-free per-request auth header reads
//! - `tokio::sync::Mutex<GitHubCredential>` for credential refresh (~1/hour)
//! - `scc::HashMap` for concurrent repo detail cache and `ETag` tracking
//! - Atomics for counters, rate limit state, and time-bounded halt (`halted_until`)
//!
//! All public methods take `&self`. Safe for sharing via `Arc<GitHubClient>`
//! across concurrent `tokio::spawn` tasks.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use jiff::{SignedDuration, Timestamp};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
use scc::HashMap as SccHashMap;
use secrecy::ExposeSecret;
use zeroize::Zeroizing;

use tracing::{debug, error, info, instrument, warn};

use crate::config;
use crate::error::GitHubApiError;
use crate::github::auth::{
    AuthMetadata, CapabilitySet, CapabilityStatus, GitHubAppConfig, GitHubCredential,
    InstallationTokenResponse, generate_app_jwt, parse_oauth_scopes,
};
use crate::github::budget::BudgetGate;
use crate::github::pagination;
use crate::github::rate_limit::RateLimitState;

/// Maximum length of error response body to include in error messages.
/// Prevents potential token echoing in logs.
const MAX_ERROR_BODY_LEN: usize = 1024;

/// Result of a single GitHub API request.
///
/// Re-exported from [`crate::api_outcome`], where the type is defined —
/// it carries no GitHub-specific vocabulary and is generic across HTTP
/// JSON APIs. Kept re-exported here so existing `github::client::ApiOutcome`
/// call sites are unaffected.
pub use crate::api_outcome::ApiOutcome;

/// Truncate an error body to prevent sensitive data from leaking into logs.
///
/// Uses `floor_char_boundary` to ensure we never split a multi-byte UTF-8
/// character, which would cause a panic.
fn truncate_error_body(body: &str) -> String {
    if body.len() <= MAX_ERROR_BODY_LEN {
        body.to_string()
    } else {
        let safe_boundary = body.floor_char_boundary(MAX_ERROR_BODY_LEN);
        format!("{}…[truncated]", &body[..safe_boundary])
    }
}

/// Read a response body using streaming chunks with a size limit.
///
/// Aborts early if the accumulated body exceeds `max_bytes`, preventing OOM
/// from unexpectedly large responses. The advisory `Content-Length` header is
/// checked first as an early exit, but is not trusted — the actual streamed
/// bytes are always counted.
///
/// **Note:** Because the `reqwest` client is configured with `gzip` support,
/// the limit applies to the **decompressed** body size, not the wire bytes.
async fn read_body_limited(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<String, GitHubApiError> {
    if let Some(len) = response.content_length()
        && len > max_bytes as u64
    {
        return Err(GitHubApiError::InvalidResponse {
            reason: format!("response Content-Length ({len}) exceeds {max_bytes} byte limit"),
        });
    }

    let content_len = response.content_length().unwrap_or(0);
    let hint = usize::try_from(content_len)
        .unwrap_or(max_bytes)
        .min(max_bytes);
    let mut body = Vec::with_capacity(hint);
    let mut stream = response;

    while let Some(chunk) = stream
        .chunk()
        .await
        .map_err(|e| GitHubApiError::InvalidResponse {
            reason: format!("body read error: {e}"),
        })?
    {
        if body.len() + chunk.len() > max_bytes {
            return Err(GitHubApiError::InvalidResponse {
                reason: format!("response body exceeds {max_bytes} byte limit"),
            });
        }
        body.extend_from_slice(&chunk);
    }

    String::from_utf8(body).map_err(|e| GitHubApiError::InvalidResponse {
        reason: format!("response body is not valid UTF-8: {e}"),
    })
}

/// GitHub REST API client with connection pooling, retry, memoization,
/// and optional credential refresh for GitHub App tokens.
///
/// All public methods take `&self`. Interior mutability is used for all
/// mutable state, making this type safe for sharing via `Arc<GitHubClient>`:
/// - `auth_header: ArcSwap<HeaderValue>` — lock-free per-request auth injection
/// - `repo_detail_cache: scc::HashMap` — per-run memoization of repo detail responses
/// - `last_response_etags: scc::HashMap` — side-channel for `ETag` capture by `request_single_inner`,
///   consumed by `repo_details` for conditional request support
/// - `rate_limit: RateLimitState` — atomics for rate limit tracking
/// - `halted_until: AtomicU64` — time-bounded halt; auto-clears when rate-limit window passes
/// - `credential: tokio::sync::Mutex` — serialized credential refresh
/// - `auth_metadata: StdMutex` — one-time metadata capture
/// - `budget: BudgetGate` — self-imposed API call limit
pub struct GitHubClient {
    /// HTTP client — built once and never replaced. Connection pool persists
    /// across credential refreshes.
    http: reqwest::Client,
    /// Per-request Authorization header, updated on credential refresh.
    /// Lock-free reads via `ArcSwap` — zero contention on the hot path.
    auth_header: ArcSwap<HeaderValue>,
    base_url: String,
    /// The trusted origin (scheme + host + port) used for pagination URL validation.
    trusted_origin: String,
    pub org_name: String,
    pub rate_limit_warnings: AtomicU32,
    repo_detail_cache: SccHashMap<String, CachedResult>,
    /// Rate limit state, updated from every API response. Shared via `Arc`
    /// to allow the daemon to own rate-limit state across collection runs.
    /// Public so callers can check
    /// `should_halt()` for fail-fast at run start.
    pub rate_limit: Arc<RateLimitState>,
    /// Unix timestamp (seconds since epoch) until which the client is halted.
    /// `0` = not halted. Auto-clears when the rate-limit window passes.
    /// Set via `fetch_max` to ensure concurrent halt triggers never regress
    /// the timestamp backward.
    halted_until: AtomicU64,
    credential: tokio::sync::Mutex<GitHubCredential>,
    /// Single-flight serializer for credential refresh attempts.
    ///
    /// Held across the HTTP token exchange so at most one refresh is in
    /// flight at a time. Distinct from `credential` so the credential data
    /// mutex is never held across an `.await` of `exchange_installation_token`.
    refresh_lock: tokio::sync::Mutex<()>,
    /// GitHub App config, if using App authentication (needed for token refresh).
    app_config: Option<GitHubAppConfig>,
    /// Auth metadata collected from API response headers.
    auth_metadata: StdMutex<Option<AuthMetadata>>,
    /// Mirrors `credential.expires_at` as Unix timestamp for lock-free checking.
    /// 0 = never expires (PAT/gh-cli credential).
    credential_expires_at: AtomicU64,
    /// API call budget gate — self-imposed limit per epoch.
    /// Shared via `Arc` to allow the daemon to own budget state across
    /// collection runs.
    budget: Arc<BudgetGate>,
    budget_cancel: tokio_util::sync::CancellationToken,
    /// Side-channel for `ETag` extraction: maps API path → last-seen `ETag`.
    /// Populated by `request_single_inner`, read by `repo_details`.
    last_response_etags: SccHashMap<String, String>,
}

/// Cached repository detail result. Only successful results are cached.
#[derive(Debug, Clone)]
struct CachedResult {
    status_code: u16,
    data: Option<serde_json::Value>,
    /// `ETag` from the GitHub API response, used for conditional requests.
    etag: Option<String>,
    /// Whether this entry was seeded from cross-run cache and has been
    /// invalidated by `evict_stale_entries`. Stale entries are revalidated
    /// via `ETag` conditional requests before being trusted.
    stale: bool,
}

/// Extract the origin (scheme + host + optional port) from a URL string.
fn extract_origin(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    let scheme = parsed.scheme();
    let host = parsed.host_str()?;
    match parsed.port() {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

/// Validate that a URL belongs to the same origin as the trusted base URL.
fn is_same_origin(url_str: &str, trusted_origin: &str) -> bool {
    match extract_origin(url_str) {
        Some(origin) => origin == trusted_origin,
        None => false,
    }
}

fn trusted_next_url(headers: &HeaderMap, trusted_origin: &str) -> Option<String> {
    let candidate_url = pagination::next_url(headers)?;
    if is_same_origin(&candidate_url, trusted_origin) {
        return Some(candidate_url);
    }
    let sanitized: String = candidate_url
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    warn!(
        url = %sanitized,
        "rejecting pagination URL from untrusted origin"
    );
    None
}

/// Validate that a URL uses HTTPS (or HTTP only if explicitly opted in).
///
/// # Errors
///
/// Returns `GitHubApiError::ClientConfigError` if the URL is invalid, uses an unsupported scheme,
/// or has no host.
pub fn validate_api_base_url(url_str: &str) -> Result<String, GitHubApiError> {
    let parsed = url::Url::parse(url_str).map_err(|e| GitHubApiError::ClientConfigError {
        reason: format!("invalid API base URL '{url_str}': {e}"),
    })?;

    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err(GitHubApiError::ClientConfigError {
            reason: format!(
                "API base URL must use https (or http for local development), got: {}",
                parsed.scheme()
            ),
        });
    }

    if !cfg!(any(debug_assertions, test)) && parsed.scheme() == "http" {
        return Err(GitHubApiError::ClientConfigError {
            reason: "API base URL must use https in release builds".to_string(),
        });
    }

    if parsed.host_str().is_none() {
        return Err(GitHubApiError::ClientConfigError {
            reason: "API base URL must include a host".to_string(),
        });
    }

    Ok(url_str.trim_end_matches('/').to_string())
}

/// Duration buffer before expiry at which we refresh GitHub App tokens.
///
/// Set to 5 minutes to ensure tokens are refreshed well before GitHub's
/// 1-hour installation token lifetime expires, accounting for potential
/// clock drift and in-flight request time.
const TOKEN_REFRESH_BUFFER: Duration = Duration::from_mins(5);

impl GitHubClient {
    /// Create a new GitHub API client.
    ///
    /// The `base_url` must be a valid HTTPS URL. HTTP is allowed for local development
    /// but should not be used in production (tokens would be sent in plaintext).
    ///
    /// `org_name` is validated as a safe path segment to prevent path injection.
    ///
    /// `budget` and `rate_limit` are shared resources — the daemon owns them
    /// and passes them to each collection run. This allows cumulative budget
    /// accounting and rate-limit tracking across runs.
    ///
    /// # Errors
    ///
    /// Returns `GitHubApiError` if the URL is invalid, the org name contains unsafe characters,
    /// or the HTTP client cannot be built.
    pub fn new(
        credential: GitHubCredential,
        base_url: &str,
        org_name: &str,
        app_config: Option<GitHubAppConfig>,
        budget: Arc<BudgetGate>,
        rate_limit: Arc<RateLimitState>,
    ) -> Result<Self, GitHubApiError> {
        let validated_url = validate_api_base_url(base_url)?;
        let trusted_origin =
            extract_origin(&validated_url).ok_or_else(|| GitHubApiError::ClientConfigError {
                reason: format!("cannot extract origin from API base URL: {validated_url}"),
            })?;

        let validated_org = cherry_pit_web::sanitize_path_segment(org_name, "org_name")
            .map(std::borrow::Cow::into_owned)
            .map_err(|e| GitHubApiError::ClientConfigError {
                reason: format!("invalid organization name: {e}"),
            })?;

        let http = Self::build_http_client()?;

        let auth_value = Zeroizing::new(format!("Bearer {}", credential.token.expose_secret()));
        let auth_header_value = HeaderValue::from_str(&auth_value).map_err(|_| {
            GitHubApiError::AuthenticationFailed {
                reason: "invalid token characters".to_string(),
            }
        })?;

        Ok(Self {
            http,
            auth_header: ArcSwap::from_pointee(auth_header_value),
            base_url: validated_url,
            trusted_origin,
            org_name: validated_org,
            rate_limit_warnings: AtomicU32::new(0),
            repo_detail_cache: SccHashMap::new(),
            rate_limit,
            halted_until: AtomicU64::new(0),
            credential_expires_at: AtomicU64::new(credential.expires_at_unix()),
            credential: tokio::sync::Mutex::new(credential),
            refresh_lock: tokio::sync::Mutex::new(()),
            app_config,
            auth_metadata: StdMutex::new(None),
            budget,
            budget_cancel: tokio_util::sync::CancellationToken::new(),
            last_response_etags: SccHashMap::new(),
        })
    }

    /// Build a `reqwest::Client` with default headers (no auth).
    ///
    /// Auth is injected per-request via `auth_header` to preserve the
    /// connection pool across credential refreshes.
    fn build_http_client() -> Result<reqwest::Client, GitHubApiError> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static(config::GITHUB_API_VERSION),
        );

        reqwest::Client::builder()
            .user_agent(config::USER_AGENT)
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(config::DEFAULT_CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(config::DEFAULT_REQUEST_TIMEOUT_SECS))
            .pool_max_idle_per_host(10)
            .build()
            .map_err(GitHubApiError::Http)
    }

    /// Lock-free check of credential expiry for the fast path.
    fn credential_needs_refresh(&self) -> bool {
        let exp = self.credential_expires_at.load(Ordering::Acquire);
        if exp == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now + TOKEN_REFRESH_BUFFER.as_secs() >= exp
    }

    /// Time-bounded halt check. Returns `true` when `halted_until` is in
    /// the future. Auto-clears when the rate-limit window passes.
    ///
    /// Follows the same `SystemTime` → Unix timestamp pattern as
    /// [`credential_needs_refresh`](Self::credential_needs_refresh).
    ///
    /// # Clock assumption
    ///
    /// Uses `SystemTime` (not monotonic `Instant`) because `halted_until`
    /// stores a GitHub-provided Unix timestamp. Clock skew smaller than
    /// the rate-limit window (~3600s) is the assumed invariant.
    ///
    /// # Concurrency
    ///
    /// Two concurrent requests can both trigger halt and store to
    /// `halted_until` simultaneously via `fetch_max`. This is benign:
    /// `fetch_max` ensures the timestamp only moves forward.
    fn is_halted(&self) -> bool {
        let until = self.halted_until.load(Ordering::Acquire);
        if until == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now < until
    }

    /// Clear the rate-limit halt, allowing new requests immediately.
    ///
    /// Primarily useful for the webhook long-lived client model where
    /// a scheduled sweep start should not inherit a stale halt from
    /// a previous window.
    ///
    /// # Concurrency note
    ///
    /// A concurrent `fetch_max` on another thread could race and overwrite
    /// the zero stored here. This is benign: if the rate limit is still
    /// exhausted, the next response will re-trigger the halt.
    pub fn reset_halt(&self) {
        self.halted_until.store(0, Ordering::Release);
    }

    /// Ensure the credential is valid, refreshing if necessary.
    ///
    /// Uses double-checked locking: a lock-free atomic check on the fast path,
    /// then acquires a dedicated `refresh_lock` (separate from the credential
    /// data mutex) and re-checks before refreshing. The HTTP token exchange
    /// is performed without holding the credential data lock, so other
    /// readers of `credential` are never blocked on network I/O.
    async fn ensure_credential(&self) -> Result<(), GitHubApiError> {
        if !self.credential_needs_refresh() {
            return Ok(());
        }

        let _refresh_guard = self.refresh_lock.lock().await;

        if !self.credential_needs_refresh() {
            return Ok(());
        }

        let app_config =
            self.app_config
                .as_ref()
                .ok_or_else(|| GitHubApiError::AuthenticationFailed {
                    reason: "token expired but no GitHub App config available for refresh"
                        .to_string(),
                })?;

        info!("refreshing GitHub App installation token");
        let new_cred = self.exchange_installation_token(app_config).await?;
        let new_expiry = new_cred.expires_at_unix();

        let auth_value = Zeroizing::new(format!("Bearer {}", new_cred.token.expose_secret()));
        let new_header = HeaderValue::from_str(&auth_value).map_err(|_| {
            GitHubApiError::AuthenticationFailed {
                reason: "invalid token characters".to_string(),
            }
        })?;

        {
            let mut cred = self.credential.lock().await;
            *cred = new_cred;
        }
        self.auth_header.store(Arc::new(new_header));
        self.credential_expires_at
            .store(new_expiry, Ordering::Release);

        Ok(())
    }

    /// Exchange a GitHub App JWT for an installation token.
    ///
    /// Reuses `self.http` (preserving the connection pool) and passes the
    /// JWT as a per-request `Authorization` header — consistent with how
    /// the main request path injects installation tokens.
    async fn exchange_installation_token(
        &self,
        app_config: &GitHubAppConfig,
    ) -> Result<GitHubCredential, GitHubApiError> {
        let installation_id = app_config.installation_id;

        let jwt = generate_app_jwt(app_config)?;
        let jwt_auth = HeaderValue::from_str(&format!("Bearer {jwt}")).map_err(|_| {
            GitHubApiError::AuthenticationFailed {
                reason: "invalid JWT characters".to_string(),
            }
        })?;
        let url = format!(
            "{}/app/installations/{installation_id}/access_tokens",
            self.base_url
        );

        let response = self
            .http
            .post(&url)
            .header(AUTHORIZATION, jwt_auth)
            .timeout(Duration::from_secs(config::DEFAULT_REQUEST_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|e| GitHubApiError::AuthenticationFailed {
                reason: format!("installation token exchange failed: {e}"),
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES)
                .await
                .unwrap_or_default();
            return Err(GitHubApiError::AuthenticationFailed {
                reason: format!(
                    "installation token exchange returned {status}: {}",
                    truncate_error_body(&body)
                ),
            });
        }

        let body = read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES)
            .await
            .map_err(|e| GitHubApiError::AuthenticationFailed {
                reason: format!("failed to read installation token response: {e}"),
            })?;
        let token_response: InstallationTokenResponse =
            serde_json::from_str(&body).map_err(|e| GitHubApiError::AuthenticationFailed {
                reason: format!("invalid installation token response: {e}"),
            })?;

        let expires_at = token_response
            .expires_at
            .parse::<Timestamp>()
            .map_err(|e| GitHubApiError::AuthenticationFailed {
                reason: format!("invalid expires_at in token response: {e}"),
            })?;

        Ok(GitHubCredential::from_installation_token(
            token_response.token,
            expires_at,
        ))
    }

    /// Make an API request with retry handling.
    ///
    /// Uses exponential backoff with jitter for retryable failures.
    /// Automatically refreshes GitHub App tokens before they expire.
    /// Returns an immediate `Failure` if the rate-limit halt flag is set.
    #[instrument(skip_all, fields(path, paginate))]
    pub async fn request(
        &self,
        path: &str,
        paginate: bool,
        retries: u32,
        timeout_secs: u64,
    ) -> ApiOutcome {
        if self.is_halted() {
            return ApiOutcome::failure(
                None,
                format!(
                    "rate limit halt: remaining < {}",
                    crate::github::rate_limit::HALT_THRESHOLD
                ),
                false,
            );
        }

        if let Err(e) = self.ensure_credential().await {
            return ApiOutcome::failure(None, format!("credential refresh failed: {e}"), false);
        }

        if !self.budget.acquire(&self.budget_cancel).await {
            return ApiOutcome::failure(None, "budget acquire cancelled".to_string(), false);
        }

        let mut stale_token_retried = false;
        let attempts = retries + 1;
        for attempt in 0..attempts {
            debug!(path = %path, attempt, paginate, "API request attempt");
            let result = if paginate {
                self.request_paginated(path, timeout_secs).await
            } else {
                self.request_single(path, timeout_secs).await
            };

            if self.rate_limit.should_halt() {
                let halt_until = self.rate_limit.load_reset().unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        .saturating_add(3600)
                });
                self.halted_until.fetch_max(halt_until, Ordering::Release);
                error!(
                    remaining = ?self.rate_limit.load_remaining(),
                    halt_until,
                    "rate limit halt triggered — requests blocked until reset window"
                );
                return result;
            }

            if result.status_code() == Some(401) && !stale_token_retried {
                stale_token_retried = true;
                if let Err(e) = self.ensure_credential().await {
                    return ApiOutcome::failure(
                        None,
                        format!("credential refresh failed: {e}"),
                        false,
                    );
                }
                continue;
            }

            if result.is_retryable() && attempt < attempts - 1 {
                if result.status_code() == Some(429) {
                    self.rate_limit_warnings.fetch_add(1, Ordering::Relaxed);
                }
                let base_ms = 1000u64 * 2u64.pow(attempt);
                let jitter_ms = fastrand_jitter(base_ms);
                let backoff = Duration::from_millis(base_ms + jitter_ms);
                debug!(
                    backoff_ms = backoff.as_millis(),
                    attempt,
                    status_code = ?result.status_code(),
                    "retrying after backoff"
                );
                tokio::time::sleep(backoff).await;
                continue;
            }

            return result;
        }

        ApiOutcome::failure(None, "retry exhaustion".to_string(), false)
    }

    /// Make a single (non-paginated) API request.
    async fn request_single(&self, path: &str, timeout_secs: u64) -> ApiOutcome {
        self.request_single_inner(path, timeout_secs, false).await
    }

    /// Make a single request that captures response headers.
    ///
    /// Used for metadata collection (e.g., `X-OAuth-Scopes` header parsing).
    async fn request_single_with_headers(&self, path: &str, timeout_secs: u64) -> ApiOutcome {
        self.request_single_inner(path, timeout_secs, true).await
    }

    /// Shared implementation for single (non-paginated) API requests.
    ///
    /// When `capture_headers` is true, captures OAuth-related response headers
    /// and includes them in the result.
    async fn request_single_inner(
        &self,
        path: &str,
        timeout_secs: u64,
        capture_headers: bool,
    ) -> ApiOutcome {
        let url = format!("{}{}", self.base_url, path);
        let auth = self.auth_header.load();
        let response = match self
            .http
            .get(&url)
            .header(AUTHORIZATION, (**auth).clone())
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) if e.is_timeout() => {
                return ApiOutcome::failure(None, "timeout".to_string(), true);
            }
            Err(e) => {
                return ApiOutcome::failure(None, e.to_string(), true);
            }
        };

        crate::github::rate_limit::update_from_headers(&self.rate_limit, response.headers());
        let status = response.status().as_u16();

        let response_etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let extracted_headers = if capture_headers {
            let mut headers = HashMap::new();
            for key in &[
                "x-oauth-scopes",
                "x-accepted-oauth-scopes",
                "x-oauth-client-id",
            ] {
                if let Some(val) = response.headers().get(*key)
                    && let Ok(s) = val.to_str()
                {
                    headers.insert(key.to_string(), s.to_string());
                }
            }
            Some(headers)
        } else {
            None
        };

        if !response.status().is_success() {
            let retryable = matches!(status, 429 | 500 | 502 | 503 | 504);
            let body = read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES)
                .await
                .unwrap_or_default();
            return ApiOutcome::failure(Some(status), truncate_error_body(&body), retryable);
        }

        let body = match read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                return ApiOutcome::failure(Some(status), format!("body read error: {e}"), false);
            }
        };

        if body.trim().is_empty() {
            if let Some(ref etag) = response_etag {
                self.last_response_etags
                    .upsert_sync(path.to_string(), etag.clone());
            }
            return ApiOutcome::Success {
                status_code: status,
                data: None,
                headers: extracted_headers,
                truncated: false,
            };
        }

        match serde_json::from_str(&body) {
            Ok(data) => {
                if let Some(ref etag) = response_etag {
                    self.last_response_etags
                        .upsert_sync(path.to_string(), etag.clone());
                }
                ApiOutcome::Success {
                    status_code: status,
                    data: Some(data),
                    headers: extracted_headers,
                    truncated: false,
                }
            }
            Err(e) => ApiOutcome::failure(Some(status), format!("invalid json: {e}"), false),
        }
    }

    /// Make a paginated API request, collecting all pages.
    ///
    /// # Safety invariants
    /// - Validates that each `next` URL from the `Link` header belongs to the
    ///   same origin as `self.base_url` to prevent SSRF attacks.
    /// - Enforces a maximum page count to prevent OOM from unbounded pagination.
    async fn request_paginated(&self, path: &str, timeout_secs: u64) -> ApiOutcome {
        let mut all_items: Vec<serde_json::Value> = Vec::new();
        let mut next_url: Option<String> = Some(format!("{}{}", self.base_url, path));
        let mut page_count: usize = 0;
        let mut truncated = false;

        while let Some(url) = next_url.take() {
            page_count += 1;
            if page_count > config::MAX_PAGINATION_PAGES {
                warn!(
                    pages = config::MAX_PAGINATION_PAGES,
                    path, "pagination limit reached"
                );
                truncated = true;
                break;
            }

            if self.is_halted() {
                warn!(
                    path = %path,
                    pages_completed = page_count - 1,
                    "pagination halted due to rate limit"
                );
                truncated = true;
                break;
            }

            if page_count > 1 && !self.budget.acquire(&self.budget_cancel).await {
                truncated = true;
                break;
            }

            let response = match self.send_paginated_request(&url, timeout_secs).await {
                Ok(resp) => resp,
                Err(outcome) => return outcome,
            };

            crate::github::rate_limit::update_from_headers(&self.rate_limit, response.headers());
            let status = response.status().as_u16();

            if !response.status().is_success() {
                let retryable = matches!(status, 429 | 500 | 502 | 503 | 504);
                let body = read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES)
                    .await
                    .unwrap_or_default();
                return ApiOutcome::failure(Some(status), truncate_error_body(&body), retryable);
            }

            next_url = trusted_next_url(response.headers(), &self.trusted_origin);

            let body = match read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES).await {
                Ok(b) => b,
                Err(e) => {
                    return ApiOutcome::failure(
                        Some(status),
                        format!("body read error: {e}"),
                        false,
                    );
                }
            };

            if body.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(serde_json::Value::Array(items)) => {
                    let remaining = config::MAX_PAGINATED_ITEMS.saturating_sub(all_items.len());
                    if items.len() > remaining {
                        all_items.extend(items.into_iter().take(remaining));
                        warn!(
                            items = all_items.len(),
                            path, "paginated item limit reached"
                        );
                        truncated = true;
                        break;
                    }
                    all_items.extend(items);
                }
                Ok(single) => {
                    if all_items.len() >= config::MAX_PAGINATED_ITEMS {
                        warn!(
                            items = all_items.len(),
                            path, "paginated item limit reached"
                        );
                        truncated = true;
                        break;
                    }
                    all_items.push(single);
                }
                Err(e) => {
                    return ApiOutcome::failure(Some(status), format!("invalid json: {e}"), false);
                }
            }
        }

        debug!(
            path = %path,
            pages = page_count,
            items = all_items.len(),
            truncated,
            "paginated request complete"
        );
        ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::Value::Array(all_items)),
            headers: None,
            truncated,
        }
    }

    /// Send a single HTTP GET for a pagination page.
    ///
    /// Returns the `Response` on success, or an `ApiOutcome` error.
    async fn send_paginated_request(
        &self,
        url: &str,
        timeout_secs: u64,
    ) -> Result<reqwest::Response, ApiOutcome> {
        let auth = self.auth_header.load();
        self.http
            .get(url)
            .header(AUTHORIZATION, (**auth).clone())
            .timeout(Duration::from_secs(timeout_secs))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ApiOutcome::failure(None, "timeout".to_string(), true)
                } else {
                    ApiOutcome::failure(None, e.to_string(), true)
                }
            })
    }

    /// Get cached or fresh repository details.
    ///
    /// Only successful results are cached; transient failures are not,
    /// to allow recovery on subsequent calls.
    ///
    /// Stale cache entries (marked by `evict_stale_entries`) are
    /// revalidated using `ETag` conditional requests to save bandwidth
    /// when the response body hasn't changed.
    ///
    /// The full request path uses `request()`, which provides retry
    /// logic, exponential backoff, and stale-token recovery. `ETags`
    /// are captured via a side-channel and read after the call.
    ///
    /// # Budget note
    /// Stale entries that fail `ETag` revalidation consume two budget
    /// permits: one for the conditional request, one for the full
    /// retry — a deliberate trade-off, since this is rare.
    ///
    /// # Path safety
    /// `repo_name` is validated by `sanitize_path_segment` before URL
    /// interpolation to prevent path injection from API-derived data.
    pub async fn repo_details(&self, repo_name: &str) -> ApiOutcome {
        enum CacheHit {
            Fresh {
                status_code: u16,
                data: Option<serde_json::Value>,
            },
            Stale {
                etag: Option<String>,
                data: Option<serde_json::Value>,
                status_code: u16,
            },
        }

        let safe_name = match cherry_pit_web::sanitize_path_segment(repo_name, "repo_name") {
            Ok(n) => n,
            Err(e) => {
                return ApiOutcome::failure(None, format!("invalid repo name: {e}"), false);
            }
        };

        let path = format!("/repos/{}/{}", self.org_name, safe_name);

        let cache_hit = self.repo_detail_cache.read_sync(repo_name, |_, cached| {
            if cached.stale {
                CacheHit::Stale {
                    etag: cached.etag.clone(),
                    data: cached.data.clone(),
                    status_code: cached.status_code,
                }
            } else {
                CacheHit::Fresh {
                    status_code: cached.status_code,
                    data: cached.data.clone(),
                }
            }
        });

        match cache_hit {
            Some(CacheHit::Fresh { status_code, data }) => {
                return ApiOutcome::Success {
                    status_code,
                    data,
                    headers: None,
                    truncated: false,
                };
            }
            Some(CacheHit::Stale {
                etag,
                data,
                status_code,
            }) => {
                if let Some(outcome) = self
                    .try_etag_revalidation(
                        repo_name,
                        &path,
                        etag.as_deref(),
                        data.as_ref(),
                        status_code,
                    )
                    .await
                {
                    return outcome;
                }
            }
            None => {}
        }

        let result = self
            .request(
                &path,
                false,
                config::DEFAULT_MAX_RETRIES,
                config::DEFAULT_REQUEST_TIMEOUT_SECS,
            )
            .await;

        let response_etag = self.last_response_etags.remove_sync(&path).map(|(_, v)| v);

        if let ApiOutcome::Success {
            status_code,
            ref data,
            ..
        } = result
        {
            self.repo_detail_cache.upsert_sync(
                repo_name.to_string(),
                CachedResult {
                    status_code,
                    data: data.clone(),
                    etag: response_etag,
                    stale: false,
                },
            );
        }

        result
    }

    /// Attempt to revalidate a stale cache entry using an `ETag` conditional request.
    ///
    /// Returns `Some(outcome)` if revalidation succeeded (304 or fresh 200).
    /// Returns `None` if revalidation failed and the caller should fall through
    /// to a full request.
    async fn try_etag_revalidation(
        &self,
        repo_name: &str,
        path: &str,
        etag: Option<&str>,
        cached_data: Option<&serde_json::Value>,
        cached_status: u16,
    ) -> Option<ApiOutcome> {
        let etag_value = etag?;

        if let Err(e) = self.ensure_credential().await {
            debug!(error = %e, "credential refresh failed during ETag revalidation");
            return None;
        }

        if self.is_halted() {
            return None;
        }

        if !self.budget.acquire(&self.budget_cancel).await {
            return None;
        }

        let url = format!("{}{}", self.base_url, path);
        let auth = self.auth_header.load();
        let response = match self
            .http
            .get(&url)
            .header(AUTHORIZATION, (**auth).clone())
            .header("If-None-Match", etag_value)
            .timeout(Duration::from_secs(config::DEFAULT_REQUEST_TIMEOUT_SECS))
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                debug!(repo = %repo_name, error = %e, "ETag revalidation: network error, falling through");
                return None;
            }
        };

        crate::github::rate_limit::update_from_headers(&self.rate_limit, response.headers());
        let status = response.status().as_u16();
        let new_etag = response
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        if status == 304 {
            debug!(repo = %repo_name, "ETag revalidation: 304 Not Modified");
            self.repo_detail_cache.upsert_sync(
                repo_name.to_string(),
                CachedResult {
                    status_code: cached_status,
                    data: cached_data.cloned(),
                    etag: new_etag.or_else(|| etag.map(String::from)),
                    stale: false,
                },
            );
            return Some(ApiOutcome::Success {
                status_code: cached_status,
                data: cached_data.cloned(),
                headers: None,
                truncated: false,
            });
        }

        if !response.status().is_success() {
            debug!(repo = %repo_name, status, "ETag revalidation: error status, falling through");
            return None;
        }

        let body = match read_body_limited(response, config::MAX_RESPONSE_BODY_BYTES).await {
            Ok(b) => b,
            Err(e) => {
                debug!(repo = %repo_name, error = %e, "ETag revalidation: body read error, falling through");
                return None;
            }
        };

        self.cache_revalidated_response(repo_name, status, new_etag, &body)
    }

    /// Cache and return the result of a fresh 200 from `ETag` revalidation.
    fn cache_revalidated_response(
        &self,
        repo_name: &str,
        status: u16,
        new_etag: Option<String>,
        body: &str,
    ) -> Option<ApiOutcome> {
        if body.trim().is_empty() {
            self.repo_detail_cache.upsert_sync(
                repo_name.to_string(),
                CachedResult {
                    status_code: status,
                    data: None,
                    etag: new_etag,
                    stale: false,
                },
            );
            return Some(ApiOutcome::Success {
                status_code: status,
                data: None,
                headers: None,
                truncated: false,
            });
        }

        match serde_json::from_str(body) {
            Ok(data) => {
                self.repo_detail_cache.upsert_sync(
                    repo_name.to_string(),
                    CachedResult {
                        status_code: status,
                        data: Some(data),
                        etag: new_etag,
                        stale: false,
                    },
                );
                self.repo_detail_cache
                    .read_sync(repo_name, |_, cached| ApiOutcome::Success {
                        status_code: cached.status_code,
                        data: cached.data.clone(),
                        headers: None,
                        truncated: false,
                    })
            }
            Err(e) => {
                debug!(repo = %repo_name, error = %e, "ETag revalidation: JSON parse error, falling through");
                None
            }
        }
    }

    /// Collect auth metadata by inspecting API response headers.
    ///
    /// Makes a lightweight API call and extracts `X-OAuth-Scopes` to determine
    /// the token tier and available scopes. For GitHub App tokens, the
    /// `X-OAuth-Scopes` header is not present; we return app-specific metadata.
    pub async fn collect_auth_metadata(&self) -> AuthMetadata {
        let cred_mode = {
            let cred = self.credential.lock().await;
            cred.mode
        };

        if cred_mode == crate::domain::auth::AuthMode::GitHubApp {
            let meta = AuthMetadata::for_github_app();
            *self.auth_metadata.lock().unwrap_or_else(|e| {
                warn!("auth_metadata mutex poisoned, recovering");
                e.into_inner()
            }) = Some(meta.clone());
            return meta;
        }

        let result = self.request_single_with_headers("/user", 15).await;

        let meta = if let Some(headers) = result.headers() {
            if let Some(scopes_header) = headers.get("x-oauth-scopes") {
                let scopes = parse_oauth_scopes(scopes_header);
                AuthMetadata::from_scopes(&scopes, &cred_mode)
            } else {
                AuthMetadata {
                    token_tier: crate::domain::auth::TokenTier::Unknown,
                    token_scopes: "not-available".to_string(),
                    auth_mode: cred_mode,
                }
            }
        } else {
            AuthMetadata::default()
        };

        *self.auth_metadata.lock().unwrap_or_else(|e| {
            warn!("auth_metadata mutex poisoned, recovering");
            e.into_inner()
        }) = Some(meta.clone());
        meta
    }

    /// Attach a pause-notify to the budget gate.
    ///
    /// Safe to call on a shared `Arc<GitHubClient>` — delegates to
    /// `BudgetGate::set_pause_notify(&self)` which uses interior mutability.
    /// Replaces any previously attached `Notify`.
    pub fn set_budget_pause_notify(&self, notify: Arc<tokio::sync::Notify>) {
        self.budget.set_pause_notify(notify);
    }

    /// Return the cumulative number of API calls made via the budget gate.
    #[must_use]
    pub fn budget_total_calls(&self) -> u64 {
        self.budget.total_calls_made()
    }

    /// Clear per-run caches without destroying the client.
    ///
    /// Clears the in-memory `scc::HashMap` repo detail cache and the `ETag`
    /// side-channel. Does **not** affect the shared `BudgetGate`,
    /// `RateLimitState`, HTTP connection pool, or credentials.
    ///
    /// # Ordering constraint
    ///
    /// Must be called only after all workers from the previous run have
    /// been fully joined and before new workers are spawned. Calling this
    /// while workers hold references to the `scc::HashMap` entries will cause
    /// them to re-fetch from the API (correct but wasteful).
    pub fn clear_run_cache(&self) {
        self.repo_detail_cache.clear_sync();
        self.last_response_etags.clear_sync();
        self.rate_limit_warnings
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Export the repo detail cache for cross-run persistence.
    ///
    /// Returns `(repo_name, CachedRepoDetail)` pairs extracted from the
    /// in-memory `scc::HashMap`. Only entries with data are exported; entries
    /// with `data: None` (e.g., 204 responses) are skipped since they
    /// carry no useful detail for cross-run caching.
    ///
    /// # Field contract
    ///
    /// The following fields are preserved and must be kept in sync with
    /// [`Self::seed_cache`]:
    ///
    /// - `default_branch` — used by all collectors to identify the primary branch
    /// - `updated_at` — used by baseline mechanism and staleness eviction
    /// - `security_and_analysis` — used by `dependabot` and `ghas_scanning` collectors
    /// - `is_security_policy_enabled` — used by `security_policy` collector
    ///
    /// If a collector starts consuming a new field from `repo_details()`,
    /// both `export_cache` and `seed_cache` must be updated to preserve it.
    pub fn export_cache(&self) -> Vec<(String, crate::domain::cache::CachedRepoDetail)> {
        let now = Timestamp::now();
        let mut exported = Vec::new();
        self.repo_detail_cache.iter_sync(|key, cached| {
            if let Some(data) = cached.data.as_ref() {
                let default_branch = data
                    .get("default_branch")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main")
                    .to_string();
                let updated_at = data
                    .get("updated_at")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let security_and_analysis = data.get("security_and_analysis").cloned();
                let is_security_policy_enabled = data
                    .get("is_security_policy_enabled")
                    .and_then(serde_json::Value::as_bool);
                exported.push((
                    key.clone(),
                    crate::domain::cache::CachedRepoDetail {
                        default_branch,
                        updated_at,
                        security_and_analysis,
                        is_security_policy_enabled,
                        fetched_at: now,
                        etag: cached.etag.clone(),
                    },
                ));
            }
            true
        });
        exported
    }

    /// Seed the repo detail cache from cross-run cached entries.
    ///
    /// Inserts pre-fetched entries into the `scc::HashMap`, allowing
    /// subsequent `repo_details()` calls to return cached results
    /// without hitting the GitHub API. Only entries within the
    /// configured TTL (`REPO_CACHE_TTL_HOURS`) are accepted.
    ///
    /// After seeding, call [`Self::evict_stale_entries`] with the current
    /// inventory to remove entries whose `updated_at` no longer matches.
    ///
    /// # Field contract
    ///
    /// Reconstructs a minimal JSON object with the same fields exported
    /// by [`Self::export_cache`]: `default_branch`, `updated_at`,
    /// `security_and_analysis`, `is_security_policy_enabled`. Both
    /// methods must stay in sync.
    ///
    /// # Panics
    ///
    /// Panics if `REPO_CACHE_TTL_HOURS` does not fit in an `i64`.
    pub fn seed_cache(&self, entries: Vec<(String, crate::domain::cache::CachedRepoDetail)>) {
        let ttl = SignedDuration::from_hours(
            i64::try_from(crate::config::REPO_CACHE_TTL_HOURS)
                .expect("REPO_CACHE_TTL_HOURS fits in i64"),
        );
        let cutoff = Timestamp::now() - ttl;
        let mut seeded = 0usize;
        for (key, detail) in entries {
            if detail.fetched_at < cutoff {
                continue;
            }
            let mut obj = serde_json::Map::new();
            obj.insert(
                "default_branch".to_string(),
                serde_json::Value::String(detail.default_branch),
            );
            if let Some(ref ua) = detail.updated_at {
                obj.insert(
                    "updated_at".to_string(),
                    serde_json::Value::String(ua.clone()),
                );
            }
            if let Some(ref sa) = detail.security_and_analysis {
                obj.insert("security_and_analysis".to_string(), sa.clone());
            }
            if let Some(enabled) = detail.is_security_policy_enabled {
                obj.insert(
                    "is_security_policy_enabled".to_string(),
                    serde_json::Value::Bool(enabled),
                );
            }
            self.repo_detail_cache.upsert_sync(
                key,
                CachedResult {
                    status_code: 200,
                    data: Some(serde_json::Value::Object(obj)),
                    etag: detail.etag,
                    stale: false,
                },
            );
            seeded += 1;
        }
        if seeded > 0 {
            info!(seeded, "repo detail cache seeded from cross-run cache");
        }
    }

    /// Mark seeded cache entries whose `updated_at` no longer matches the
    /// current inventory as stale for `ETag` revalidation.
    ///
    /// Called after the repository inventory is loaded to prevent stale
    /// `security_and_analysis` or `is_security_policy_enabled` data from
    /// being served for repos that changed between runs. Entries whose
    /// cached `updated_at` differs from the inventory's current value are
    /// marked stale. On the next `repo_details()` call, stale entries
    /// attempt `ETag` conditional revalidation before falling through to a
    /// full request.
    ///
    /// # Ordering constraint
    ///
    /// Must be called **before** concurrent repo evaluation begins.
    ///
    /// `repos` is a slice of `(repo_name, current_updated_at)` pairs from
    /// the inventory. Accepts a generic representation to avoid coupling
    /// the `github::client` module to `domain::repository`.
    pub fn evict_stale_entries(&self, repos: &[(String, Option<String>)]) {
        let mut marked = 0usize;
        for (name, current_updated_at) in repos {
            let was_stale = self.repo_detail_cache.update_sync(name, |_, cached| {
                let cached_updated_at = cached
                    .data
                    .as_ref()
                    .and_then(|d| d.get("updated_at"))
                    .and_then(serde_json::Value::as_str);

                let is_stale = match (cached_updated_at, current_updated_at.as_deref()) {
                    (Some(c), Some(cur)) => c != cur,
                    _ => false,
                };

                if is_stale {
                    cached.stale = true;
                }
                is_stale
            });
            if was_stale == Some(true) {
                marked += 1;
            }
        }
        if marked > 0 {
            info!(
                marked,
                "marked stale repo detail cache entries for ETag revalidation"
            );
        }
    }

    /// Probe GitHub API capabilities to determine which API families are accessible.
    ///
    /// Makes lightweight test calls to key org-level endpoints and records
    /// which are accessible. This allows the collector to degrade gracefully
    /// for optional checks while failing closed on mandatory capabilities.
    ///
    /// `PrivateBranchProtectionRead` is probed against a single sample repo
    /// drawn from the org repository listing response above — the full repo
    /// inventory is not yet loaded at this point in startup — via a real GET
    /// against that repo's rulesets endpoint.
    ///
    /// Other repo-level capabilities (contents, per-repo branch protection,
    /// etc.) are not probed here — each collector independently calls the
    /// relevant API and handles permission errors per-repo.
    pub async fn probe_capabilities(&self) -> CapabilitySet {
        let repos_list_probe = self
            .request_single(
                &format!("/orgs/{}/repos?per_page=1", self.org_name),
                config::DEFAULT_REQUEST_TIMEOUT_SECS,
            )
            .await;
        let repos_list = classify_capability_probe(&repos_list_probe);
        let sample_repo = first_repo_name(repos_list_probe.data())
            .and_then(|name| cherry_pit_web::sanitize_path_segment(name, "repo_name").ok())
            .map(std::borrow::Cow::into_owned);

        let org_secret_scanning_alerts = self
            .probe_endpoint(&format!(
                "/orgs/{}/secret-scanning/alerts?per_page=1&state=open",
                self.org_name
            ))
            .await;

        let private_branch_protection_read = match sample_repo {
            Some(repo) => {
                self.probe_endpoint(&format!("/repos/{}/{}/rulesets", self.org_name, repo))
                    .await
            }
            None => CapabilityStatus::NotProbed,
        };

        let caps = CapabilitySet {
            repos_list,
            org_secret_scanning_alerts,
            private_branch_protection_read,
        };

        if !caps.can_run() {
            error!("mandatory capability probe failed: cannot list org repositories");
        }

        let unavail = caps.unavailable_capabilities();
        if !unavail.is_empty() {
            let names: Vec<String> = unavail.iter().map(ToString::to_string).collect();
            warn!(capabilities = %names.join(", "), "optional capabilities denied");
        }

        caps
    }

    /// Probe a single endpoint and return its capability status.
    ///
    /// Note: 403 and 404 are both classified as `PermissionDenied` because
    /// GitHub returns 404 for unauthorized access to private resources. If
    /// needed, distinguish these in the future.
    async fn probe_endpoint(&self, path: &str) -> CapabilityStatus {
        let result = self
            .request_single(path, config::DEFAULT_REQUEST_TIMEOUT_SECS)
            .await;
        classify_capability_probe(&result)
    }
}

fn classify_capability_probe(result: &ApiOutcome) -> CapabilityStatus {
    match result.status_code() {
        Some(status) if (200..300).contains(&status) => CapabilityStatus::Available,
        Some(403 | 404) => CapabilityStatus::PermissionDenied,
        _ => CapabilityStatus::Unavailable,
    }
}

fn first_repo_name(data: Option<&serde_json::Value>) -> Option<&str> {
    data?.as_array()?.first()?.get("name")?.as_str()
}

/// Simple jitter: returns a random value in [0, `max_ms`).
fn fastrand_jitter(max_ms: u64) -> u64 {
    if max_ms == 0 {
        return 0;
    }
    fastrand::u64(0..max_ms)
}

impl std::fmt::Debug for GitHubClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubClient")
            .field("base_url", &self.base_url)
            .field("org_name", &self.org_name)
            .field(
                "rate_limit_warnings",
                &self.rate_limit_warnings.load(Ordering::Relaxed),
            )
            .field("halted_until", &self.halted_until.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_api_base_url_accepts_https() {
        let result = validate_api_base_url("https://api.github.com");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://api.github.com");
    }

    #[test]
    fn validate_api_base_url_strips_trailing_slash() {
        let result = validate_api_base_url("https://api.github.com/");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "https://api.github.com");
    }

    #[test]
    fn validate_api_base_url_accepts_http_in_tests() {
        let result = validate_api_base_url("http://localhost:8080");
        assert!(result.is_ok());
    }

    #[test]
    fn validate_api_base_url_rejects_file_scheme() {
        let result = validate_api_base_url("file:///etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn validate_api_base_url_rejects_ftp_scheme() {
        let result = validate_api_base_url("ftp://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn validate_api_base_url_rejects_invalid_url() {
        let result = validate_api_base_url("not a url");
        assert!(result.is_err());
    }

    #[test]
    fn same_origin_validation() {
        let origin = "https://api.github.com";
        assert!(is_same_origin(
            "https://api.github.com/orgs/foo/repos?page=2",
            origin
        ));
        assert!(!is_same_origin("https://evil.example.com/exfil", origin));
        assert!(!is_same_origin("http://api.github.com/repos", origin));
        assert!(!is_same_origin("not-a-url", origin));
    }

    #[test]
    fn same_origin_with_port() {
        let origin = "https://github.example.com:8443";
        assert!(is_same_origin(
            "https://github.example.com:8443/api/v3/repos",
            origin
        ));
        assert!(!is_same_origin(
            "https://github.example.com:9999/api/v3/repos",
            origin
        ));
    }

    #[test]
    fn extract_origin_works() {
        assert_eq!(
            extract_origin("https://api.github.com/v3/repos"),
            Some("https://api.github.com".to_string())
        );
        assert_eq!(
            extract_origin("https://example.com:8443/api"),
            Some("https://example.com:8443".to_string())
        );
        assert_eq!(extract_origin("not-a-url"), None);
    }

    #[test]
    fn jitter_is_bounded() {
        for _ in 0..100 {
            let j = fastrand_jitter(1000);
            assert!(j < 1000);
        }
        assert_eq!(fastrand_jitter(0), 0);
    }

    #[test]
    fn jitter_produces_varying_values() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            seen.insert(fastrand_jitter(1000));
        }
        assert!(
            seen.len() >= 2,
            "expected at least 2 distinct jitter values in 100 calls, got {}",
            seen.len()
        );
    }

    #[test]
    fn truncate_error_body_short() {
        let body = "short error";
        assert_eq!(truncate_error_body(body), "short error");
    }

    #[test]
    fn truncate_error_body_long() {
        let body = "x".repeat(2000);
        let result = truncate_error_body(&body);
        assert!(result.len() < 2000);
        assert!(result.ends_with("…[truncated]"));
    }

    #[test]
    fn truncate_error_body_multibyte_utf8() {
        let body = "🦀".repeat(300);
        let result = truncate_error_body(&body);
        assert!(result.ends_with("…[truncated]"));
        let suffix = "…[truncated]";
        let body_portion = &result[..result.len() - suffix.len()];
        assert!(body_portion.len() <= MAX_ERROR_BODY_LEN);
        assert!(std::str::from_utf8(body_portion.as_bytes()).is_ok());
    }

    /// Helper to build default `Arc<BudgetGate>` and `Arc<RateLimitState>` for tests.
    fn test_budget_and_rate_limit() -> (Arc<BudgetGate>, Arc<RateLimitState>) {
        (
            Arc::new(BudgetGate::new(
                config::API_BUDGET_LIMIT,
                Duration::from_secs(config::API_BUDGET_WAIT_SECS),
            )),
            Arc::new(crate::github::rate_limit::new_default()),
        )
    }

    /// Helper to build a `GitHubClient` pointed at a local mock server.
    fn build_test_client(base_url: &str) -> GitHubClient {
        let credential = GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        let (budget, rate_limit) = test_budget_and_rate_limit();
        GitHubClient::new(credential, base_url, "test-org", None, budget, rate_limit)
            .expect("test client construction should succeed")
    }

    #[tokio::test]
    async fn request_returns_failure_when_budget_acquire_cancelled() {
        let pause = Arc::new(tokio::sync::Notify::new());
        let budget = Arc::new(
            BudgetGate::new(1, Duration::from_mins(1)).with_pause_notify(Arc::clone(&pause)),
        );
        let (_, rate_limit) = test_budget_and_rate_limit();
        let credential = GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        let client = Arc::new(
            GitHubClient::new(
                credential,
                "https://api.github.invalid",
                "test-org",
                None,
                Arc::clone(&budget),
                rate_limit,
            )
            .expect("test client construction should succeed"),
        );

        let warmup_cancel = tokio_util::sync::CancellationToken::new();
        assert!(budget.acquire(&warmup_cancel).await);

        let requester_client = Arc::clone(&client);
        let requester =
            tokio::spawn(async move { requester_client.request("/test/path", false, 0, 5).await });

        pause.notified().await;
        client.budget_cancel.cancel();

        let outcome = tokio::time::timeout(Duration::from_millis(200), requester)
            .await
            .expect("cancelled budget acquire should return promptly")
            .expect("request task should not panic");

        assert!(outcome.is_err());
        match outcome {
            ApiOutcome::Failure {
                status_code,
                error,
                retryable,
            } => {
                assert_eq!(status_code, None);
                assert_eq!(error, "budget acquire cancelled");
                assert!(!retryable);
            }
            ApiOutcome::Success { .. } => panic!("expected Failure, got Success"),
        }
    }

    #[tokio::test]
    async fn request_single_preserves_actual_status_code_201() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/created"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({"id": 42})))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client
            .request_single_inner("/test/created", 10, false)
            .await;

        assert!(result.is_ok());
        assert_eq!(
            result.status_code(),
            Some(201),
            "status_code should reflect actual HTTP status, not hardcoded 200"
        );
        assert_eq!(
            result.data().unwrap().get("id").unwrap().as_u64().unwrap(),
            42
        );
    }

    #[tokio::test]
    async fn request_single_preserves_actual_status_code_202() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/accepted"))
            .respond_with(
                ResponseTemplate::new(202).set_body_json(serde_json::json!({"queued": true})),
            )
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client
            .request_single_inner("/test/accepted", 10, false)
            .await;

        assert!(result.is_ok());
        assert_eq!(result.status_code(), Some(202));
    }

    #[tokio::test]
    async fn request_single_empty_body_preserves_status_code_204() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/no-content"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client
            .request_single_inner("/test/no-content", 10, false)
            .await;

        assert!(result.is_ok());
        assert_eq!(
            result.status_code(),
            Some(204),
            "empty-body success should use actual HTTP status"
        );
        assert!(result.data().is_none(), "204 should have no data");
    }

    #[tokio::test]
    async fn request_single_with_headers_captures_oauth_scopes() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/headers"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"user": "test"}))
                    .append_header("x-oauth-scopes", "repo, read:org")
                    .append_header("x-accepted-oauth-scopes", "read:org"),
            )
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client.request_single_inner("/test/headers", 10, true).await;

        assert!(result.is_ok());
        assert_eq!(result.status_code(), Some(200));

        let headers = result
            .headers()
            .expect("headers should be captured when capture_headers=true");
        assert_eq!(
            headers.get("x-oauth-scopes").map(String::as_str),
            Some("repo, read:org"),
            "x-oauth-scopes header should be captured"
        );
        assert_eq!(
            headers.get("x-accepted-oauth-scopes").map(String::as_str),
            Some("read:org"),
            "x-accepted-oauth-scopes header should be captured"
        );
    }

    #[tokio::test]
    async fn request_single_without_headers_does_not_capture() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/no-headers"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"user": "test"}))
                    .append_header("x-oauth-scopes", "repo"),
            )
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client
            .request_single_inner("/test/no-headers", 10, false)
            .await;

        assert!(result.is_ok());
        assert!(
            result.headers().is_none(),
            "headers should NOT be captured when capture_headers=false"
        );
    }

    /// PAT credentials have `credential_expires_at == 0`. The fast path in
    /// `credential_needs_refresh()` must return `false`, skipping the mutex
    /// entirely. If it mistakenly returned `true`, `ensure_credential()` would
    /// fail with "no GitHub App config available for refresh" since PAT clients
    /// have `app_config: None`.
    #[tokio::test]
    async fn pat_credential_fast_path_skips_refresh() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/pat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());

        assert_eq!(
            client.credential_expires_at.load(Ordering::Relaxed),
            0,
            "PAT credential should have expires_at_unix == 0"
        );

        assert!(
            !client.credential_needs_refresh(),
            "credential_needs_refresh() should return false for PAT"
        );

        let result = client.request("/test/pat", false, 0, 10).await;
        assert!(
            result.is_ok(),
            "PAT request should succeed without credential refresh: {:?}",
            result.error_message()
        );
        assert_eq!(result.status_code(), Some(200));
    }

    /// A 401 response triggers a single stale-token retry. The client calls
    /// `ensure_credential()` (no-op for PAT), then retries the request.
    /// The mock server returns 401 on the first call and 200 on the second.
    #[tokio::test]
    async fn stale_token_401_retries_once_then_succeeds() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(path("/test/retry"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"recovered": true})),
            )
            .mount(&server)
            .await;

        Mock::given(path("/test/retry"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad credentials"))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client.request("/test/retry", false, 1, 10).await;

        assert!(
            result.is_ok(),
            "should succeed after 401 retry: {:?}",
            result.error_message()
        );
        assert_eq!(result.status_code(), Some(200));
    }

    /// A persistent 401 (every response is 401) must terminate, not loop
    /// forever. The 401 retry fires once (setting `stale_token_retried`),
    /// then the second 401 falls through as a non-retryable failure.
    #[tokio::test]
    async fn persistent_401_terminates_without_infinite_loop() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(path("/test/always-401"))
            .respond_with(ResponseTemplate::new(401).set_body_string("permanently invalid"))
            .expect(2)
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client.request("/test/always-401", false, 3, 10).await;

        assert!(!result.is_ok(), "persistent 401 should fail");
        assert_eq!(
            result.status_code(),
            Some(401),
            "should surface the 401 status code"
        );
    }

    const TEST_RSA_PRIVATE_KEY: &str = "-----BEGIN RSA PRIVATE KEY-----\n\
        MIIEowIBAAKCAQEAw4UBwY51Vdbsax7WKa5BFBaFHrT8bNI9HwTUgGnDwcTDCFqD\n\
        E85jo1C7gGUb4fI0SNHXVEHF/itipUVsj+3dVNye11NmZXabZTGnPAkUaIs686bQ\n\
        GYjiPJJGAgP+i7SisSmyln9DIM2/41Nf7MCL58k5uoTHBX/P8Dvp5L/ahEMj3pHH\n\
        jSeZugxE7o235+arPh9glKn/Iuq3get27z+6LGRnTB4rFlNFfv/5jvXLgZOrB9be\n\
        ZQSIHnW6AMHuc6j0INkAetWvVuSJ6zQsftnueMcMgxoA5ugnnAV9GYKhSKr4oYp8\n\
        m+Emz0yrAl6uDRNWTKcgXhFk3UTcsJ5ZuVGliwIDAQABAoIBADO1dwfuObj4jPEt\n\
        qB1A4SRDanR7EDFljtWnzN2jWyrhc2U/rt/rmy1jmhs0YmHo0QwbNzQo6wi0B7RG\n\
        /pW4Jmudp4KyI2gdLK7gKWb2zcdyXyZ2TR4bth2n380DqmvfW5G4QeuMf7/qul+Q\n\
        OtPd/oJQFSzvlcUuDtvttIeTd+K3eqcJ30p1EII9ClogigJivRsbL9UUJwU42JtQ\n\
        fS9giQxEnBTzpfk9H5A+7Gy9DBI06G6iZEKjgHMGXyP2nxtqx0Ek5yGkK2Pt2gg9\n\
        dOe7uC7sh14hRs3DMMvOdRPllQG/uKXQvyHl3an52gYlmr2iwXI1zk9k99QwOyY0\n\
        OcsF7O0CgYEA48Ng5hnKT34CE1PIbB1vmxGvqd5Tp/q01xA4/SiqFBPNlrptFwHt\n\
        dlsi+Ufru0QNI40nFVpooJ8SQ4R3NV33WZXx01+blkIwD+GnmtZTBIWv7EtBYgc3\n\
        OXSzasNZRLwfqgbXZbOUHOJFqd2wtV9rtvX4Kgysd1gWwzcA89IvnH8CgYEA28JM\n\
        NSO/yo0iKf7eQVeeJQJFcWwrlHvVG0rnCrranj+s6atydHo0mT8bTFU56710pe1G\n\
        Q1YK6A74Qpnk9QF9fEPuX1pg9vBO605+zwKEtuFgx3xCJx5YZf1TbqLWXP6dztQ5\n\
        AH3thy1lABg6A6Ybfcvsa2sgc3lRpVFceC1PIPUCgYA18poqBmPQDlWphEfNq+86\n\
        eKb2Ak4oVI6u/g2xkQcv+DzS/ddHAtLfHNkc2HcyhPzjtdRTD3YGzYbC7UZbIqWq\n\
        14RO/69XmNfPezB60VcalBvGSVD0Sic/ea/hkuG7ESAi4rn0QePML6A2iucHHtHh\n\
        pUMhmpzjK79Af+++0MMsOwKBgDVVLSuEVopwwAbTHNtcyTuQFoxVRSpO90QdZH79\n\
        JAtdxrga7LcJ5XP/lb9ru5fTrdiLAg9bdWAmKef381Hmn66lydcIVxn27iA7N5lD\n\
        sjOz9MnVBTT7L1bpKPNjv4RoIqJMbN0Ksreos6dXOdUi3e8kq2bSY9jCa6ckXL2p\n\
        uVd1AoGBAJyUczNlGjCluj9Ekr9C3CofovMgormcmQp09NNiZ/OKk1SW5IKRwu+Y\n\
        9GzOIOhPWTbev+dZ2cDC8mcMth5qsIuGg2mF8lTrijZ04CtnqTcRhiYfWDKf8R6f\n\
        oC+wB51AUMN9YZMwF2xDT8ZdwSDb11j4YiG4twBNZdEep0CwDjgM\n\
        -----END RSA PRIVATE KEY-----\n";

    /// Under concurrency, multiple tasks may observe an expired credential
    /// simultaneously. The double-check locking in `ensure_credential()` must
    /// ensure exactly one token exchange occurs — verified by wiremock's
    /// `expect(1)` assertion on the installation token endpoint.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_credential_refresh_deduplicates() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        let future_expiry = (Timestamp::now() + SignedDuration::from_hours(1)).to_string();
        Mock::given(method("POST"))
            .and(path("/app/installations/12345/access_tokens"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": "ghs_new_test_token_from_refresh",
                "expires_at": future_expiry
            })))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(path("/test/concurrent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;

        let app_config = GitHubAppConfig {
            app_id: 99999,
            private_key_pem: secrecy::SecretString::from(TEST_RSA_PRIVATE_KEY),
            installation_id: 12345,
        };

        let soon = Timestamp::now() + SignedDuration::from_secs(30);
        let credential =
            GitHubCredential::from_installation_token("ghs_about_to_expire".to_string(), soon);

        let (budget, rate_limit) = test_budget_and_rate_limit();
        let client = Arc::new(
            GitHubClient::new(
                credential,
                &server.uri(),
                "test-org",
                Some(app_config),
                budget,
                rate_limit,
            )
            .expect("test client construction should succeed"),
        );

        assert!(
            client.credential_needs_refresh(),
            "test credential should trigger refresh (within 5-min buffer)"
        );

        let n = 10;
        let mut handles = Vec::new();
        for _ in 0..n {
            let c = Arc::clone(&client);
            handles.push(tokio::spawn(async move {
                c.request("/test/concurrent", false, 0, 10).await
            }));
        }

        for handle in handles {
            let result = handle.await.expect("task should not panic");
            assert!(
                result.is_ok(),
                "concurrent request should succeed: {:?}",
                result.error_message()
            );
        }

        assert!(
            !client.credential_needs_refresh(),
            "credential should be fresh after refresh"
        );
    }

    #[test]
    fn export_cache_seed_cache_round_trip() {
        let client_a = build_test_client("https://api.github.com");

        client_a.repo_detail_cache.upsert_sync(
            "my-repo".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(serde_json::json!({
                    "default_branch": "develop",
                    "updated_at": "2026-04-10T12:00:00Z",
                    "security_and_analysis": {
                        "secret_scanning": { "status": "enabled" }
                    },
                    "is_security_policy_enabled": true
                })),
                etag: Some("\"abc123\"".to_string()),
                stale: false,
            },
        );

        let exported = client_a.export_cache();
        assert_eq!(exported.len(), 1);
        let (name, detail) = &exported[0];
        assert_eq!(name, "my-repo");
        assert_eq!(detail.default_branch, "develop");
        assert_eq!(detail.updated_at.as_deref(), Some("2026-04-10T12:00:00Z"));
        assert!(detail.security_and_analysis.is_some());
        assert_eq!(detail.is_security_policy_enabled, Some(true));
        assert_eq!(detail.etag.as_deref(), Some("\"abc123\""));

        let client_b = build_test_client("https://api.github.com");
        assert!(client_b.repo_detail_cache.is_empty());
        client_b.seed_cache(exported);
        assert_eq!(client_b.repo_detail_cache.len(), 1);

        let entry = client_b
            .repo_detail_cache
            .read_sync("my-repo", |_, v| v.clone())
            .expect("seeded entry should exist");
        let data = entry.data.as_ref().expect("seeded entry should have data");
        assert_eq!(entry.status_code, 200);
        assert_eq!(data["default_branch"], "develop");
        assert_eq!(data["updated_at"], "2026-04-10T12:00:00Z");
        assert!(data.get("security_and_analysis").is_some());
        assert_eq!(data["is_security_policy_enabled"], true);
    }

    #[test]
    fn seed_cache_skips_stale_entries() {
        let client = build_test_client("https://api.github.com");

        let stale_detail = crate::domain::cache::CachedRepoDetail {
            default_branch: "main".into(),
            updated_at: Some("2026-01-01T00:00:00Z".into()),
            security_and_analysis: None,
            is_security_policy_enabled: None,
            fetched_at: Timestamp::now() - SignedDuration::from_hours(25),
            etag: None,
        };

        client.seed_cache(vec![("old-repo".to_string(), stale_detail)]);
        assert!(
            client.repo_detail_cache.is_empty(),
            "stale entries (>24h) should not be seeded"
        );
    }

    #[test]
    fn evict_stale_entries_marks_mismatched_updated_at_as_stale() {
        let client = build_test_client("https://api.github.com");

        client.repo_detail_cache.upsert_sync(
            "changed-repo".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(serde_json::json!({
                    "default_branch": "main",
                    "updated_at": "2026-04-10T12:00:00Z"
                })),
                etag: None,
                stale: false,
            },
        );

        let repos = vec![(
            "changed-repo".to_string(),
            Some("2026-04-11T08:00:00Z".to_string()),
        )];
        client.evict_stale_entries(&repos);

        let entry = client
            .repo_detail_cache
            .read_sync("changed-repo", |_, v| v.clone())
            .expect("entry should still exist");
        assert!(
            entry.stale,
            "entry with mismatched updated_at should be marked stale"
        );
    }

    #[test]
    fn evict_stale_entries_keeps_matching_entries() {
        let client = build_test_client("https://api.github.com");

        client.repo_detail_cache.upsert_sync(
            "unchanged-repo".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(serde_json::json!({
                    "default_branch": "main",
                    "updated_at": "2026-04-10T12:00:00Z"
                })),
                etag: None,
                stale: false,
            },
        );

        let repos = vec![(
            "unchanged-repo".to_string(),
            Some("2026-04-10T12:00:00Z".to_string()),
        )];
        client.evict_stale_entries(&repos);

        assert_eq!(
            client.repo_detail_cache.len(),
            1,
            "entry with matching updated_at should be kept"
        );
    }

    #[test]
    fn evict_stale_entries_keeps_entries_when_updated_at_missing() {
        let client = build_test_client("https://api.github.com");

        client.repo_detail_cache.upsert_sync(
            "no-timestamp".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(serde_json::json!({
                    "default_branch": "main"
                })),
                etag: None,
                stale: false,
            },
        );

        let repos = vec![("no-timestamp".to_string(), None)];
        client.evict_stale_entries(&repos);

        assert_eq!(
            client.repo_detail_cache.len(),
            1,
            "entry should be kept when neither side has updated_at"
        );
    }

    #[test]
    fn evict_stale_entries_keeps_entry_when_cached_has_no_updated_at() {
        let client = build_test_client("https://api.github.com");

        client.repo_detail_cache.upsert_sync(
            "no-cached-ts".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(serde_json::json!({
                    "default_branch": "main"
                })),
                etag: None,
                stale: false,
            },
        );

        let repos = vec![(
            "no-cached-ts".to_string(),
            Some("2026-04-11T08:00:00Z".to_string()),
        )];
        client.evict_stale_entries(&repos);

        assert_eq!(
            client.repo_detail_cache.len(),
            1,
            "entry should be kept when cached side has no updated_at"
        );
    }

    #[test]
    fn seed_cache_accepts_entry_near_ttl_boundary() {
        let client = build_test_client("https://api.github.com");

        let ttl_hours = i64::try_from(crate::config::REPO_CACHE_TTL_HOURS).expect("fits in i64");
        let near_boundary_detail = crate::domain::cache::CachedRepoDetail {
            default_branch: "main".into(),
            updated_at: Some("2026-04-10T00:00:00Z".into()),
            security_and_analysis: None,
            is_security_policy_enabled: None,
            fetched_at: Timestamp::now() - SignedDuration::from_hours(ttl_hours)
                + SignedDuration::from_secs(1),
            etag: None,
        };

        client.seed_cache(vec![(
            "near-boundary-repo".to_string(),
            near_boundary_detail,
        )]);
        assert_eq!(
            client.repo_detail_cache.len(),
            1,
            "entry 1s before TTL expiry should be accepted"
        );
    }

    #[tokio::test]
    async fn repo_details_captures_etag_from_response() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/repos/test-org/etag-repo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "default_branch": "main",
                        "updated_at": "2026-04-11T00:00:00Z"
                    }))
                    .insert_header("etag", "\"abc123\""),
            )
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client.repo_details("etag-repo").await;

        assert!(result.is_ok(), "repo_details should succeed");

        let cached = client
            .repo_detail_cache
            .read_sync("etag-repo", |_, v| v.clone())
            .expect("cache entry should exist");
        assert_eq!(
            cached.etag.as_deref(),
            Some("\"abc123\""),
            "ETag should be captured from response into cache"
        );
        assert!(!cached.stale, "fresh entry should not be stale");

        assert!(
            !client
                .last_response_etags
                .contains_sync("/repos/test-org/etag-repo"),
            "side-channel entry should be consumed by repo_details"
        );
    }

    #[tokio::test]
    async fn repo_details_etag_revalidation_304_reuses_cached_data() {
        use wiremock::matchers::{header, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(path("/repos/test-org/revalidate-repo"))
            .and(header("If-None-Match", "\"etag-v1\""))
            .respond_with(ResponseTemplate::new(304))
            .expect(1)
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());

        let cached_data = serde_json::json!({
            "default_branch": "main",
            "updated_at": "2026-04-10T00:00:00Z"
        });
        client.repo_detail_cache.upsert_sync(
            "revalidate-repo".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(cached_data.clone()),
                etag: Some("\"etag-v1\"".to_string()),
                stale: true,
            },
        );

        let result = client.repo_details("revalidate-repo").await;

        assert!(result.is_ok(), "304 revalidation should return success");
        assert_eq!(
            result.status_code(),
            Some(200),
            "should preserve original status code"
        );
        assert_eq!(
            result.data(),
            Some(&cached_data),
            "304 should return the cached data"
        );

        let cached = client
            .repo_detail_cache
            .read_sync("revalidate-repo", |_, v| v.clone())
            .expect("cache entry should exist");
        assert!(!cached.stale, "entry should be marked fresh after 304");
        assert!(
            cached.etag.is_some(),
            "ETag should be preserved after revalidation"
        );
    }

    #[tokio::test]
    async fn repo_details_etag_revalidation_failure_falls_through_to_full_request() {
        use wiremock::matchers::{header, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(path("/repos/test-org/fallthrough-repo"))
            .and(header("If-None-Match", "\"stale-etag\""))
            .respond_with(ResponseTemplate::new(500).set_body_string("server error"))
            .expect(1)
            .named("conditional request")
            .mount(&server)
            .await;

        Mock::given(path("/repos/test-org/fallthrough-repo"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "default_branch": "develop",
                        "updated_at": "2026-04-12T00:00:00Z"
                    }))
                    .insert_header("etag", "\"new-etag\""),
            )
            .expect(1)
            .named("full request")
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());

        client.repo_detail_cache.upsert_sync(
            "fallthrough-repo".to_string(),
            CachedResult {
                status_code: 200,
                data: Some(serde_json::json!({
                    "default_branch": "main",
                    "updated_at": "2026-04-10T00:00:00Z"
                })),
                etag: Some("\"stale-etag\"".to_string()),
                stale: true,
            },
        );

        let result = client.repo_details("fallthrough-repo").await;

        assert!(
            result.is_ok(),
            "fall-through to full request should succeed"
        );
        let data = result.data().expect("should have data");
        assert_eq!(
            data.get("default_branch").and_then(|v| v.as_str()),
            Some("develop"),
            "should return fresh data from full request, not stale cached data"
        );

        let cached = client
            .repo_detail_cache
            .read_sync("fallthrough-repo", |_, v| v.clone())
            .expect("cache entry should exist");
        assert!(!cached.stale, "entry should be fresh after full request");
        assert_eq!(
            cached.etag.as_deref(),
            Some("\"new-etag\""),
            "cache should have the new ETag"
        );
    }

    #[test]
    fn is_halted_returns_false_when_not_halted() {
        let client = build_test_client("https://api.github.com");
        assert!(!client.is_halted());
    }

    #[test]
    fn is_halted_returns_true_when_future_timestamp() {
        let client = build_test_client("https://api.github.com");
        client.halted_until.store(u64::MAX, Ordering::Release);
        assert!(client.is_halted());
    }

    #[test]
    fn is_halted_auto_clears_past_timestamp() {
        let client = build_test_client("https://api.github.com");
        client.halted_until.store(1, Ordering::Release);
        assert!(!client.is_halted());
    }

    #[test]
    fn reset_halt_clears_halted_until() {
        let client = build_test_client("https://api.github.com");
        client.halted_until.store(u64::MAX, Ordering::Release);
        assert!(client.is_halted());
        client.reset_halt();
        assert!(!client.is_halted());
    }

    #[tokio::test]
    async fn shared_budget_gate_cumulates_across_clients() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/test/shared"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .expect(2)
            .mount(&server)
            .await;

        let (budget, rate_limit) = test_budget_and_rate_limit();
        assert_eq!(budget.total_calls_made(), 0);

        let client1 = build_test_client_with_budget(&server.uri(), &budget, &rate_limit);
        let _ = client1.request("/test/shared", false, 0, 10).await;
        let calls_after_run1 = budget.total_calls_made();
        assert!(calls_after_run1 > 0, "budget should have recorded calls");

        let client2 = build_test_client_with_budget(&server.uri(), &budget, &rate_limit);
        let _ = client2.request("/test/shared", false, 0, 10).await;
        let calls_after_run2 = budget.total_calls_made();
        assert!(
            calls_after_run2 > calls_after_run1,
            "budget should cumulate across clients: run1={calls_after_run1}, run2={calls_after_run2}"
        );
    }

    fn build_test_client_with_budget(
        base_url: &str,
        budget: &Arc<BudgetGate>,
        rate_limit: &Arc<RateLimitState>,
    ) -> GitHubClient {
        let credential = GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        GitHubClient::new(
            credential,
            base_url,
            "test-org",
            None,
            Arc::clone(budget),
            Arc::clone(rate_limit),
        )
        .expect("test client construction should succeed")
    }

    #[test]
    fn clear_run_cache_clears_cache_not_budget() {
        let (budget, rate_limit) = test_budget_and_rate_limit();
        let client = build_test_client_with_budget("https://api.github.com", &budget, &rate_limit);

        let detail = crate::domain::cache::CachedRepoDetail {
            default_branch: "main".into(),
            updated_at: Some("2026-04-14T00:00:00Z".into()),
            security_and_analysis: None,
            is_security_policy_enabled: None,
            fetched_at: Timestamp::now(),
            etag: Some("\"abc123\"".into()),
        };
        client.seed_cache(vec![("test-repo".into(), detail)]);

        let exported = client.export_cache();
        assert!(!exported.is_empty(), "cache should have seeded entry");

        client.clear_run_cache();
        let exported_after = client.export_cache();
        assert!(
            exported_after.is_empty(),
            "cache should be empty after clear"
        );

        assert_eq!(budget.total_calls_made(), 0, "budget should be untouched");
    }

    #[test]
    fn first_repo_name_extracts_first_array_entry() {
        let data = serde_json::json!([{"name": "sample-repo"}, {"name": "other"}]);
        assert_eq!(first_repo_name(Some(&data)), Some("sample-repo"));
    }

    #[test]
    fn first_repo_name_none_when_array_empty() {
        let data = serde_json::json!([]);
        assert_eq!(first_repo_name(Some(&data)), None);
    }

    #[test]
    fn first_repo_name_none_when_no_data() {
        assert_eq!(first_repo_name(None), None);
    }

    #[test]
    fn first_repo_name_none_when_entry_has_no_name() {
        let data = serde_json::json!([{"id": 1}]);
        assert_eq!(first_repo_name(Some(&data)), None);
    }

    #[tokio::test]
    async fn probe_capabilities_marks_private_branch_protection_available_on_rulesets_200() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/repos"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"name": "sample-repo"}])),
            )
            .mount(&server)
            .await;
        Mock::given(path("/orgs/test-org/secret-scanning/alerts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(path("/repos/test-org/sample-repo/rulesets"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let caps = client.probe_capabilities().await;

        assert_eq!(
            caps.private_branch_protection_read,
            CapabilityStatus::Available
        );
    }

    #[tokio::test]
    async fn probe_capabilities_marks_private_branch_protection_permission_denied_on_403() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/repos"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{"name": "sample-repo"}])),
            )
            .mount(&server)
            .await;
        Mock::given(path("/orgs/test-org/secret-scanning/alerts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(path("/repos/test-org/sample-repo/rulesets"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let caps = client.probe_capabilities().await;

        assert_eq!(
            caps.private_branch_protection_read,
            CapabilityStatus::PermissionDenied
        );
    }

    #[tokio::test]
    async fn probe_capabilities_leaves_private_branch_protection_not_probed_without_sample_repo() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/orgs/test-org/repos"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        Mock::given(path("/orgs/test-org/secret-scanning/alerts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let caps = client.probe_capabilities().await;

        assert_eq!(
            caps.private_branch_protection_read,
            CapabilityStatus::NotProbed
        );
    }

    #[tokio::test]
    async fn request_paginated_caps_items_before_accumulation_not_after() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let oversized_page: Vec<serde_json::Value> = (0..=config::MAX_PAGINATED_ITEMS)
            .map(|i| serde_json::json!({"id": i}))
            .collect();

        let server = MockServer::start().await;
        Mock::given(path("/oversized-page"))
            .respond_with(ResponseTemplate::new(200).set_body_json(oversized_page))
            .mount(&server)
            .await;

        let client = build_test_client(&server.uri());
        let result = client.request("/oversized-page", true, 1, 60).await;

        assert!(result.is_ok());
        assert!(
            result.is_truncated(),
            "single oversized page must be reported truncated"
        );
        let items = result
            .data()
            .and_then(serde_json::Value::as_array)
            .expect("data should be an array");
        assert_eq!(
            items.len(),
            config::MAX_PAGINATED_ITEMS,
            "cap must be enforced at accumulation time, never exceeded even transiently"
        );
    }
}
