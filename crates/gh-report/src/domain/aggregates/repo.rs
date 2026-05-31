//! `Repo` aggregate — repository evaluation lifecycle (CHE-0054:R2).
//!
//! Owns two variants of [`DomainEvent`]: `RepoEvaluated` and
//! `RepoRemoved`. Enforces the three CHE-0054:R2 invariants:
//!
//! - **(a)** `RepoEvaluated` may appear any number of times.
//! - **(b)** `RepoRemoved` is terminal.
//! - **(c)** No events may follow `RepoRemoved`.
//!
//! Keyed by repository identity (the `inventory_key` /
//! `domain_key` field on the events; resolution to `AggregateId` lives
//! in `AppState` per CHE-0054:R5, Inc B7'a-6). Non-Repo variants
//! reaching [`Repo::apply`] are defensively ignored.
//!
//! Per CHE-0009:R1–R2, [`Repo::apply`] is total and infallible. Per
//! CHE-0008:R1, every `HandleCommand` impl is pure.

use cherry_pit_core::{Aggregate, Command, HandleCommand};

use crate::domain::events::DomainEvent;

/// Repo lifecycle phase derived from applied events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoPhase {
    /// No events applied yet — empty aggregate (CHE-0012:R1).
    #[default]
    Empty,
    /// At least one `RepoEvaluated` applied; alive.
    Active,
    /// `RepoRemoved` applied; terminal (CHE-0054:R2.b).
    Removed,
}

/// The `Repo` aggregate (CHE-0054:R2).
#[derive(Debug, Clone, Default)]
pub struct Repo {
    /// Current lifecycle phase.
    pub phase: RepoPhase,
    /// `domain_key` from the first event seen, if any.
    pub domain_key: Option<String>,
    /// Number of `RepoEvaluated` events applied.
    //
    // Width fixed at u64 per GEN-0004:R1 (no platform-dependent widths
    // in domain-model fields; cascades from `DomainEvent` field widths
    // even though this struct is not itself serialised today).
    pub evaluation_count: u64,
}

impl Aggregate for Repo {
    type Event = DomainEvent;

    fn apply(&mut self, event: &Self::Event) {
        match event {
            DomainEvent::RepoEvaluated { domain_key, .. } => {
                if self.phase == RepoPhase::Empty {
                    self.domain_key = Some(domain_key.clone());
                }
                if self.phase != RepoPhase::Removed {
                    self.phase = RepoPhase::Active;
                    // saturating_add (COM-0023:R3) — `apply` is infallible
                    // per CHE-0009:R1; saturating at u64::MAX (≈1.8e19) is
                    // unreachable in any realistic workload and preserves
                    // the no-panic contract of the projection path.
                    self.evaluation_count = self.evaluation_count.saturating_add(1);
                }
            }
            DomainEvent::RepoRemoved { domain_key, .. } => {
                if self.phase == RepoPhase::Empty {
                    self.domain_key = Some(domain_key.clone());
                }
                self.phase = RepoPhase::Removed;
            }
            // Non-Repo variants — defensively ignored. Routing is the
            // application boundary's responsibility (CHE-0054:R5).
            // `debug_assert!` (linus mid-review Info-1) traps
            // mis-routing in dev/test; release preserves silent-ignore.
            DomainEvent::SweepStarted { .. }
            | DomainEvent::SweepProgress { .. }
            | DomainEvent::SweepCompleted { .. }
            | DomainEvent::SweepFailed { .. }
            | DomainEvent::EvidencePublished { .. }
            | DomainEvent::PartialEvidenceRendered { .. }
            | DomainEvent::WebhookReceived { .. } => {
                debug_assert!(
                    false,
                    "Repo::apply received non-Repo variant: {event:?} (CHE-0054:R5 routing bug)"
                );
            }
        }
    }
}

