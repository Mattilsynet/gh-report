//! Domain events persisted by gh-report.

use serde::{Deserialize, Serialize};

use crate::domain::evidence::RepositoryEvidence;

/// Repository presence encoded on the single durable gh-report event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepoPresence {
    /// Repository is present in the current read model.
    Active,
    /// Repository has been removed from the current read model.
    Removed,
}

/// A domain event representing the latest known repository state.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[repr(u8)]
pub enum DomainEvent {
    /// A single repository state snapshot was captured.
    RepositoryStateCaptured {
        /// Repository inventory key.
        domain_key: String,
        /// Human-readable repository name.
        repo_name: String,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
        /// Repository evidence produced by this evaluation.
        evidence: Option<Box<RepositoryEvidence>>,
        /// Whether this snapshot keeps or removes the repository.
        presence: RepoPresence,
    } = 0,
}

impl std::fmt::Display for DomainEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RepositoryStateCaptured {
                repo_name,
                presence,
                ..
            } => {
                let presence = match presence {
                    RepoPresence::Active => "active",
                    RepoPresence::Removed => "removed",
                };
                write!(f, "repository_state_captured({repo_name}, {presence})")
            }
        }
    }
}

impl DomainEvent {
    /// Returns the event type discriminator as a static string.
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::RepositoryStateCaptured { .. } => "RepositoryStateCaptured",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_state_captured_active_structural_round_trip() {
        use crate::test_fixtures;

        let evidence = test_fixtures::all_passing_evidence("repo-1");
        let event = DomainEvent::RepositoryStateCaptured {
            domain_key: "id-repo-1".into(),
            repo_name: "repo-1".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: Some(Box::new(evidence.clone())),
            presence: RepoPresence::Active,
        };

        assert_eq!(event.event_type(), "RepositoryStateCaptured");
        let DomainEvent::RepositoryStateCaptured {
            evidence: actual,
            presence,
            ..
        } = event;
        assert_eq!(actual.as_deref(), Some(&evidence));
        assert_eq!(presence, RepoPresence::Active);
    }

    #[test]
    fn repository_state_captured_removed_structural_round_trip() {
        let event = DomainEvent::RepositoryStateCaptured {
            domain_key: "id-old-repo".into(),
            repo_name: "old-repo".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: None,
            presence: RepoPresence::Removed,
        };
        assert_eq!(event.event_type(), "RepositoryStateCaptured");
        let DomainEvent::RepositoryStateCaptured {
            repo_name,
            presence,
            ..
        } = &event;
        assert_eq!(repo_name, "old-repo");
        assert_eq!(*presence, RepoPresence::Removed);
    }

    #[test]
    fn display_impl_covers_snapshot_variant() {
        let event = DomainEvent::RepositoryStateCaptured {
            domain_key: "k".into(),
            repo_name: "r".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: None,
            presence: RepoPresence::Active,
        };
        let display = format!("{event}");
        assert!(display.contains("repository_state_captured"));
        assert!(display.contains("active"));
    }

    #[test]
    fn event_type_returns_correct_discriminator() {
        let event = DomainEvent::RepositoryStateCaptured {
            domain_key: "k".into(),
            repo_name: "r".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: None,
            presence: RepoPresence::Removed,
        };
        assert_eq!(event.event_type(), "RepositoryStateCaptured");
    }
}
