//! Repository domain model.

use serde::{Deserialize, Serialize};

/// A normalized repository from inventory.
///
/// # Equality semantics
///
/// `PartialEq`/`Eq` compare only *identity and structural* fields
/// (`id`, `name`, `visibility`, `default_branch`, `archived`,
/// `inventory_key`, `updated_at`). Informational metadata
/// (`pushed_at`, `created_at`, `description`, `fork`, `html_url`,
/// `topics`, `license_spdx`, `language`, `node_id`, `has_issues`) is
/// excluded so that checkpoint and baseline deduplication are not
/// affected by cosmetic changes to these fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    /// Numeric or node ID from GitHub API.
    pub id: String,
    /// GitHub GraphQL node ID, if available.
    pub node_id: Option<String>,
    /// Repository name (not fully qualified).
    pub name: String,
    /// Visibility: "public", "internal", or "private".
    pub visibility: Visibility,
    /// Primary language, if known.
    pub language: Option<String>,
    /// Default branch name (e.g., "main").
    pub default_branch: String,
    /// Whether the repository is archived.
    pub archived: bool,
    /// Stable key for checkpoint and evidence correlation.
    pub inventory_key: String,
    /// ISO 8601 timestamp of last update (settings change or push).
    /// Used by baseline mechanism to detect changes.
    pub updated_at: Option<String>,

    /// Whether the repository has issues enabled.
    pub has_issues: bool,
    /// ISO 8601 timestamp of the last git push. Unlike `updated_at`,
    /// this reflects actual code activity (not settings changes).
    pub pushed_at: Option<String>,
    /// ISO 8601 timestamp of when the repository was created.
    pub created_at: Option<String>,
    /// Short description of the repository.
    pub description: Option<String>,
    /// Whether this repository is a fork of another repository.
    pub fork: bool,
    /// Browser URL for the repository (e.g., `https://github.com/org/repo`).
    pub html_url: Option<String>,
    /// Topic tags applied to the repository.
    pub topics: Vec<String>,
    /// SPDX license identifier (e.g., `"MIT"`, `"Apache-2.0"`), if detected.
    pub license_spdx: Option<String>,
}

impl PartialEq for Repository {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.visibility == other.visibility
            && self.default_branch == other.default_branch
            && self.archived == other.archived
            && self.inventory_key == other.inventory_key
            && self.updated_at == other.updated_at
    }
}

impl Eq for Repository {}

/// Repository visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// The repository is visible to everyone on the internet.
    Public = 0,
    /// The repository is visible to members of the organization.
    Internal = 1,
    /// The repository is only visible to users with explicit access.
    Private = 2,
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Public => f.write_str("Public"),
            Self::Internal => f.write_str("Internal"),
            Self::Private => f.write_str("Private"),
        }
    }
}

impl Repository {
    /// Returns `true` if this is a public repository.
    #[must_use]
    pub fn is_public(&self) -> bool {
        self.visibility == Visibility::Public
    }
}

/// Sorting key for deterministic repository ordering.
impl Ord for Repository {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id.cmp(&other.id).then(self.name.cmp(&other.name))
    }
}

impl PartialOrd for Repository {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures;

    #[test]
    fn is_public_checks_visibility() {
        assert!(test_fixtures::make_repository("pub", false, Visibility::Public).is_public());
        assert!(!test_fixtures::make_repository("priv", false, Visibility::Private).is_public());
        assert!(!test_fixtures::make_repository("int", false, Visibility::Internal).is_public());
    }

    #[test]
    fn ordering_is_deterministic() {
        let a = test_fixtures::make_repository("alpha", false, Visibility::Public);
        let b = test_fixtures::make_repository("beta", false, Visibility::Public);
        assert!(a < b);
    }

    #[test]
    fn equality_excludes_informational_fields() {
        let mut a = test_fixtures::make_repository("repo", false, Visibility::Public);
        let mut b = a.clone();

        a.description = Some("Old description".to_string());
        b.description = Some("New description".to_string());
        a.topics = vec!["rust".to_string()];
        b.topics = vec!["go".to_string()];
        a.fork = false;
        b.fork = true;
        a.pushed_at = Some("2026-01-01T00:00:00Z".to_string());
        b.pushed_at = Some("2026-04-01T00:00:00Z".to_string());

        assert_eq!(a, b, "informational fields should not affect equality");
    }

    #[test]
    fn equality_includes_structural_fields() {
        let a = test_fixtures::make_repository("repo", false, Visibility::Public);
        let mut b = a.clone();

        b.default_branch = "develop".to_string();
        assert_ne!(a, b, "structural field change should break equality");
    }

    #[test]
    fn visibility_serde_round_trip() {
        let variants = [
            (Visibility::Public, "\"public\""),
            (Visibility::Private, "\"private\""),
            (Visibility::Internal, "\"internal\""),
        ];
        for (vis, expected_json) in &variants {
            let serialized = serde_json::to_string(vis).unwrap();
            assert_eq!(
                &serialized, expected_json,
                "serialization mismatch for {vis:?}"
            );
            let deserialized: Visibility = serde_json::from_str(&serialized).unwrap();
            assert_eq!(*vis, deserialized, "round-trip mismatch for {vis:?}");
        }
    }

    #[test]
    fn visibility_deserialize_unknown_variant_fails() {
        let result = serde_json::from_str::<Visibility>("\"protected\"");
        assert!(
            result.is_err(),
            "unknown variant should fail deserialization"
        );
    }

    #[test]
    fn visibility_deserialize_wrong_case_fails() {
        let result = serde_json::from_str::<Visibility>("\"Public\"");
        assert!(
            result.is_err(),
            "PascalCase variant should fail deserialization"
        );
    }

    #[test]
    fn visibility_deserialize_empty_string_fails() {
        let result = serde_json::from_str::<Visibility>("\"\"");
        assert!(result.is_err(), "empty string should fail deserialization");
    }

    #[test]
    fn visibility_display_title_case() {
        assert_eq!(Visibility::Public.to_string(), "Public");
        assert_eq!(Visibility::Internal.to_string(), "Internal");
        assert_eq!(Visibility::Private.to_string(), "Private");
    }

    #[test]
    fn visibility_display_differs_from_serde() {
        for vis in &[
            Visibility::Public,
            Visibility::Internal,
            Visibility::Private,
        ] {
            let display = vis.to_string();
            let serde = serde_json::to_string(vis).unwrap();
            let serde_unquoted = serde.trim_matches('"');
            assert_ne!(
                display, serde_unquoted,
                "Display and serde should differ for {vis:?}"
            );
        }
    }
}