/// Errors rejecting commands against `Repo` invariants (CHE-0054:R2).
///
/// `#[non_exhaustive]` per linus L1 — B7'b/c may add variants for
/// CAS/sequence-number conflicts or future invariant enrichment.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RepoError {
    /// No events may follow `RepoRemoved` (CHE-0054:R2.c).
    #[error("Repo already removed (terminal)")]
    AlreadyRemoved,
    /// Underlying `EventStore` operation (load/create/append) failed.
    /// Never produced by `Aggregate::handle()` (CHE-0008:R1 keeps the
    /// aggregate pure); raised at the merger boundary when the store
    /// returns `cherry_pit_core::StoreError`.
    #[error("storage error: {0}")]
    Storage(String),
}

impl From<cherry_pit_core::StoreError> for RepoError {
    fn from(err: cherry_pit_core::StoreError) -> Self {
        Self::Storage(err.to_string())
    }
}

// --- Commands (CHE-0054:R4 use cases) ---------------------------------

/// Record a repository evaluation.
#[derive(Debug, Clone)]
pub struct RecordEvaluation {
    pub domain_key: String,
    pub repo_name: String,
    pub success: bool,
    pub source: String,
    pub duration_ms: u64,
    pub timestamp: String,
    pub evidence: Option<Box<crate::domain::evidence::RepositoryEvidence>>,
}
impl Command for RecordEvaluation {}

/// Record a repository removal (terminal).
#[derive(Debug, Clone)]
pub struct RecordRemoval {
    pub domain_key: String,
    pub repo_name: String,
    pub timestamp: String,
}
impl Command for RecordRemoval {}

// --- HandleCommand impls (CHE-0008:R1 pure) ---------------------------

impl HandleCommand<RecordEvaluation> for Repo {
    type Error = RepoError;

    fn handle(&self, cmd: RecordEvaluation) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase == RepoPhase::Removed {
            return Err(RepoError::AlreadyRemoved);
        }
        Ok(vec![DomainEvent::RepoEvaluated {
            domain_key: cmd.domain_key,
            repo_name: cmd.repo_name,
            success: cmd.success,
            source: cmd.source,
            duration_ms: cmd.duration_ms,
            timestamp: cmd.timestamp,
            evidence: cmd.evidence,
        }])
    }
}

impl HandleCommand<RecordRemoval> for Repo {
    type Error = RepoError;

    fn handle(&self, cmd: RecordRemoval) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase == RepoPhase::Removed {
            return Err(RepoError::AlreadyRemoved);
        }
        Ok(vec![DomainEvent::RepoRemoved {
            domain_key: cmd.domain_key,
            repo_name: cmd.repo_name,
            timestamp: cmd.timestamp,
        }])
    }
}

