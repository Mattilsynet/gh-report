//! GitHub authentication helpers.
//!
//! Handles token discovery, scope validation, credential management,
//! GitHub App JWT generation, and installation token exchange.

use std::path::Path;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use tracing::{debug, trace};

use crate::domain::auth::AuthMode;
use crate::error::GitHubApiError;

/// Resolved credential for making GitHub API requests.
///
/// The token is stored as a `SecretString` and is never exposed in Debug output.
#[derive(Clone)]
pub struct GitHubCredential {
    /// The authentication mode used.
    pub mode: AuthMode,
    /// The bearer token (kept secret in memory).
    pub token: SecretString,
    /// When the token expires (only relevant for GitHub App installation tokens).
    pub expires_at: Option<Timestamp>,
}

impl std::fmt::Debug for GitHubCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubCredential")
            .field("mode", &self.mode)
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Environment variables removed from `gh` CLI subprocesses to prevent
/// secret leakage. The rest of the environment is preserved so that `gh`
/// can locate its config, resolve proxies, and find TLS certificates.
const GH_ENV_DENYLIST: &[&str] = &[
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GH_APP_PRIVATE_KEY",
    "GH_APP_PRIVATE_KEY_PATH",
    "GITHUB_ACTION",
    "CI",
];

/// Create a `std::process::Command` for the `gh` CLI with dangerous
/// environment variables removed.
fn gh_command(args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("gh");
    cmd.args(args);
    for var in GH_ENV_DENYLIST {
        cmd.env_remove(var);
    }
    cmd
}

impl GitHubCredential {
    /// Discover credentials from the environment.
    ///
    /// Checks `GITHUB_TOKEN` environment variable first, then falls back
    /// to `gh auth token` for local development.
    ///
    /// # Errors
    ///
    /// Returns `GitHubApiError::AuthenticationFailed` if no valid credentials are found.
    pub fn from_environment() -> Result<Self, GitHubApiError> {
        if let Ok(token) = std::env::var("GITHUB_TOKEN")
            && !token.is_empty()
        {
            debug!(
                mode = "pat",
                "credential resolved from GITHUB_TOKEN environment variable"
            );
            return Ok(Self {
                mode: AuthMode::Pat,
                token: SecretString::from(token),
                expires_at: None,
            });
        }

        debug!("GITHUB_TOKEN not set, falling back to gh CLI");
        Self::from_gh_cli()
    }

    /// Get credentials from `gh auth token`.
    fn from_gh_cli() -> Result<Self, GitHubApiError> {
        trace!("attempting credential discovery via gh auth token");
        let output = gh_command(&["auth", "token"]).output().map_err(|_| {
            GitHubApiError::AuthenticationFailed {
                reason: "failed to execute 'gh auth token'".to_string(),
            }
        })?;

        if !output.status.success() {
            debug!("gh auth token returned non-zero exit code");
            return Err(GitHubApiError::AuthenticationFailed {
                reason: "gh auth token failed; run 'gh auth login' first".to_string(),
            });
        }

        let raw_token =
            String::from_utf8(output.stdout).map_err(|_| GitHubApiError::AuthenticationFailed {
                reason: "gh auth token returned non-UTF-8 output".to_string(),
            })?;
        let token = raw_token.trim().to_string();

        if token.is_empty() {
            return Err(GitHubApiError::AuthenticationFailed {
                reason: "gh auth token returned empty token".to_string(),
            });
        }

        debug!(mode = "gh_cli_fallback", "credential resolved from gh CLI");
        Ok(Self {
            mode: AuthMode::GhCliFallback,
            token: SecretString::from(token),
            expires_at: None,
        })
    }

    /// Create a credential from a GitHub App installation token exchange.
    #[must_use]
    pub fn from_installation_token(token: String, expires_at: Timestamp) -> Self {
        Self {
            mode: AuthMode::GitHubApp,
            token: SecretString::from(token),
            expires_at: Some(expires_at),
        }
    }

    /// Whether this credential needs refresh (GitHub App tokens expire).
    ///
    /// Returns true if the token expires within `buffer` of now.
    #[must_use]
    pub fn needs_refresh(&self, buffer: Duration) -> bool {
        match self.expires_at {
            Some(expires) => {
                let buffer_jiff = SignedDuration::try_from(buffer).unwrap_or_default();
                (Timestamp::now() + buffer_jiff) >= expires
            }
            None => false,
        }
    }

    /// Unix timestamp of token expiry, or 0 for tokens that never expire.
    #[must_use]
    pub fn expires_at_unix(&self) -> u64 {
        self.expires_at
            .map_or(0, |ts| ts.as_second().cast_unsigned())
    }
}

use crate::domain::auth::{Capability, TokenTier};

/// The required classic PAT scopes for "Full" tier.
const FULL_TIER_SCOPES: &[&str] = &["repo", "read:org", "security_events"];

/// Determine token tier from a set of OAuth scope strings.
///
/// `security_events` is a sub-scope of `repo`, so when the top-level `repo`
/// scope is granted GitHub only returns `repo` in `X-OAuth-Scopes` – we treat
/// `repo` as implying `security_events`.
#[must_use]
pub fn classify_token_tier(scopes: &[String]) -> TokenTier {
    if scopes.is_empty() {
        return TokenTier::Unknown;
    }
    let scope_set: std::collections::HashSet<&str> = scopes.iter().map(String::as_str).collect();
    let has_scope =
        |s: &str| scope_set.contains(s) || (s == "security_events" && scope_set.contains("repo"));
    if FULL_TIER_SCOPES.iter().all(|s| has_scope(s)) {
        TokenTier::Full
    } else {
        TokenTier::Limited
    }
}

