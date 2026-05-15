//! `Run` aggregate — sweep lifecycle (CHE-0054:R1).
//!
//! Owns the five run-scoped variants of [`DomainEvent`]:
//! `SweepStarted`, `SweepProgress`, `SweepCompleted`, `SweepFailed`,
//! `EvidencePublished`. Enforces the four CHE-0054:R1 invariants:
//!
//! - **(a)** `SweepStarted` is the first event of any `Run` instance.
//! - **(b)** At most one terminal event (`SweepCompleted` xor
//!   `SweepFailed`) per instance.
//! - **(c)** `EvidencePublished` may only follow `SweepCompleted`.
//! - **(d)** `SweepProgress` may only appear between `SweepStarted` and
//!   a terminal event.
//!
//! Non-run variants reaching [`Run::apply`] (e.g. `RepoEvaluated`)
//! are defensively ignored — the application boundary is responsible
//! for routing each event to the correct aggregate per CHE-0054:R5.
//!
//! Per CHE-0009:R1–R2, [`Run::apply`] is total and infallible. Per
//! CHE-0008:R1, every `HandleCommand` impl is pure (no I/O, no
//! side-effects beyond returning events).

use cherry_pit_core::{Aggregate, Command, HandleCommand};

use crate::domain::events::DomainEvent;

/// Run lifecycle phase derived from applied events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunPhase {
    /// No events applied yet — empty aggregate (CHE-0012:R1).
    #[default]
    Empty,
    /// `SweepStarted` applied; no terminal event yet.
    Started,
    /// `SweepCompleted` applied; awaits optional `EvidencePublished`.
    Completed,
    /// `SweepFailed` applied; terminal.
    Failed,
    /// `EvidencePublished` applied; terminal.
    Published,
}

/// The `Run` aggregate (CHE-0054:R1).
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// Current lifecycle phase.
    pub phase: RunPhase,
    /// `batch_id` from the originating `SweepStarted`, if any.
    pub batch_id: Option<String>,
    /// Total repo count declared at sweep start.
    pub repo_count: usize,
    /// Number of repos completed (last `SweepProgress::completed`).
    pub completed: usize,
}

impl Aggregate for Run {
    type Event = DomainEvent;

    fn apply(&mut self, event: &Self::Event) {
        match event {
            DomainEvent::SweepStarted {
                batch_id,
                repo_count,
                ..
            } => {
                self.phase = RunPhase::Started;
                self.batch_id = Some(batch_id.clone());
                self.repo_count = *repo_count;
            }
            DomainEvent::SweepProgress { completed, .. } => {
                self.completed = *completed;
            }
            DomainEvent::SweepCompleted { .. } => {
                self.phase = RunPhase::Completed;
            }
            DomainEvent::SweepFailed { .. } => {
                self.phase = RunPhase::Failed;
            }
            DomainEvent::EvidencePublished { .. } => {
                self.phase = RunPhase::Published;
            }
            // Non-Run variants — defensively ignored. Routing to the
            // correct aggregate is the application layer's
            // responsibility (CHE-0054:R5). `debug_assert!` (linus
            // mid-review Info-1) traps mis-routing in dev/test while
            // preserving silent-ignore in release per R5.
            DomainEvent::RepoEvaluated { .. }
            | DomainEvent::RepoRemoved { .. }
            | DomainEvent::WebhookReceived { .. } => {
                debug_assert!(
                    false,
                    "Run::apply received non-Run variant: {event:?} (CHE-0054:R5 routing bug)"
                );
            }
        }
    }
}

/// Errors rejecting commands against `Run` invariants (CHE-0054:R1).
///
/// `#[non_exhaustive]` per linus L1 — B7'b/c may add variants for
/// CAS/sequence-number conflicts, EventStore append failures, or
/// future invariant enrichment.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunError {
    /// Command requires `Empty` phase but aggregate is already started
    /// (CHE-0054:R1.a).
    #[error("Run already started: phase={0:?}")]
    AlreadyStarted(RunPhase),
    /// Command requires `Started` phase (CHE-0054:R1.b/d).
    #[error("Run not in Started phase: phase={0:?}")]
    NotStarted(RunPhase),
    /// `PublishEvidence` requires `Completed` phase (CHE-0054:R1.c).
    #[error("EvidencePublished may only follow SweepCompleted: phase={0:?}")]
    NotCompleted(RunPhase),
    /// Routing-index miss: append-path called for a `batch_id` whose
    /// `AggregateId` was never registered by `start_sweep` (CHE-0054:R5).
    /// Surfaces the typed non-fatal path required by CHE-0024:R1 — the
    /// caller's `if let Err warn!` arm logs and continues.
    #[error("routing index has no AggregateId for batch_id={0:?}")]
    RoutingMiss(String),
}