// --- Tests (CHE-0008:R3 pure-handle unit tests) -----------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> String {
        "2026-05-10T12:00:00Z".to_string()
    }

    fn evaluated(r: &mut Repo) {
        r.apply(&DomainEvent::RepoEvaluated {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            success: true,
            source: "scheduled_batch".into(),
            duration_ms: 100,
            timestamp: ts(),
            evidence: None,
        });
    }

    // --- apply (CHE-0009 infallible) ---

    #[test]
    fn default_repo_is_empty() {
        let r = Repo::default();
        assert_eq!(r.phase, RepoPhase::Empty);
        assert!(r.domain_key.is_none());
        assert_eq!(r.evaluation_count, 0);
    }

    #[test]
    fn apply_repo_evaluated_sets_active_phase() {
        let mut r = Repo::default();
        evaluated(&mut r);
        assert_eq!(r.phase, RepoPhase::Active);
        assert_eq!(r.domain_key.as_deref(), Some("id-r"));
        assert_eq!(r.evaluation_count, 1);
    }

    #[test]
    fn apply_repo_evaluated_repeats_increment_counter() {
        // CHE-0054:R2.a — RepoEvaluated may appear any number of times.
        let mut r = Repo::default();
        evaluated(&mut r);
        evaluated(&mut r);
        evaluated(&mut r);
        assert_eq!(r.phase, RepoPhase::Active);
        assert_eq!(r.evaluation_count, 3);
    }

    #[test]
    fn apply_repo_removed_sets_removed_phase() {
        let mut r = Repo::default();
        evaluated(&mut r);
        r.apply(&DomainEvent::RepoRemoved {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            timestamp: ts(),
        });
        assert_eq!(r.phase, RepoPhase::Removed);
    }

    #[test]
    fn apply_repo_removed_from_empty_records_domain_key() {
        let mut r = Repo::default();
        r.apply(&DomainEvent::RepoRemoved {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            timestamp: ts(),
        });
        assert_eq!(r.phase, RepoPhase::Removed);
        assert_eq!(r.domain_key.as_deref(), Some("id-r"));
    }

    #[test]
    fn apply_evaluated_after_removed_does_not_revive() {
        // CHE-0054:R2.c — defensive: even if a stray event were applied,
        // phase stays Removed (the command path rejects this case at
        // handle()).
        let mut r = Repo::default();
        evaluated(&mut r);
        r.apply(&DomainEvent::RepoRemoved {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            timestamp: ts(),
        });
        evaluated(&mut r);
        assert_eq!(r.phase, RepoPhase::Removed);
    }

    #[test]
    #[should_panic(expected = "CHE-0054:R5 routing bug")]
    fn apply_panics_in_debug_on_non_repo_variant() {
        // Per linus mid-review Info-1: dev/test traps mis-routing,
        // release silently ignores (debug_assert no-ops).
        let mut r = Repo::default();
        r.apply(&DomainEvent::SweepStarted {
            org: "o".into(),
            repo_count: 1,
            batch_id: "b".into(),
            timestamp: ts(),
            snapshot_signature: None,
        });
    }

    // --- handle (CHE-0008 pure) ---

    #[test]
    fn record_evaluation_from_empty_emits_event() {
        let r = Repo::default();
        let events = r
            .handle(RecordEvaluation {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                success: true,
                source: "scheduled_batch".into(),
                duration_ms: 100,
                timestamp: ts(),
                evidence: None,
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::RepoEvaluated { .. }));
    }

    #[test]
    fn record_evaluation_from_active_repeats() {
        let mut r = Repo::default();
        evaluated(&mut r);
        let events = r
            .handle(RecordEvaluation {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                success: false,
                source: "scheduled_batch".into(),
                duration_ms: 50,
                timestamp: ts(),
                evidence: None,
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::RepoEvaluated { .. }));
    }

    #[test]
    fn record_evaluation_after_removed_rejects_invariant_c() {
        let mut r = Repo::default();
        evaluated(&mut r);
        r.apply(&DomainEvent::RepoRemoved {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            timestamp: ts(),
        });
        let err = r
            .handle(RecordEvaluation {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                success: true,
                source: "x".into(),
                duration_ms: 1,
                timestamp: ts(),
                evidence: None,
            })
            .unwrap_err();
        assert_eq!(err, RepoError::AlreadyRemoved);
    }

    #[test]
    fn record_removal_from_active_emits_event() {
        let mut r = Repo::default();
        evaluated(&mut r);
        let events = r
            .handle(RecordRemoval {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                timestamp: ts(),
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::RepoRemoved { .. }));
    }

    #[test]
    fn record_removal_from_empty_emits_event() {
        // Webhook-driven removal can arrive for a repo we've never
        // evaluated locally; allowed per CHE-0054:R2 (no
        // pre-evaluation precondition stated).
        let r = Repo::default();
        let events = r
            .handle(RecordRemoval {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                timestamp: ts(),
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::RepoRemoved { .. }));
    }

    #[test]
    fn record_removal_after_removed_rejects_invariant_c() {
        let mut r = Repo::default();
        r.apply(&DomainEvent::RepoRemoved {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            timestamp: ts(),
        });
        let err = r
            .handle(RecordRemoval {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RepoError::AlreadyRemoved);
    }
}
