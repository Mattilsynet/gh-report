//! GitHub API response DTOs.
//!
//! These are raw API response shapes, kept separate from internal domain models.

use serde::{Deserialize, Serialize};

/// Repository response from the GitHub REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GhRepository {
    /// The unique identifier of the repository.
    pub id: Option<u64>,
    /// The GraphQL global node ID of the repository.
    pub node_id: Option<String>,
    /// The name of the repository (without the owner).
    pub name: String,
    /// The visibility of the repository (`"public"`, `"private"`, or `"internal"`).
    pub visibility: Option<String>,
    /// Whether the repository is private.
    pub private: Option<bool>,
    /// The primary programming language of the repository.
    pub language: Option<String>,
    /// The default branch of the repository.
    pub default_branch: Option<String>,
    /// Whether the repository is archived and read-only.
    pub archived: Option<bool>,
    /// Whether the repository has issues enabled.
    pub has_issues: Option<bool>,
    /// ISO 8601 timestamp of last update (settings change or push).
    pub updated_at: Option<String>,
    /// ISO 8601 timestamp of the last git push (reflects actual code activity,
    /// unlike `updated_at` which also changes on settings modifications).
    pub pushed_at: Option<String>,
    /// ISO 8601 timestamp of when the repository was created.
    pub created_at: Option<String>,
    /// Short description of the repository.
    pub description: Option<String>,
    /// Whether this repository is a fork of another repository.
    pub fork: Option<bool>,
    /// Browser URL for the repository (e.g., `https://github.com/org/repo`).
    pub html_url: Option<String>,
    /// Topic tags applied to the repository.
    #[serde(default)]
    pub topics: Option<Vec<String>>,
    /// License information, if detected by GitHub.
    pub license: Option<LicenseInfo>,
    /// Whether the repository has a security policy enabled.
    #[serde(rename = "is_security_policy_enabled")]
    pub is_security_policy_enabled: Option<bool>,
    /// Security and analysis settings for the repository.
    pub security_and_analysis: Option<SecurityAndAnalysis>,
}

/// License information from the GitHub REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseInfo {
    /// SPDX identifier (e.g., `"MIT"`, `"Apache-2.0"`).
    pub spdx_id: Option<String>,
    /// Human-readable license name (e.g., `"MIT License"`).
    pub name: Option<String>,
}

/// Security and analysis settings from repository details.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityAndAnalysis {
    pub secret_scanning: Option<FeatureStatus>,
    pub dependabot_security_updates: Option<FeatureStatus>,
}