/// Parse the `X-OAuth-Scopes` header value into a list of scope strings.
///
/// The header is a comma-separated list of scope names.
/// Returns an empty vec if the header is empty or absent.
#[must_use]
pub fn parse_oauth_scopes(header_value: &str) -> Vec<String> {
    header_value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Auth metadata collected for evidence recording (scopes, token tier).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthMetadata {
    pub token_tier: TokenTier,
    pub token_scopes: String,
    pub auth_mode: AuthMode,
}

impl Default for AuthMetadata {
    fn default() -> Self {
        Self {
            token_tier: TokenTier::Unknown,
            token_scopes: "unknown".to_string(),
            auth_mode: AuthMode::Unknown,
        }
    }
}

impl AuthMetadata {
    /// Build auth metadata from parsed OAuth scopes and auth mode.
    #[must_use]
    pub fn from_scopes(scopes: &[String], mode: &AuthMode) -> Self {
        let tier = classify_token_tier(scopes);
        let scopes_str = if scopes.is_empty() {
            "unknown".to_string()
        } else {
            scopes.join(", ")
        };
        Self {
            token_tier: tier,
            token_scopes: scopes_str,
            auth_mode: *mode,
        }
    }

    /// Build auth metadata for a GitHub App credential.
    ///
    /// GitHub App installation tokens do not have OAuth scopes; permissions
    /// are defined on the App itself. We report tier as Unknown since
    /// the classic scope model doesn't apply.
    #[must_use]
    pub fn for_github_app() -> Self {
        Self {
            token_tier: TokenTier::Unknown,
            token_scopes: "github-app-installation".to_string(),
            auth_mode: AuthMode::GitHubApp,
        }
    }

    /// Serialize `token_tier` as a string (for backward-compatible evidence output).
    #[must_use]
    pub fn token_tier_str(&self) -> String {
        self.token_tier.to_string()
    }
}

/// Collect auth metadata using `gh auth status --json hosts`.
///
/// Falls back to `AuthMetadata::default()` on any failure.
#[must_use]
pub fn gh_cli_auth_metadata() -> AuthMetadata {
    let Ok(output) = gh_command(&["auth", "status", "--json", "hosts"]).output() else {
        return AuthMetadata::default();
    };

    if !output.status.success() {
        return AuthMetadata::default();
    }

    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return AuthMetadata::default();
    };

    parse_gh_auth_status_json(&stdout)
}

/// Parse the JSON output from `gh auth status --json hosts`.
///
/// Expected shape:
/// ```json
/// {
///   "hosts": {
///     "github.com": [
///       { "active": true, "scopes": "repo, read:org, ..." }
///     ]
///   }
/// }
/// ```
fn parse_gh_auth_status_json(json_str: &str) -> AuthMetadata {
    let payload: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return AuthMetadata::default(),
    };

    let host_entries = match payload
        .get("hosts")
        .and_then(|h| h.get("github.com"))
        .and_then(|e| e.as_array())
    {
        Some(entries) if !entries.is_empty() => entries,
        _ => return AuthMetadata::default(),
    };

    let active_entry = host_entries
        .iter()
        .find(|e| {
            e.get("active")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| host_entries.first());

    let Some(active_entry) = active_entry else {
        return AuthMetadata::default();
    };

    let scopes_str = active_entry
        .get("scopes")
        .and_then(|s| s.as_str())
        .unwrap_or("unknown");

    if scopes_str == "unknown" || scopes_str.is_empty() {
        return AuthMetadata::default();
    }

    let scopes = parse_oauth_scopes(scopes_str);
    let tier = classify_token_tier(&scopes);
    AuthMetadata {
        token_tier: tier,
        token_scopes: scopes_str.to_string(),
        auth_mode: AuthMode::GhCliFallback,
    }
}

/// Parse and validate a positive `u64` from a string.
///
/// Returns `Err` if the string is not a valid `u64` or if the value is `0`.
/// Used for both `GH_APP_ID` and `GH_APP_INSTALLATION_ID`.
fn parse_positive_u64(raw: &str, field_name: &str) -> Result<u64, GitHubApiError> {
    let id: u64 = raw
        .trim()
        .parse()
        .map_err(|_| GitHubApiError::ClientConfigError {
            reason: format!("{field_name} must be a positive integer, got: {raw}"),
        })?;
    if id == 0 {
        return Err(GitHubApiError::ClientConfigError {
            reason: format!("{field_name} must be a positive integer (got 0)"),
        });
    }
    Ok(id)
}

/// Parse and validate a GitHub App ID from a string.
///
/// Returns `Err` if the string is not a valid `u64` or if the value is `0`
/// (which is never a valid GitHub App ID).
///
/// # Errors
///
/// Returns `GitHubApiError::ClientConfigError` if the value is not a positive integer.
pub fn parse_app_id(raw: &str) -> Result<u64, GitHubApiError> {
    parse_positive_u64(raw, "GH_APP_ID")
}

/// Configuration for GitHub App authentication.
#[derive(Debug, Clone)]
pub struct GitHubAppConfig {
    /// The GitHub App ID.
    pub app_id: u64,
    /// PEM-encoded RSA private key for JWT signing.
    pub private_key_pem: SecretString,
    /// GitHub App installation ID (required).
    pub installation_id: u64,
}

