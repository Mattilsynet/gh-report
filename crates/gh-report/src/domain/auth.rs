//! Authentication-related domain types.
//!
//! Pure value enums that describe token capabilities, API feature tiers,
//! and authentication modes. These live in `domain` because they are
//! serialized into evidence artifacts and have no dependency on GitHub
//! API infrastructure.
//!
//! For runtime classification logic (scope parsing, capability probing),
//! see [`crate::github::auth`].

use serde::{Deserialize, Serialize};

/// Supported authentication modes.
///
/// Describes how the application authenticated with the GitHub API.
/// Serialized into evidence artifacts for audit trail purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AuthMode {
    /// GitHub Personal Access Token (classic or fine-grained).
    #[serde(rename = "pat")]
    Pat,
    /// GitHub App installation authentication.
    #[serde(rename = "github_app")]
    GitHubApp,
    /// Local `gh auth` developer fallback.
    #[serde(rename = "gh_cli_fallback")]
    GhCliFallback,
    /// Unknown or not yet determined (used as default before credential discovery).
    #[serde(rename = "unknown")]
    Unknown,
}

impl std::fmt::Display for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pat => write!(f, "pat"),
            Self::GitHubApp => write!(f, "github_app"),
            Self::GhCliFallback => write!(f, "gh_cli_fallback"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Token capability tier based on available OAuth scopes.
///
/// - Full: {repo, read:org, `security_events`} all present
/// - Limited: Some scopes present but not the full set
/// - Unknown: Unable to determine (e.g. GitHub App, fine-grained PAT, or unavailable)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenTier {
    /// All required scopes present: repo, read:org, `security_events`.
    Full,
    /// Some scopes present but not the full required set.
    Limited,
    /// Unable to determine (e.g., GitHub App, fine-grained PAT).
    Unknown,
}

impl std::fmt::Display for TokenTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "Full"),
            Self::Limited => write!(f, "Limited"),
            Self::Unknown => write!(f, "Unknown"),
        }
    }
}

/// An optional GitHub API capability that can be probed at startup.
///
/// Each variant maps to a specific API family. Using an enum instead of
/// string keys ensures typos are caught at compile time — a misspelled
/// capability can never silently skip a security check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Organization-level secret scanning alerts.
    OrgSecretScanningAlerts,
}

impl Capability {
    /// All optional capabilities, for iteration.
    pub const ALL: &[Capability] = &[Capability::OrgSecretScanningAlerts];
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OrgSecretScanningAlerts => write!(f, "org_secret_scanning_alerts"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_tier_serde_wire_format() {
        assert_eq!(serde_json::to_string(&TokenTier::Full).unwrap(), "\"full\"");
        assert_eq!(
            serde_json::to_string(&TokenTier::Limited).unwrap(),
            "\"limited\""
        );
        assert_eq!(
            serde_json::to_string(&TokenTier::Unknown).unwrap(),
            "\"unknown\""
        );
    }

    #[test]
    fn capability_serde_wire_format() {
        assert_eq!(
            serde_json::to_string(&Capability::OrgSecretScanningAlerts).unwrap(),
            "\"org_secret_scanning_alerts\""
        );
    }

    #[test]
    fn token_tier_display() {
        assert_eq!(TokenTier::Full.to_string(), "Full");
        assert_eq!(TokenTier::Limited.to_string(), "Limited");
        assert_eq!(TokenTier::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn capability_display_matches_serde() {
        // Verify that Display output matches serde serialization for all variants.
        for cap in Capability::ALL {
            let display = cap.to_string();
            let serde_str = serde_json::to_value(cap).unwrap();
            assert_eq!(
                display,
                serde_str.as_str().unwrap(),
                "Display and serde mismatch for {cap:?}"
            );
        }
    }

    #[test]
    fn capability_display() {
        assert_eq!(
            Capability::OrgSecretScanningAlerts.to_string(),
            "org_secret_scanning_alerts"
        );
    }

    #[test]
    fn auth_mode_serde_wire_format() {
        assert_eq!(serde_json::to_string(&AuthMode::Pat).unwrap(), "\"pat\"");
        assert_eq!(
            serde_json::to_string(&AuthMode::GitHubApp).unwrap(),
            "\"github_app\""
        );
        assert_eq!(
            serde_json::to_string(&AuthMode::GhCliFallback).unwrap(),
            "\"gh_cli_fallback\""
        );
        assert_eq!(
            serde_json::to_string(&AuthMode::Unknown).unwrap(),
            "\"unknown\""
        );
    }

    #[test]
    fn auth_mode_display() {
        assert_eq!(AuthMode::Pat.to_string(), "pat");
        assert_eq!(AuthMode::GitHubApp.to_string(), "github_app");
        assert_eq!(AuthMode::GhCliFallback.to_string(), "gh_cli_fallback");
        assert_eq!(AuthMode::Unknown.to_string(), "unknown");
    }
}