// --- Commands (CHE-0054:R4 use cases) ---------------------------------

/// Begin a new sweep run.
#[derive(Debug, Clone)]
pub struct StartSweep {
    pub org: String,
    pub repo_count: usize,
    pub batch_id: String,
    pub timestamp: String,
}
impl Command for StartSweep {}

/// Record a progress checkpoint mid-sweep.
#[derive(Debug, Clone)]
pub struct RecordProgress {
    pub batch_id: String,
    pub completed: usize,
    pub total: usize,
    pub timestamp: String,
}
impl Command for RecordProgress {}

/// Mark sweep complete (success terminal).
#[derive(Debug, Clone)]
pub struct CompleteSweep {
    pub batch_id: String,
    pub duration_ms: u64,
    pub repo_count: usize,
    pub timestamp: String,
}
impl Command for CompleteSweep {}

/// Mark sweep failed (failure terminal).
#[derive(Debug, Clone)]
pub struct FailSweep {
    pub batch_id: String,
    pub error: String,
    pub duration_ms: u64,
    pub timestamp: String,
}
impl Command for FailSweep {}

/// Publish evidence after a successful sweep.
#[derive(Debug, Clone)]
pub struct PublishEvidence {
    pub page_count: usize,
    /// Whether this publish is a warm-start (replayed from baseline,
    /// no fresh GitHub API calls). Distinguishes cold-boot replay from
    /// live cycle output for downstream cache-warming consumers.
    pub warm_start: bool,
    pub timestamp: String,
}
impl Command for PublishEvidence {}

// --- HandleCommand impls (CHE-0008:R1 pure) ---------------------------

impl HandleCommand<StartSweep> for Run {
    type Error = RunError;

    fn handle(&self, cmd: StartSweep) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase != RunPhase::Empty {
            return Err(RunError::AlreadyStarted(self.phase));
        }
        Ok(vec![DomainEvent::SweepStarted {
            org: cmd.org,
            repo_count: cmd.repo_count,
            batch_id: cmd.batch_id,
            timestamp: cmd.timestamp,
        }])
    }
}

impl HandleCommand<RecordProgress> for Run {
    type Error = RunError;

    fn handle(&self, cmd: RecordProgress) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase != RunPhase::Started {
            return Err(RunError::NotStarted(self.phase));
        }
        Ok(vec![DomainEvent::SweepProgress {
            batch_id: cmd.batch_id,
            completed: cmd.completed,
            total: cmd.total,
            timestamp: cmd.timestamp,
        }])
    }
}

impl HandleCommand<CompleteSweep> for Run {
    type Error = RunError;

    fn handle(&self, cmd: CompleteSweep) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase != RunPhase::Started {
            return Err(RunError::NotStarted(self.phase));
        }
        Ok(vec![DomainEvent::SweepCompleted {
            batch_id: cmd.batch_id,
            duration_ms: cmd.duration_ms,
            repo_count: cmd.repo_count,
            timestamp: cmd.timestamp,
        }])
    }
}

impl HandleCommand<FailSweep> for Run {
    type Error = RunError;

    fn handle(&self, cmd: FailSweep) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase != RunPhase::Started {
            return Err(RunError::NotStarted(self.phase));
        }
        Ok(vec![DomainEvent::SweepFailed {
            batch_id: cmd.batch_id,
            error: cmd.error,
            duration_ms: cmd.duration_ms,
            timestamp: cmd.timestamp,
        }])
    }
}

impl HandleCommand<PublishEvidence> for Run {
    type Error = RunError;