impl GitHubAppConfig {
    /// Load GitHub App configuration from environment variables.
    ///
    /// Required: `GH_APP_ID`, `GH_APP_PRIVATE_KEY` or `GH_APP_PRIVATE_KEY_PATH`
    /// Optional: `GH_APP_INSTALLATION_ID`
    ///
    /// # Errors
    ///
    /// Returns `GitHubApiError::ClientConfigError` if required variables are invalid or missing.
    pub fn from_environment() -> Result<Option<Self>, GitHubApiError> {
        let app_id_str = match std::env::var("GH_APP_ID") {
            Ok(v) if !v.is_empty() => v,
            _ => {
                trace!("GH_APP_ID not set, GitHub App auth not configured");
                return Ok(None);
            }
        };

        debug!(app_id = %app_id_str, "GitHub App configuration detected");
        let app_id = parse_app_id(&app_id_str)?;

        let private_key_pem = if let Ok(key) = std::env::var("GH_APP_PRIVATE_KEY") {
            if key.is_empty() {
                return Err(GitHubApiError::ClientConfigError {
                    reason: "GH_APP_PRIVATE_KEY is set but empty".to_string(),
                });
            }
            key
        } else if let Ok(path) = std::env::var("GH_APP_PRIVATE_KEY_PATH") {
            read_private_key_file(&path)?
        } else {
            return Err(GitHubApiError::ClientConfigError {
                reason:
                    "GH_APP_ID set but neither GH_APP_PRIVATE_KEY nor GH_APP_PRIVATE_KEY_PATH provided"
                        .to_string(),
            });
        };

        let installation_id = match std::env::var("GH_APP_INSTALLATION_ID") {
            Ok(v) if !v.is_empty() => {
                let id = parse_positive_u64(&v, "GH_APP_INSTALLATION_ID")?;
                debug!(installation_id = %v, "explicit installation ID configured");
                id
            }
            _ => {
                return Err(GitHubApiError::ClientConfigError {
                    reason: "GH_APP_INSTALLATION_ID is required when GH_APP_ID is set".to_string(),
                });
            }
        };

        debug!(app_id, installation_id, "GitHub App config loaded");
        Ok(Some(Self {
            app_id,
            private_key_pem: SecretString::from(private_key_pem),
            installation_id,
        }))
    }
}

/// Read a PEM private key from a file path.
///
/// Validates that the file exists, is not empty, looks like a PEM key, and
/// (on Unix) has restrictive permissions (not readable by group or other).
fn read_private_key_file(path: &str) -> Result<String, GitHubApiError> {
    let path = Path::new(path);
    if !path.exists() {
        return Err(GitHubApiError::ClientConfigError {
            reason: format!("GH_APP_PRIVATE_KEY_PATH does not exist: {}", path.display()),
        });
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(path) {
            let mode = meta.mode();
            if mode & 0o077 != 0 {
                return Err(GitHubApiError::ClientConfigError {
                    reason: format!(
                        "private key file {} has overly permissive permissions (mode {:o}); \
                         expected no group/other access (e.g., chmod 600)",
                        path.display(),
                        mode & 0o777
                    ),
                });
            }
        }
    }

    let content = std::fs::read_to_string(path).map_err(|e| GitHubApiError::ClientConfigError {
        reason: format!("failed to read private key from {}: {e}", path.display()),
    })?;

    if content.trim().is_empty() {
        return Err(GitHubApiError::ClientConfigError {
            reason: "private key file is empty".to_string(),
        });
    }

    if !content.contains("-----BEGIN") {
        return Err(GitHubApiError::ClientConfigError {
            reason: "private key file does not appear to contain a PEM-encoded key".to_string(),
        });
    }

    Ok(content)
}

/// Minimum RSA key size in bits accepted for App JWT signing.
const MIN_RSA_BITS: usize = 2048;

/// Validate that a PEM-encoded RSA private key is at least [`MIN_RSA_BITS`] bits.
///
/// Parses the PEM body and extracts the RSA modulus length from the ASN.1
/// DER encoding (`RSAPrivateKey ::= SEQUENCE { version, n, e, ... }`).
/// The modulus is the second INTEGER in the outer SEQUENCE; its byte length
/// × 8 gives an approximation within 8 bits, which is sufficient for the
/// 1024 vs 2048 boundary check.
fn validate_rsa_key_size(pem_str: &str) -> Result<(), GitHubApiError> {
    let pem_data = pem::parse(pem_str).map_err(|e| GitHubApiError::AuthenticationFailed {
        reason: format!("invalid PEM: {e}"),
    })?;
    let der = pem_data.contents();

    let modulus_bytes =
        extract_rsa_modulus_len(der).ok_or_else(|| GitHubApiError::AuthenticationFailed {
            reason: "could not parse RSA key structure to determine key size".to_string(),
        })?;
    let modulus_bits = modulus_bytes * 8;

    if modulus_bits < MIN_RSA_BITS {
        return Err(GitHubApiError::AuthenticationFailed {
            reason: format!("RSA key is {modulus_bits} bits; minimum {MIN_RSA_BITS} required"),
        });
    }
    Ok(())
}

/// Walk minimal DER to find the modulus byte length in an `RSAPrivateKey`.
///
/// `RSAPrivateKey ::= SEQUENCE { version INTEGER, modulus INTEGER, ... }`
/// Returns the byte length of the modulus INTEGER value (leading zero
/// stripped), or `None` if the structure is not recognisable.
fn extract_rsa_modulus_len(der: &[u8]) -> Option<usize> {
    let rest = skip_der_tag_len(der, 0x30)?;
    let rest = skip_der_tlv(rest)?;
    read_der_integer_value_len(rest)
}

/// If `data` starts with DER tag `expected_tag`, skip tag + length and
/// return the slice covering the value bytes.
fn skip_der_tag_len(data: &[u8], expected_tag: u8) -> Option<&[u8]> {
    if data.first()? != &expected_tag {
        return None;
    }
    let (len, hdr) = decode_der_length(&data[1..])?;
    let start = 1 + hdr;
    if data.len() < start + len {
        return None;
    }
    Some(&data[start..])
}

/// Skip one complete DER TLV (tag + length + value) and return the remainder.
fn skip_der_tlv(data: &[u8]) -> Option<&[u8]> {
    if data.is_empty() {
        return None;
    }
    let (len, hdr) = decode_der_length(&data[1..])?;
    let total = 1 + hdr + len;
    if data.len() < total {
        return None;
    }
    Some(&data[total..])
}

/// Read the value length of a DER INTEGER, stripping the leading zero byte
/// that DER uses for positive integers whose high bit is set.
fn read_der_integer_value_len(data: &[u8]) -> Option<usize> {
    if data.first()? != &0x02 {
        return None;
    }
    let (len, hdr) = decode_der_length(&data[1..])?;
    let start = 1 + hdr;
    if data.len() < start + len || len == 0 {
        return None;
    }
    let val = &data[start..start + len];
    if val[0] == 0x00 && len > 1 {
        Some(len - 1)
    } else {
        Some(len)
    }
}

