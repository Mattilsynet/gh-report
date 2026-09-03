//! Organization-derived configuration seam (UF2-GEN).
//!
//! `gh-report` is a general GitHub-org tool; any deployment's own
//! organization is a configuration value, never a literal baked into
//! rendering code or templates. Remediation copy that names a specific
//! organization's internal process (who to contact, how membership is
//! governed, reference links) belongs here. Defaults are fully generic —
//! a fresh deployment renders no assumptions about how any particular
//! organization runs its GitHub access process; a deployment that has a
//! documented self-service path supplies it through this seam.

/// A labelled reference link (e.g., an internal self-service guide).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpLink {
    /// Link text shown to the reader.
    pub label: String,
    /// Target URL.
    pub url: String,
}

/// Org-specific guidance for the "add a team member" remediation (UF2-1).
///
/// All fields are optional; absent fields fall back to
/// organization-agnostic phrasing (see
/// [`crate::report::view_model::compose_team_access_guidance`]).
#[derive(Debug, Clone, Default)]
pub struct TeamAccessGuidance {
    /// Who to contact (e.g., `"#plattform on the Acme Slack"`).
    pub contact: Option<String>,
    /// How membership is actually granted (e.g., `"an Azure AD security
    /// group"`).
    pub governance_model: Option<String>,
    /// Reference links for the self-service path.
    pub help_links: Vec<HelpLink>,
}

/// Organization-derived help/remediation configuration (UF2-GEN seam).
///
/// Extend this struct — not rendering code or templates — when a future
/// remediation needs another organization-specific value.
#[derive(Debug, Clone, Default)]
pub struct OrgHelpConfig {
    /// Guidance for the "add a team member" remediation (UF2-1).
    pub team_access: TeamAccessGuidance,
}

#[cfg(test)]
mod tests {
    use super::{OrgHelpConfig, TeamAccessGuidance};

    #[test]
    fn default_org_help_config_carries_no_org_specifics() {
        let cfg = OrgHelpConfig::default();

        assert!(cfg.team_access.contact.is_none());
        assert!(cfg.team_access.governance_model.is_none());
        assert!(cfg.team_access.help_links.is_empty());
    }

    #[test]
    fn team_access_guidance_accepts_org_supplied_values() {
        let cfg = TeamAccessGuidance {
            contact: Some("#platform on the Acme Slack".to_string()),
            governance_model: Some("an Acme identity-provider group".to_string()),
            help_links: vec![super::HelpLink {
                label: "Acme access guide".to_string(),
                url: "https://example.com/access".to_string(),
            }],
        };

        assert_eq!(cfg.contact.as_deref(), Some("#platform on the Acme Slack"));
        assert_eq!(cfg.help_links.len(), 1);
    }
}