    fn handle(&self, cmd: PublishEvidence) -> Result<Vec<DomainEvent>, Self::Error> {
        if self.phase != RunPhase::Completed {
            return Err(RunError::NotCompleted(self.phase));
        }
        Ok(vec![DomainEvent::EvidencePublished {
            page_count: cmd.page_count,
            warm_start: cmd.warm_start,
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

    fn started() -> Run {
        let mut r = Run::default();
        r.apply(&DomainEvent::SweepStarted {
            org: "org".into(),
            repo_count: 3,
            batch_id: "b1".into(),
            timestamp: ts(),
        });
        r
    }

    // --- apply (CHE-0009 infallible) ---

    #[test]
    fn default_run_is_empty() {
        let r = Run::default();
        assert_eq!(r.phase, RunPhase::Empty);
        assert!(r.batch_id.is_none());
        assert_eq!(r.repo_count, 0);
        assert_eq!(r.completed, 0);
    }

    #[test]
    fn apply_sweep_started_sets_started_phase() {
        let r = started();
        assert_eq!(r.phase, RunPhase::Started);
        assert_eq!(r.batch_id.as_deref(), Some("b1"));
        assert_eq!(r.repo_count, 3);
    }

    #[test]
    fn apply_sweep_progress_updates_completed() {
        let mut r = started();
        r.apply(&DomainEvent::SweepProgress {
            batch_id: "b1".into(),
            completed: 2,
            total: 3,
            timestamp: ts(),
        });
        assert_eq!(r.phase, RunPhase::Started);
        assert_eq!(r.completed, 2);
    }

    #[test]
    fn apply_sweep_completed_sets_completed_phase() {
        let mut r = started();
        r.apply(&DomainEvent::SweepCompleted {
            batch_id: "b1".into(),
            duration_ms: 1000,
            repo_count: 3,
            timestamp: ts(),
        });
        assert_eq!(r.phase, RunPhase::Completed);
    }

    #[test]
    fn apply_sweep_failed_sets_failed_phase() {
        let mut r = started();
        r.apply(&DomainEvent::SweepFailed {
            batch_id: "b1".into(),
            error: "timeout".into(),
            duration_ms: 7200,
            timestamp: ts(),
        });
        assert_eq!(r.phase, RunPhase::Failed);
    }

    #[test]
    fn apply_evidence_published_sets_published_phase() {
        let mut r = started();
        r.apply(&DomainEvent::SweepCompleted {
            batch_id: "b1".into(),
            duration_ms: 1000,
            repo_count: 3,
            timestamp: ts(),
        });
        r.apply(&DomainEvent::EvidencePublished {
            page_count: 5,
            warm_start: false,
            timestamp: ts(),
        });
        assert_eq!(r.phase, RunPhase::Published);
    }

    #[test]
    #[should_panic(expected = "CHE-0054:R5 routing bug")]
    fn apply_panics_in_debug_on_non_run_variant() {
        // Per linus mid-review Info-1: dev/test traps mis-routing,
        // release silently ignores (debug_assert no-ops).
        let mut r = Run::default();
        r.apply(&DomainEvent::RepoEvaluated {
            domain_key: "k".into(),
            repo_name: "r".into(),
            success: true,
            source: "s".into(),
            duration_ms: 0,
            timestamp: ts(),
            evidence: None,
        });
    }

    // --- handle (CHE-0008 pure) ---

    #[test]
    fn start_sweep_from_empty_emits_sweep_started() {
        let r = Run::default();
        let events = r
            .handle(StartSweep {
                org: "org".into(),
                repo_count: 3,
                batch_id: "b1".into(),
                timestamp: ts(),
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DomainEvent::SweepStarted { .. }));
    }

    #[test]
    fn start_sweep_after_started_rejects_invariant_a() {
        let r = started();
        let err = r
            .handle(StartSweep {
                org: "org".into(),
                repo_count: 1,
                batch_id: "b2".into(),
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::AlreadyStarted(RunPhase::Started));
    }

    #[test]
    fn record_progress_from_empty_rejects() {
        let r = Run::default();
        let err = r
            .handle(RecordProgress {
                batch_id: "b1".into(),
                completed: 1,
                total: 3,
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::NotStarted(RunPhase::Empty));
    }

    #[test]
    fn record_progress_from_started_emits_progress() {
        let r = started();
        let events = r
            .handle(RecordProgress {
                batch_id: "b1".into(),
                completed: 2,
                total: 3,
                timestamp: ts(),
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::SweepProgress { .. }));
    }

    #[test]
    fn complete_sweep_from_started_emits_sweep_completed() {
        let r = started();
        let events = r
            .handle(CompleteSweep {
                batch_id: "b1".into(),
                duration_ms: 1000,
                repo_count: 3,
                timestamp: ts(),
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::SweepCompleted { .. }));
    }

    #[test]
    fn fail_sweep_from_started_emits_sweep_failed() {
        let r = started();
        let events = r
            .handle(FailSweep {
                batch_id: "b1".into(),
                error: "timeout".into(),
                duration_ms: 7200,
                timestamp: ts(),
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::SweepFailed { .. }));
    }

    #[test]
    fn complete_sweep_from_completed_rejects_invariant_b() {
        // Once Completed, cannot complete again.
        let mut r = started();
        r.apply(&DomainEvent::SweepCompleted {
            batch_id: "b1".into(),
            duration_ms: 1000,
            repo_count: 3,
            timestamp: ts(),
        });
        let err = r
            .handle(CompleteSweep {
                batch_id: "b1".into(),
                duration_ms: 1000,
                repo_count: 3,
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::NotStarted(RunPhase::Completed));
    }

    #[test]
    fn fail_sweep_from_completed_rejects_invariant_b() {
        // Once Completed, cannot fail (terminal-xor).
        let mut r = started();
        r.apply(&DomainEvent::SweepCompleted {
            batch_id: "b1".into(),
            duration_ms: 1000,
            repo_count: 3,
            timestamp: ts(),
        });
        let err = r
            .handle(FailSweep {
                batch_id: "b1".into(),
                error: "late".into(),
                duration_ms: 1,
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::NotStarted(RunPhase::Completed));
    }

    #[test]
    fn publish_evidence_from_completed_emits_event() {
        let mut r = started();
        r.apply(&DomainEvent::SweepCompleted {
            batch_id: "b1".into(),
            duration_ms: 1000,
            repo_count: 3,
            timestamp: ts(),
        });
        let events = r
            .handle(PublishEvidence {
                page_count: 5,
                warm_start: false,
                timestamp: ts(),
            })
            .unwrap();
        assert!(matches!(events[0], DomainEvent::EvidencePublished { .. }));
    }

    #[test]
    fn publish_evidence_from_started_rejects_invariant_c() {
        let r = started();
        let err = r
            .handle(PublishEvidence {
                page_count: 5,
                warm_start: false,
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::NotCompleted(RunPhase::Started));
    }

    #[test]
    fn publish_evidence_from_failed_rejects_invariant_c() {
        let mut r = started();
        r.apply(&DomainEvent::SweepFailed {
            batch_id: "b1".into(),
            error: "x".into(),
            duration_ms: 1,
            timestamp: ts(),
        });
        let err = r
            .handle(PublishEvidence {
                page_count: 5,
                warm_start: false,
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::NotCompleted(RunPhase::Failed));
    }

    #[test]
    fn record_progress_after_terminal_rejects_invariant_d() {
        let mut r = started();
        r.apply(&DomainEvent::SweepCompleted {
            batch_id: "b1".into(),
            duration_ms: 1,
            repo_count: 3,
            timestamp: ts(),
        });
        let err = r
            .handle(RecordProgress {
                batch_id: "b1".into(),
                completed: 4,
                total: 3,
                timestamp: ts(),
            })
            .unwrap_err();
        assert_eq!(err, RunError::NotStarted(RunPhase::Completed));
    }

    // --- RunError::RoutingMiss (CHE-0024:R1) ---

    #[test]
    fn routing_miss_display_carries_batch_id() {
        let err = RunError::RoutingMiss("xyz".into());
        assert_eq!(
            err.to_string(),
            "routing index has no AggregateId for batch_id=\"xyz\""
        );
    }

    #[test]
    fn routing_miss_debug_carries_batch_id() {
        let err = RunError::RoutingMiss("xyz".into());
        assert!(format!("{err:?}").contains("RoutingMiss"));
        assert!(format!("{err:?}").contains("xyz"));
    }
}