/// Decode a DER length field. Returns `(length_value, header_bytes_consumed)`.
fn decode_der_length(data: &[u8]) -> Option<(usize, usize)> {
    let first = *data.first()?;
    if first < 0x80 {
        Some((first as usize, 1))
    } else {
        let num_bytes = (first & 0x7f) as usize;
        if num_bytes == 0 || num_bytes > 4 || data.len() < 1 + num_bytes {
            return None;
        }
        let mut len: usize = 0;
        for &b in &data[1..=num_bytes] {
            len = len.checked_shl(8)? | (b as usize);
        }
        Some((len, 1 + num_bytes))
    }
}

/// Generate a JWT for GitHub App authentication.
///
/// The JWT is signed with RS256 using the App's private key.
/// It is valid for 10 minutes (GitHub's maximum).
///
/// # Errors
///
/// Returns `GitHubApiError::AuthenticationFailed` if the private key is invalid or JWT signing fails.
///
/// # Security
/// - The private key is only accessed via `expose_secret()`
/// - The JWT itself has a short lifetime (10 minutes)
/// - The JWT cannot be used directly for API calls; it must be exchanged
///   for an installation token
pub fn generate_app_jwt(app_config: &GitHubAppConfig) -> Result<String, GitHubApiError> {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};

    debug!(app_id = app_config.app_id, "generating GitHub App JWT");

    validate_rsa_key_size(app_config.private_key_pem.expose_secret())?;

    let now = Timestamp::now();
    let iat = (now - SignedDuration::from_secs(60)).as_second();
    let exp = (now + SignedDuration::from_mins(10)).as_second();

    let claims = AppJwtClaims {
        iat,
        exp,
        iss: app_config.app_id.to_string(),
    };

    let header = Header::new(Algorithm::RS256);
    let encoding_key = EncodingKey::from_rsa_pem(
        app_config.private_key_pem.expose_secret().as_bytes(),
    )
    .map_err(|e| GitHubApiError::AuthenticationFailed {
        reason: format!("invalid RSA private key: {e}"),
    })?;

    jsonwebtoken::encode(&header, &claims, &encoding_key).map_err(|e| {
        GitHubApiError::AuthenticationFailed {
            reason: format!("JWT signing failed: {e}"),
        }
    })
}

/// JWT claims for GitHub App authentication.
#[derive(Debug, Serialize, Deserialize)]
struct AppJwtClaims {
    /// Issued-at time (Unix timestamp).
    iat: i64,
    /// Expiration time (Unix timestamp).
    exp: i64,
    /// Issuer (the GitHub App ID).
    iss: String,
}

/// Response from the installation token exchange endpoint.
///
/// The `token` field is a raw `String` here because it arrives via
/// deserialization. It is immediately wrapped in `SecretString` after
/// parsing and must never be logged. The manual `Debug` impl redacts it.
#[derive(Deserialize)]
pub struct InstallationTokenResponse {
    pub token: String,
    pub expires_at: String,
}

impl std::fmt::Debug for InstallationTokenResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstallationTokenResponse")
            .field("token", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Represents the availability of a specific GitHub API capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// The API endpoint is accessible and returned a successful response.
    Available,
    /// The API endpoint returned a permission error (403/404).
    PermissionDenied,
    /// The API endpoint is not available (server error, timeout, etc.).
    Unavailable,
    /// Probe was not performed.
    NotProbed,
}

/// Set of GitHub API capabilities probed at startup.
///
/// Mandatory capabilities must be available for a valid run.
/// Optional capabilities degrade to `Unknown` check results when unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilitySet {
    /// Repository inventory listing — MANDATORY.
    pub repos_list: CapabilityStatus,
    /// Organization secret scanning alerts — optional.
    pub org_secret_scanning_alerts: CapabilityStatus,
}

impl Default for CapabilitySet {
    fn default() -> Self {
        Self {
            repos_list: CapabilityStatus::NotProbed,
            org_secret_scanning_alerts: CapabilityStatus::NotProbed,
        }
    }
}

impl CapabilitySet {
    /// Whether the mandatory capabilities for a valid run are available.
    #[must_use]
    pub fn can_run(&self) -> bool {
        self.repos_list == CapabilityStatus::Available
    }

    /// Whether a specific optional capability is available.
    #[must_use]
    pub fn is_available(&self, capability: Capability) -> bool {
        match capability {
            Capability::OrgSecretScanningAlerts => {
                self.org_secret_scanning_alerts == CapabilityStatus::Available
            }
        }
    }

    /// Return a list of capabilities that are not available (for observability).
    #[must_use]
    pub fn unavailable_capabilities(&self) -> Vec<Capability> {
        let mut unavail = Vec::new();
        if self.org_secret_scanning_alerts != CapabilityStatus::Available {
            unavail.push(Capability::OrgSecretScanningAlerts);
        }
        unavail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_does_not_expose_token() {
        let cred = GitHubCredential {
            mode: AuthMode::Pat,
            token: SecretString::from("ghp_supersecretvalue12345".to_string()),
            expires_at: None,
        };
        let debug_output = format!("{cred:?}");
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("supersecret"));
        assert!(!debug_output.contains("ghp_"));
    }