/// Generic feature status (enabled/disabled).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureStatus {
    pub status: Option<String>,
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Full GitHub API–style payload with all fields populated.
    fn full_json() -> serde_json::Value {
        serde_json::json!({
            "id": 123_456,
            "node_id": "R_kgDOABC",
            "name": "my-repo",
            "visibility": "public",
            "private": false,
            "language": "Rust",
            "default_branch": "main",
            "archived": false,
            "has_issues": true,
            "updated_at": "2026-04-01T12:00:00Z",
            "pushed_at": "2026-04-01T11:00:00Z",
            "created_at": "2024-01-01T00:00:00Z",
            "description": "A test repository",
            "fork": false,
            "html_url": "https://github.com/org/my-repo",
            "topics": ["security", "ci"],
            "license": { "spdx_id": "MIT", "name": "MIT License" },
            "is_security_policy_enabled": true,
            "security_and_analysis": {
                "secret_scanning": { "status": "enabled" },
                "dependabot_security_updates": { "status": "enabled" }
            }
        })
    }

    #[test]
    fn round_trip_all_fields() {
        let repo: GhRepository = serde_json::from_value(full_json()).unwrap();
        assert_eq!(repo.id, Some(123_456));
        assert_eq!(repo.name, "my-repo");
        assert_eq!(repo.visibility.as_deref(), Some("public"));
        assert_eq!(repo.is_security_policy_enabled, Some(true));
        let topics: Vec<String> = vec!["security".into(), "ci".into()];
        assert_eq!(repo.topics.as_deref(), Some(topics.as_slice()));

        // Round-trip through JSON: serialize and re-deserialize.
        let json = serde_json::to_string(&repo).unwrap();
        let repo2: GhRepository = serde_json::from_str(&json).unwrap();
        assert_eq!(repo2.name, "my-repo");
        assert_eq!(repo2.id, Some(123_456));
    }

    #[test]
    fn all_optional_fields_missing() {
        let json = serde_json::json!({ "name": "bare-repo" });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        assert_eq!(repo.name, "bare-repo");
        assert_eq!(repo.id, None);
        assert_eq!(repo.visibility, None);
        assert_eq!(repo.default_branch, None);
        assert_eq!(repo.topics, None);
        assert!(repo.license.is_none());
        assert_eq!(repo.is_security_policy_enabled, None);
        assert!(repo.security_and_analysis.is_none());
    }

    #[test]
    fn null_optional_fields() {
        let json = serde_json::json!({
            "name": "null-repo",
            "id": null,
            "visibility": null,
            "license": null,
            "topics": null,
            "is_security_policy_enabled": null
        });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        assert_eq!(repo.name, "null-repo");
        assert_eq!(repo.id, None);
        assert_eq!(repo.visibility, None);
        assert!(repo.license.is_none());
        assert_eq!(repo.topics, None);
        assert_eq!(repo.is_security_policy_enabled, None);
    }

    #[test]
    fn missing_required_name_fails() {
        let json = serde_json::json!({ "id": 1 });
        let result = serde_json::from_value::<GhRepository>(json);
        assert!(result.is_err(), "should fail without required 'name' field");
    }

    #[test]
    fn is_security_policy_enabled_rename_round_trip() {
        // The struct field uses #[serde(rename)]; verify the wire name.
        let repo: GhRepository = serde_json::from_value(full_json()).unwrap();
        let serialized = serde_json::to_value(&repo).unwrap();
        assert_eq!(
            serialized.get("is_security_policy_enabled"),
            Some(&serde_json::json!(true)),
            "serialized JSON must use the renamed key"
        );
        // Struct field name (is_security_policy_enabled) must NOT appear
        // under its un-renamed form — serde(rename) replaces it entirely.
        assert!(
            serialized.get("is_security_policy_enabled").is_some(),
            "renamed key must be present in output"
        );
    }

    #[test]
    fn unknown_visibility_string_accepted() {
        let json = serde_json::json!({
            "name": "vis-repo",
            "visibility": "restricted"
        });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        assert_eq!(repo.visibility.as_deref(), Some("restricted"));
    }

    #[test]
    fn extra_unknown_fields_ignored() {
        let json = serde_json::json!({
            "name": "extra-repo",
            "unknown_field": "surprise",
            "another_unknown": 42
        });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        assert_eq!(repo.name, "extra-repo");
    }

    #[test]
    fn topics_defaults_to_none_when_missing() {
        // topics has #[serde(default)] — verify behavior when key is absent.
        let json = serde_json::json!({ "name": "no-topics" });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        assert_eq!(repo.topics, None);
    }

    #[test]
    fn license_info_round_trip() {
        let json = serde_json::json!({
            "name": "lic-repo",
            "license": { "spdx_id": "Apache-2.0", "name": "Apache License 2.0" }
        });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        let license = repo.license.as_ref().unwrap();
        assert_eq!(license.spdx_id.as_deref(), Some("Apache-2.0"));
        assert_eq!(license.name.as_deref(), Some("Apache License 2.0"));
    }

    #[test]
    fn security_and_analysis_partial_fields() {
        let json = serde_json::json!({
            "name": "partial-sa",
            "security_and_analysis": {
                "secret_scanning": { "status": "enabled" }
            }
        });
        let repo: GhRepository = serde_json::from_value(json).unwrap();
        let sa = repo.security_and_analysis.unwrap();
        assert_eq!(
            sa.secret_scanning.unwrap().status.as_deref(),
            Some("enabled")
        );
        assert!(sa.dependabot_security_updates.is_none());
    }
}