    #[test]
    fn installation_token_response_debug_does_not_expose_token() {
        let resp = InstallationTokenResponse {
            token: "ghs_supersecretinstallationtoken123".to_string(),
            expires_at: "2026-04-09T12:00:00Z".to_string(),
        };
        let debug_output = format!("{resp:?}");
        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("supersecret"));
        assert!(!debug_output.contains("ghs_"));
    }

    #[test]
    fn parse_oauth_scopes_basic() {
        let scopes = parse_oauth_scopes("repo, read:org, security_events");
        assert_eq!(scopes, vec!["repo", "read:org", "security_events"]);
    }

    #[test]
    fn parse_oauth_scopes_empty() {
        let scopes = parse_oauth_scopes("");
        assert!(scopes.is_empty());
    }

    #[test]
    fn parse_oauth_scopes_extra_whitespace() {
        let scopes = parse_oauth_scopes("  repo ,  read:org  , security_events  ");
        assert_eq!(scopes, vec!["repo", "read:org", "security_events"]);
    }

    #[test]
    fn classify_full_tier() {
        let scopes: Vec<String> = vec![
            "repo".to_string(),
            "read:org".to_string(),
            "security_events".to_string(),
        ];
        assert_eq!(classify_token_tier(&scopes), TokenTier::Full);
    }

    #[test]
    fn classify_full_tier_with_extras() {
        let scopes: Vec<String> = vec![
            "repo".to_string(),
            "read:org".to_string(),
            "security_events".to_string(),
            "admin:repo_hook".to_string(),
        ];
        assert_eq!(classify_token_tier(&scopes), TokenTier::Full);
    }

    #[test]
    fn classify_full_tier_repo_implies_security_events() {
        let scopes: Vec<String> = vec!["repo".to_string(), "read:org".to_string()];
        assert_eq!(classify_token_tier(&scopes), TokenTier::Full);
    }

    #[test]
    fn classify_limited_tier() {
        let scopes: Vec<String> = vec!["repo".to_string()];
        assert_eq!(classify_token_tier(&scopes), TokenTier::Limited);
    }

    #[test]
    fn classify_unknown_tier_empty() {
        let scopes: Vec<String> = vec![];
        assert_eq!(classify_token_tier(&scopes), TokenTier::Unknown);
    }

    #[test]
    fn auth_metadata_from_scopes() {
        let scopes: Vec<String> = vec![
            "repo".to_string(),
            "read:org".to_string(),
            "security_events".to_string(),
        ];
        let meta = AuthMetadata::from_scopes(&scopes, &AuthMode::Pat);
        assert_eq!(meta.token_tier, TokenTier::Full);
        assert_eq!(meta.token_scopes, "repo, read:org, security_events");
    }

    #[test]
    fn auth_metadata_for_github_app() {
        let meta = AuthMetadata::for_github_app();
        assert_eq!(meta.token_tier, TokenTier::Unknown);
        assert_eq!(meta.token_scopes, "github-app-installation");
    }

    #[test]
    fn parse_gh_auth_status_full_scopes() {
        let json = r#"{
            "hosts": {
                "github.com": [
                    {
                        "active": true,
                        "scopes": "repo, read:org, security_events"
                    }
                ]
            }
        }"#;
        let meta = parse_gh_auth_status_json(json);
        assert_eq!(meta.token_tier, TokenTier::Full);
        assert_eq!(meta.token_scopes, "repo, read:org, security_events");
    }

    #[test]
    fn parse_gh_auth_status_limited_scopes() {
        let json = r#"{
            "hosts": {
                "github.com": [
                    {
                        "active": true,
                        "scopes": "repo"
                    }
                ]
            }
        }"#;
        let meta = parse_gh_auth_status_json(json);
        assert_eq!(meta.token_tier, TokenTier::Limited);
    }

    #[test]
    fn parse_gh_auth_status_no_active_uses_first() {
        let json = r#"{
            "hosts": {
                "github.com": [
                    {
                        "active": false,
                        "scopes": "repo, read:org, security_events"
                    }
                ]
            }
        }"#;
        let meta = parse_gh_auth_status_json(json);
        assert_eq!(meta.token_tier, TokenTier::Full);
    }

    #[test]
    fn parse_gh_auth_status_empty_hosts() {
        let json = r#"{"hosts": {}}"#;
        let meta = parse_gh_auth_status_json(json);
        assert_eq!(meta.token_tier, TokenTier::Unknown);
        assert_eq!(meta.token_scopes, "unknown");
    }

    #[test]
    fn parse_gh_auth_status_invalid_json() {
        let meta = parse_gh_auth_status_json("not json");
        assert_eq!(meta.token_tier, TokenTier::Unknown);
    }

    #[test]
    fn parse_gh_auth_status_unknown_scopes() {
        let json = r#"{
            "hosts": {
                "github.com": [
                    {
                        "active": true,
                        "scopes": "unknown"
                    }
                ]
            }
        }"#;
        let meta = parse_gh_auth_status_json(json);
        assert_eq!(meta.token_tier, TokenTier::Unknown);
        assert_eq!(meta.token_scopes, "unknown");
    }

    #[test]
    fn pat_never_needs_refresh() {
        let cred = GitHubCredential {
            mode: AuthMode::Pat,
            token: SecretString::from("ghp_test"),
            expires_at: None,
        };
        assert!(!cred.needs_refresh(Duration::from_mins(5)));
    }

    #[test]
    fn app_token_needs_refresh_when_near_expiry() {
        let cred = GitHubCredential {
            mode: AuthMode::GitHubApp,
            token: SecretString::from("ghs_test"),
            expires_at: Some(Timestamp::now() + SignedDuration::from_secs(60)),
        };
        assert!(cred.needs_refresh(Duration::from_mins(5)));
    }

    #[test]
    fn app_token_does_not_need_refresh_when_far_from_expiry() {
        let cred = GitHubCredential {
            mode: AuthMode::GitHubApp,
            token: SecretString::from("ghs_test"),
            expires_at: Some(Timestamp::now() + SignedDuration::from_mins(30)),
        };
        assert!(!cred.needs_refresh(Duration::from_mins(5)));
    }

    #[test]
    fn capability_set_default_cannot_run() {
        let caps = CapabilitySet::default();
        assert!(!caps.can_run());
    }

    #[test]
    fn capability_set_can_run_with_repos() {
        let caps = CapabilitySet {
            repos_list: CapabilityStatus::Available,
            ..Default::default()
        };
        assert!(caps.can_run());
    }

    #[test]
    fn capability_set_unavailable_list() {
        let caps = CapabilitySet {
            repos_list: CapabilityStatus::Available,
            org_secret_scanning_alerts: CapabilityStatus::Unavailable,
        };
        let unavail = caps.unavailable_capabilities();
        assert_eq!(unavail, vec![Capability::OrgSecretScanningAlerts,]);
    }

    #[test]
    fn capability_is_available_checks() {
        let caps = CapabilitySet {
            repos_list: CapabilityStatus::Available,
            ..Default::default()
        };
        assert!(!caps.is_available(Capability::OrgSecretScanningAlerts));
    }

    #[test]
    fn read_private_key_file_nonexistent() {
        let result = read_private_key_file("/nonexistent/path/key.pem");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("does not exist"));
    }

    #[cfg(unix)]
    #[test]
    fn read_private_key_file_rejects_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test.pem");
        std::fs::write(
            &key_path,
            "-----BEGIN RSA PRIVATE KEY-----\nfake\n-----END RSA PRIVATE KEY-----\n",
        )
        .unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let result = read_private_key_file(key_path.to_str().unwrap());
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("overly permissive"));
    }

    #[cfg(unix)]
    #[test]
    fn read_private_key_file_accepts_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("test.pem");
        std::fs::write(
            &key_path,
            "-----BEGIN RSA PRIVATE KEY-----\nfake\n-----END RSA PRIVATE KEY-----\n",
        )
        .unwrap();
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let result = read_private_key_file(key_path.to_str().unwrap());
        assert!(result.is_ok());
    }

    #[test]
    fn parse_app_id_valid() {
        assert_eq!(parse_app_id("12345").unwrap(), 12345);
        assert_eq!(parse_app_id("1").unwrap(), 1);
        assert_eq!(parse_app_id(" 42 ").unwrap(), 42);
    }

    #[test]
    fn parse_app_id_zero_rejected() {
        let err = parse_app_id("0").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("positive integer"),
            "expected positive integer error, got: {msg}"
        );
    }

    #[test]
    fn parse_app_id_non_numeric_rejected() {
        assert!(parse_app_id("abc").is_err());
        assert!(parse_app_id("").is_err());
        assert!(parse_app_id("-1").is_err());
    }

    #[test]
    fn installation_id_zero_rejected() {
        let err = parse_positive_u64("0", "GH_APP_INSTALLATION_ID").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("positive integer"),
            "expected positive integer error, got: {msg}"
        );
    }

    #[test]
    fn installation_id_valid() {
        assert_eq!(
            parse_positive_u64("98765", "GH_APP_INSTALLATION_ID").unwrap(),
            98765
        );
    }

    #[test]
    fn auth_mode_format_pinning_pat() {
        let scopes = vec!["repo".to_string()];
        let meta = AuthMetadata::from_scopes(&scopes, &AuthMode::Pat);
        assert_eq!(
            meta.auth_mode,
            AuthMode::Pat,
            "auth_mode for Pat changed unexpectedly"
        );
    }

    #[test]
    fn auth_mode_format_pinning_github_app() {
        let meta = AuthMetadata::for_github_app();
        assert_eq!(
            meta.auth_mode,
            AuthMode::GitHubApp,
            "auth_mode for GitHubApp changed unexpectedly"
        );
    }

    #[test]
    fn auth_mode_format_pinning_gh_cli_fallback() {
        let json = r#"{
            "hosts": {
                "github.com": [
                    {
                        "active": true,
                        "scopes": "repo, read:org, security_events"
                    }
                ]
            }
        }"#;
        let meta = parse_gh_auth_status_json(json);
        assert_eq!(
            meta.auth_mode,
            AuthMode::GhCliFallback,
            "auth_mode for GhCliFallback changed unexpectedly"
        );
    }

    #[test]
    fn auth_mode_serde_round_trip() {
        for (variant, expected_json) in [
            (AuthMode::Pat, "\"pat\""),
            (AuthMode::GitHubApp, "\"github_app\""),
            (AuthMode::GhCliFallback, "\"gh_cli_fallback\""),
            (AuthMode::Unknown, "\"unknown\""),
        ] {
            let serialized = serde_json::to_string(&variant).unwrap();
            assert_eq!(serialized, expected_json, "wire format for {variant:?}");
            let deserialized: AuthMode = serde_json::from_str(&serialized).unwrap();
            assert_eq!(deserialized, variant, "round-trip for {variant:?}");
        }
    }

    #[test]
    fn generate_app_jwt_rejects_1024_bit_key() {
        const WEAK_PEM: &str = "\
-----BEGIN RSA PRIVATE KEY-----
MIICXAIBAAKBgQDOv5aACQGdS/rWzmloZlIOrxW5S7MLHMhdAXdHAZB+EOg6WX4G
QERqYXv62JsUOwNAL3bt855giRBHGbXoJzXtsZmqDdAqVLmM1pnDKDjyQFqPZY7s
smEC0uFaH+eeIlb0iO+lRm6cIT6dry1zwWBJY9KGrrNMz8BtTboWjXgHoQIDAQAB
AoGBAJtDlEGpAdZgFgu1TcHCfcNbR2Q1bktdHTeDf1EK4rlZ9xzC0nrdTsPZW+NB
Qg1KWCGew6DlgL4ckOXkcBDdSYhS1Feu0Qsw0oEjw8lZ8LPSWQUQE8fXWBGgxp3p
WeE/OPAKS8QydQRb0QL3NBBdlkexCjnEHcaPxj7rJ1iQqGDBAkEA9IoFNZq5RY2J
S27iECYRLkP8kD2W0xpABF88GFGMejjRXPCFLJSinsMDSeBAWAKeBs+hjwLL5TjC
pE340HERaQJBANhwJ30Z+63BZlqwuPYGVaKPNXIjHvWzSho+0Tr2SZ5ehltwp6z9
NdOeTu/k6KB7e5DEJp+5xOAfclqecRN5xXkCQFFalY8W0WplQvbYhdbPg0m8DotC
IipLAl8x+8EvaCfFPUnJLtT9AfkFcdOjCmT9QeuMKfh0+rZgosictBlMdHkCQGr0
sW7u4iJxSiVS43QgmTzlzCGFHY2JdfsWQ8sBXkv2piqVtyaTUoAq4RNHaXW0z9Ew
PW39HT8sCxSg63wWVvECQCLl3C31DShFhoPUeq2+2NYJVn3sK0X6y9dTlt0b8gBh
kZ6CvXp9LqBAJ43EJbheiVarZ2w570jbUZPsXx7UnvU=
-----END RSA PRIVATE KEY-----";
        let cfg = GitHubAppConfig {
            app_id: 1,
            private_key_pem: SecretString::from(WEAK_PEM.to_string()),
            installation_id: 12345,
        };
        let err = generate_app_jwt(&cfg).expect_err("1024-bit key must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("rsa") && msg.contains("2048"),
            "expected rsa/2048 key-size error, got: {msg}"
        );
    }

    #[test]
    fn generate_app_jwt_accepts_2048_bit_key() {
        const STRONG_PEM: &str = "\
-----BEGIN RSA PRIVATE KEY-----
MIIEpQIBAAKCAQEA1/aBChcJ0EuJWsjHw8Dp0091xfQWMnnL/sbTJN/UuwLy/hv4
LYmH2LNhPrVGlLOLea0m41pAL9yIYFYUJI0L4Vfn0UVPJFcjTEb+YZPCP16t5SER
dwAookTBOnwBFCHU1npVk7cz/ryVoOM4+odnW1zDZAK4FwsZutG3DvjJiU69VnsA
6plT8jdouhxlpbP9G41TP/l/f3rUbS+T7E06fy3xrj6a7opdSnTLhO2Pb6M/Aohr
ibLRD6CfI2UnkkWz6xHbQjpiUnt0fMJgA+8H928s79F/NfzU49ZHvRLqYuw36uP1
ISZ5/mUw5fT9wfvA1GC+iZtkqlX3nOcBZUnZkwIDAQABAoIBAQCa9ey3gbpv9JN1
SdZVNupQzpZSWQdIZq6ifKXqspUhL0eOYCFfA20vZ98iMM6ZSo+M2lqqDgs6jIJq
pblEVNSud/YF6jaUe9X/GH3VJEHgWJ5sZ6LxgXKmpLEFtw7LFE91KkiXeoBbi5PN
4tzynw/htZkZ/P18w2FN9MbmfkuWMlCwqJadlovRXELcSa4iq1KbXp2SZeYt636W
enpE3Och1f9xnndXgHWG/WoVrHkvfgzHtU/yh0gaOwEPMev7mvwunCd8a4vG2vHF
fSVRTtO8YneTU//DZ1uDNjYgIaCT7cN9eRDISCz4ciUqAsWQpoX23rPmaYYGS1iy
AkyfPZOJAoGBAPfXRPhlV9X0+4MwJPYtw9N7NQ8e7K1aAHu7E8uDKEbzzlmUPKSo
xT7EaAH651N7ENY5PVc8kAqkv0c4LaZfFEhL8DLdFFe9Au/LV88TfmKiHMZ6aRcO
LHa/5jXOF4+OYRARE/XFJwjV1uw48DJrZdt5voDmBu1h4CW3t/I9TPMdAoGBAN8S
k4aKa9UrJF3paAH+telGiQyiP3Zj5kvnFpzNmZTuPruwxv5mhYuJGU7PpL1sNMAx
8OyubwN9brnIb43jCXwplEjH0cCOLAiMNSQCbm4PYgCw9KcGa5nzFeoPePPxVaaO
fjDABkXTsgVDqbx8JIG6cWeZnB4icBv6whGSAzBvAoGBAI5w+MDSbhMYA924M+YR
E3VeYHZaTaisC48RTCUxMlrlEPnHCruQDB0xAJ3yuDTwjBKzPx/+PMMBQLYMAaCX
EK8khd6V1XU/uopbEhJ/n6nMhkFEZVXM3Z06WXMfCceGCx8S0af1MaQQUr/dUZ+I
vjfP1r96dQzFre+/kUb2GF25AoGBAMNei6JL3UFndYRihdspb70NL77G4voXaH2V
uPJAB4CuYHcVzlLFC7U3r9icd1YHTPP/SVihNU1DMBS6fSkxbP83k01i5EvWuK4L
zgbpsjnmcxjT4pHeR6MfiVPjlTVhanhjWBXuOBAz5jhCGIih2X9dATGREXA7DSEU
L6Af13c1AoGAWRlWbo5MEvXKNCp5ogRD5/6HzZgb39y0Rdhgw5CfRPwGaKsvdCLX
SdVk3bZS9t4ctyrsRFdr7KY3NF25gXDzmB4SFW5bIUl9uDtW4FU5pFDc8ye6vvTB
XkeSUmcjZbzD4mjVTjw9nFxys8W5RtYUHJgUF/Sve01bwRCiwLrWMX0=
-----END RSA PRIVATE KEY-----";
        let cfg = GitHubAppConfig {
            app_id: 1,
            private_key_pem: SecretString::from(STRONG_PEM.to_string()),
            installation_id: 12345,
        };
        let _jwt = generate_app_jwt(&cfg).expect("2048-bit key must sign");
    }

    #[test]
    fn validate_rsa_key_size_rejects_weak_key_directly() {
        const WEAK_PEM: &str = "\
-----BEGIN RSA PRIVATE KEY-----
MIICXAIBAAKBgQDOv5aACQGdS/rWzmloZlIOrxW5S7MLHMhdAXdHAZB+EOg6WX4G
QERqYXv62JsUOwNAL3bt855giRBHGbXoJzXtsZmqDdAqVLmM1pnDKDjyQFqPZY7s
smEC0uFaH+eeIlb0iO+lRm6cIT6dry1zwWBJY9KGrrNMz8BtTboWjXgHoQIDAQAB
AoGBAJtDlEGpAdZgFgu1TcHCfcNbR2Q1bktdHTeDf1EK4rlZ9xzC0nrdTsPZW+NB
Qg1KWCGew6DlgL4ckOXkcBDdSYhS1Feu0Qsw0oEjw8lZ8LPSWQUQE8fXWBGgxp3p
WeE/OPAKS8QydQRb0QL3NBBdlkexCjnEHcaPxj7rJ1iQqGDBAkEA9IoFNZq5RY2J
S27iECYRLkP8kD2W0xpABF88GFGMejjRXPCFLJSinsMDSeBAWAKeBs+hjwLL5TjC
pE340HERaQJBANhwJ30Z+63BZlqwuPYGVaKPNXIjHvWzSho+0Tr2SZ5ehltwp6z9
NdOeTu/k6KB7e5DEJp+5xOAfclqecRN5xXkCQFFalY8W0WplQvbYhdbPg0m8DotC
IipLAl8x+8EvaCfFPUnJLtT9AfkFcdOjCmT9QeuMKfh0+rZgosictBlMdHkCQGr0
sW7u4iJxSiVS43QgmTzlzCGFHY2JdfsWQ8sBXkv2piqVtyaTUoAq4RNHaXW0z9Ew
PW39HT8sCxSg63wWVvECQCLl3C31DShFhoPUeq2+2NYJVn3sK0X6y9dTlt0b8gBh
kZ6CvXp9LqBAJ43EJbheiVarZ2w570jbUZPsXx7UnvU=
-----END RSA PRIVATE KEY-----";
        let err = validate_rsa_key_size(WEAK_PEM).expect_err("1024-bit key must be rejected");
        let msg = format!("{err}");
        assert!(msg.contains("1024") && msg.contains("2048"), "got: {msg}");
    }

    #[test]
    fn validate_rsa_key_size_accepts_strong_key_directly() {
        const STRONG_PEM: &str = "\
-----BEGIN RSA PRIVATE KEY-----
MIIEpQIBAAKCAQEA1/aBChcJ0EuJWsjHw8Dp0091xfQWMnnL/sbTJN/UuwLy/hv4
LYmH2LNhPrVGlLOLea0m41pAL9yIYFYUJI0L4Vfn0UVPJFcjTEb+YZPCP16t5SER
dwAookTBOnwBFCHU1npVk7cz/ryVoOM4+odnW1zDZAK4FwsZutG3DvjJiU69VnsA
6plT8jdouhxlpbP9G41TP/l/f3rUbS+T7E06fy3xrj6a7opdSnTLhO2Pb6M/Aohr
ibLRD6CfI2UnkkWz6xHbQjpiUnt0fMJgA+8H928s79F/NfzU49ZHvRLqYuw36uP1
ISZ5/mUw5fT9wfvA1GC+iZtkqlX3nOcBZUnZkwIDAQABAoIBAQCa9ey3gbpv9JN1
SdZVNupQzpZSWQdIZq6ifKXqspUhL0eOYCFfA20vZ98iMM6ZSo+M2lqqDgs6jIJq
pblEVNSud/YF6jaUe9X/GH3VJEHgWJ5sZ6LxgXKmpLEFtw7LFE91KkiXeoBbi5PN
4tzynw/htZkZ/P18w2FN9MbmfkuWMlCwqJadlovRXELcSa4iq1KbXp2SZeYt636W
enpE3Och1f9xnndXgHWG/WoVrHkvfgzHtU/yh0gaOwEPMev7mvwunCd8a4vG2vHF
fSVRTtO8YneTU//DZ1uDNjYgIaCT7cN9eRDISCz4ciUqAsWQpoX23rPmaYYGS1iy
AkyfPZOJAoGBAPfXRPhlV9X0+4MwJPYtw9N7NQ8e7K1aAHu7E8uDKEbzzlmUPKSo
xT7EaAH651N7ENY5PVc8kAqkv0c4LaZfFEhL8DLdFFe9Au/LV88TfmKiHMZ6aRcO
LHa/5jXOF4+OYRARE/XFJwjV1uw48DJrZdt5voDmBu1h4CW3t/I9TPMdAoGBAN8S
k4aKa9UrJF3paAH+telGiQyiP3Zj5kvnFpzNmZTuPruwxv5mhYuJGU7PpL1sNMAx
8OyubwN9brnIb43jCXwplEjH0cCOLAiMNSQCbm4PYgCw9KcGa5nzFeoPePPxVaaO
fjDABkXTsgVDqbx8JIG6cWeZnB4icBv6whGSAzBvAoGBAI5w+MDSbhMYA924M+YR
E3VeYHZaTaisC48RTCUxMlrlEPnHCruQDB0xAJ3yuDTwjBKzPx/+PMMBQLYMAaCX
EK8khd6V1XU/uopbEhJ/n6nMhkFEZVXM3Z06WXMfCceGCx8S0af1MaQQUr/dUZ+I
vjfP1r96dQzFre+/kUb2GF25AoGBAMNei6JL3UFndYRihdspb70NL77G4voXaH2V
uPJAB4CuYHcVzlLFC7U3r9icd1YHTPP/SVihNU1DMBS6fSkxbP83k01i5EvWuK4L
zgbpsjnmcxjT4pHeR6MfiVPjlTVhanhjWBXuOBAz5jhCGIih2X9dATGREXA7DSEU
L6Af13c1AoGAWRlWbo5MEvXKNCp5ogRD5/6HzZgb39y0Rdhgw5CfRPwGaKsvdCLX
SdVk3bZS9t4ctyrsRFdr7KY3NF25gXDzmB4SFW5bIUl9uDtW4FU5pFDc8ye6vvTB
XkeSUmcjZbzD4mjVTjw9nFxys8W5RtYUHJgUF/Sve01bwRCiwLrWMX0=
-----END RSA PRIVATE KEY-----";
        validate_rsa_key_size(STRONG_PEM).expect("2048-bit key must be accepted");
    }
}
