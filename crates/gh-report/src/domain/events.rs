//! Domain events representing state transitions in the system.
//!
//! These events model what happened in the domain, not what should happen
//! (commands) or how it was delivered (infrastructure). Each variant
//! captures the essential facts of a state transition.
//!
//! ## Encoding (pardosa-genome adoption — δ.2.b)
//!
//! `DomainEvent` derives `pardosa_encoding::Encode` via
//! `#[derive(GenomeSafe)]` — the discriminant table and per-variant
//! field order live on the enum's own rustdoc below. Emission is
//! declaration-order for both the `u8` variant discriminant (via
//! `#[repr(u8)]` + explicit `Variant = 0..=8` literals) and each
//! variant's payload fields, per GEN-0037:R4. `Encode` satisfies the
//! supertrait required on `cherry_pit_core::DomainEvent` per amended
//! CHE-0010 and advances pardosa-genome adoption per CHE-0065:R1.
//!
//! The `#[serde(tag = "type")]` discriminator was removed in an
//! earlier sub-mission and `event_type()` returns the `PascalCase`
//! Rust variant name. `Serialize`/`Deserialize` derives remain on
//! `DomainEvent` — they are load-bearing for the
//! [`pardosa_eventstore::PardosaLogEventStore<DomainEvent>`] substrate
//! and cannot be dropped without an atomic consumer-side swap.
//! Without `#[serde(tag)]`, serde's default enum
//! representation is external tagging (`{"RepoEvaluated": {...}}`),
//! but no production path inspects that shape — wire identity flows
//! through `Encode` and `event_type()`.
//!
//! ## Variant evolution (CHE-0022:R5)
//!
//! `DomainEvent` is **not** `#[non_exhaustive]`: CHE-0022:R5 forbids
//! the attribute on domain event enums — they are versioned via
//! additive variants, not attribute hedging. The exhaustive `match`
//! in [`DomainEvent::event_type`] (no wildcard arm) covers the
//! documentation purpose `#[non_exhaustive]` previously served: any
//! new variant produces a compile error there, forcing the maintainer
//! to update the discriminant table on the enum's rustdoc and append
//! the next `Variant = N` literal.

use pardosa_genome::GenomeSafe;
use serde::{Deserialize, Serialize};

use crate::domain::evidence::RepositoryEvidence;

/// A domain event representing a state transition in the system.
///
/// Published to an in-process event bus and consumed by subscribers
/// (logging, metrics, persistence, etc.).
///
/// ## Invariants
///
/// - Events are immutable after creation.
/// - Timestamps are ISO 8601 UTC strings (from `jiff::Timestamp::now()`).
/// - `domain_key` values match the `inventory_key` on `Repository`.
///
/// ## Wire format (GEN-0037:R4 + pardosa-derive)
///
/// The `GenomeSafe` derive emits a `pardosa_encoding::Encode` impl that
/// writes the `u8` discriminant (via `#[repr(u8)]` + explicit
/// `Variant = 0..=8` literals below) followed by each payload field in
/// declaration order. Variant reorder, insertion, removal, or field
/// reorder within a variant is a wire-format break; new variants must
/// be appended at the end with the next discriminant, and new fields
/// must be appended at the end of their variant (CHE-0064:R2 +
/// PAR-0024:R5 + CHE-0022:R5 additive evolution).
///
/// | Discriminant | Variant                    | Payload fields (declaration order)                                                                  |
/// |--------------|----------------------------|-----------------------------------------------------------------------------------------------------|
/// | `0u8`        | `SweepStarted`             | `org`, `repo_count`, `batch_id`, `timestamp`, `snapshot_signature`                                  |
/// | `1u8`        | `RepoEvaluated`            | `domain_key`, `repo_name`, `success`, `source`, `duration_ms`, `timestamp`, `evidence`              |
/// | `2u8`        | `RepoRemoved`              | `domain_key`, `repo_name`, `timestamp`                                                              |
/// | `3u8`        | `SweepCompleted`           | `batch_id`, `duration_ms`, `repo_count`, `timestamp`                                                |
/// | `4u8`        | `WebhookReceived`          | `action`, `repo`, `timestamp`                                                                       |
/// | `5u8`        | `EvidencePublished`        | `page_count`, `warm_start`, `timestamp`                                                             |
/// | `6u8`        | `PartialEvidenceRendered`  | `batch_id`, `page_count`, `pending_repos`, `timestamp`                                              |
/// | `7u8`        | `SweepFailed`              | `batch_id`, `error`, `duration_ms`, `timestamp`                                                     |
/// | `8u8`        | `SweepProgress`            | `batch_id`, `completed`, `total`, `timestamp`                                                       |
///
/// `RepoEvaluated.evidence: Option<Box<RepositoryEvidence>>` resolves
/// via the `Option<T>` blanket → `Box<T>` blanket →
/// `<RepositoryEvidence as Encode>::encode` chain in `pardosa-encoding`.
#[derive(Debug, Clone, Serialize, Deserialize, GenomeSafe)]
#[repr(u8)]
pub enum DomainEvent {
    /// A scheduled sweep has started.
    SweepStarted {
        /// Target organization.
        org: String,
        /// Number of repositories to evaluate.
        //
        // Width fixed at u64 per GEN-0004:R1 / GEN-0032 / GEN-0037 EVT-004:
        // platform-dependent widths are forbidden in any canonical-bytes
        // type position. Conversion from `usize` (e.g. `Vec::len()`) lives
        // at the constructor call sites per COM-0023.
        repo_count: u64,
        /// Unique identifier for this sweep batch.
        batch_id: String,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
        /// Snapshot signature this sweep was keyed to (SHA-256 of the
        /// org-level alert summary minus `run_timestamp`); audit-trail
        /// surface for the replay-as-rebuild baseline (CHE-0048:24 /
        /// CHE-0065). `None` until δ.3c-ii (bead `adr-fmt-baao9`)
        /// threads `build_snapshot_signature` through `StartSweep`;
        /// kept `Option` for backward-tolerance per CHE-0064:R2
        /// additive evolution.
        //
        // Tail placement is load-bearing: pardosa-encoding emits
        // payload fields in declaration order, so appending here
        // preserves prefix-compatibility with the δ.1 byte-equality
        // discipline (CHE-0022:R5 + PAR-0024:R5).
        snapshot_signature: Option<String>,
    } = 0,

    /// A single repository was evaluated (success or failure).
    RepoEvaluated {
        /// Repository inventory key (e.g., `"id-my-repo"`).
        domain_key: String,
        /// Human-readable repository name.
        repo_name: String,
        /// Whether the evaluation succeeded.
        success: bool,
        /// Origin of the evaluation job (e.g., `"scheduled_batch"`, `"external"`).
        source: String,
        /// Evaluation duration in milliseconds.
        duration_ms: u64,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
        /// Repository evidence produced by this evaluation.
        ///
        /// Load-bearing for `EvidenceProjection`: the projection is the
        /// sole writer of the read model (CHE-0048:R2 + BC-v2-2), so the
        /// envelope must carry the materialised state. `Some` for both
        /// success and failure paths once the collector populates them
        /// (failure path uses `collect::failure_evidence`); `None` is
        /// permitted for transitional / metadata-only emissions and for
        /// backward-compat with msgpack envelopes serialized before B6'
        /// landed (CHE-0022 additive evolution).
        ///
        /// Boxed because `RepositoryEvidence` is ~560 bytes and would
        /// otherwise dominate the enum's stack footprint (the other 7
        /// variants are tens of bytes); `Option<Box<_>>` also benefits
        /// from the null-pointer niche optimisation.
        evidence: Option<Box<RepositoryEvidence>>,
    } = 1,

    /// A repository was removed from the evidence store.
    ///
    /// Triggered by webhook `repository.deleted` or `repository.archived` events.
    RepoRemoved {
        /// Repository inventory key.
        domain_key: String,
        /// Human-readable repository name.
        repo_name: String,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 2,

    /// A scheduled sweep has completed.
    SweepCompleted {
        /// Unique identifier matching the originating [`SweepStarted::batch_id`].
        batch_id: String,
        /// Total sweep duration in milliseconds.
        duration_ms: u64,
        /// Number of repositories evaluated.
        repo_count: u64,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 3,

    /// A webhook was received and validated.
    WebhookReceived {
        /// Mapped action (e.g., `"enqueue"`, `"remove"`, `"ignore"`).
        action: String,
        /// Repository name, if applicable.
        repo: Option<String>,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 4,

    /// Evidence was published (HTML cache updated, WebSocket broadcast sent).
    EvidencePublished {
        /// Number of HTML pages in the cache.
        page_count: u64,
        /// Whether this was a warm-start publish (from baseline, no API calls).
        warm_start: bool,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 5,

    /// Mid-sweep partial render emitted by the partial-publisher debounce.
    ///
    /// Non-terminal: admissible only while `Run.phase == Started`. Does
    /// NOT drive a phase transition. Distinct from the terminal
    /// `EvidencePublished` (which follows `SweepCompleted` per
    /// CHE-0054:R1.c). See the new R1.e admitting this variant.
    PartialEvidenceRendered {
        /// Sweep `batch_id` this partial belongs to.
        batch_id: String,
        /// Number of HTML pages rendered into the cache.
        page_count: u64,
        /// Repos still pending evaluation at render time.
        pending_repos: u64,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 6,

    /// A scheduled sweep failed (timeout, error, or all jobs rejected).
    SweepFailed {
        /// Unique identifier matching the originating [`SweepStarted::batch_id`].
        batch_id: String,
        /// Human-readable error description.
        error: String,
        /// Duration from sweep start to failure, in milliseconds.
        duration_ms: u64,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 7,

    /// Progress update during a sweep (emitted at phase transitions).
    SweepProgress {
        /// Unique identifier matching the originating [`SweepStarted::batch_id`].
        batch_id: String,
        /// Number of repositories completed so far (resumed + baseline + evaluated).
        completed: u64,
        /// Total number of repositories in the sweep.
        total: u64,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    } = 8,
}

impl std::fmt::Display for DomainEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SweepStarted {
                org, repo_count, ..
            } => {
                write!(f, "sweep_started(org={org}, repos={repo_count})")
            }
            Self::RepoEvaluated {
                repo_name, success, ..
            } => {
                let outcome = if *success { "ok" } else { "fail" };
                write!(f, "repo_evaluated({repo_name}, {outcome})")
            }
            Self::RepoRemoved { repo_name, .. } => {
                write!(f, "repo_removed({repo_name})")
            }
            Self::SweepCompleted {
                repo_count,
                duration_ms,
                ..
            } => {
                write!(
                    f,
                    "sweep_completed(repos={repo_count}, duration={duration_ms}ms)"
                )
            }
            Self::WebhookReceived { action, repo, .. } => {
                let repo_str = repo.as_deref().unwrap_or("n/a");
                write!(f, "webhook_received({action}, {repo_str})")
            }
            Self::EvidencePublished {
                page_count,
                warm_start,
                ..
            } => {
                let label = if *warm_start { "warm" } else { "live" };
                write!(f, "evidence_published({page_count} pages, {label})")
            }
            Self::PartialEvidenceRendered {
                batch_id,
                page_count,
                pending_repos,
                ..
            } => {
                write!(
                    f,
                    "partial_evidence_rendered(batch={batch_id}, pages={page_count}, pending={pending_repos})"
                )
            }
            Self::SweepFailed {
                batch_id,
                error,
                duration_ms,
                ..
            } => {
                write!(
                    f,
                    "sweep_failed(batch={batch_id}, error={error}, duration={duration_ms}ms)"
                )
            }
            Self::SweepProgress {
                completed, total, ..
            } => {
                write!(f, "sweep_progress({completed}/{total})")
            }
        }
    }
}

impl DomainEvent {
    /// Returns the event type discriminator as a static string.
    ///
    /// `PascalCase`, matching the Rust variant name 1:1. Used for
    /// structured-log emission (e.g. tracing fields) and HTTP
    /// projection responses. The on-disk wire identity is the
    /// leading `u8` discriminator in
    /// `<Self as pardosa_encoding::Encode>::encode`, not this string.
    ///
    /// The match is intentionally exhaustive (no wildcard arm) so that
    /// adding a new `DomainEvent` variant produces a compile error here,
    /// forcing the maintainer to add the corresponding `event_type()` arm.
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::SweepStarted { .. } => "SweepStarted",
            Self::RepoEvaluated { .. } => "RepoEvaluated",
            Self::RepoRemoved { .. } => "RepoRemoved",
            Self::SweepCompleted { .. } => "SweepCompleted",
            Self::WebhookReceived { .. } => "WebhookReceived",
            Self::EvidencePublished { .. } => "EvidencePublished",
            Self::PartialEvidenceRendered { .. } => "PartialEvidenceRendered",
            Self::SweepFailed { .. } => "SweepFailed",
            Self::SweepProgress { .. } => "SweepProgress",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_str() -> String {
        jiff::Timestamp::now().to_string()
    }

    // Structural placeholders for the former 9 `*_serialization_round_trip`
    // tests + the 10th `repo_evaluated_with_evidence_round_trip` test.
    // Real wire-format round-trips through pardosa-genome land in sub-04
    // of the pardosa-serde-swap package; for sub-03 the assertion is
    // simply that each variant constructs, exposes its discriminator,
    // and preserves fields under a destructuring match. The former
    // `tag_stability_known_json_strings` and `event_type_matches_serde_tag`
    // tests are gone — they probed serde JSON tag stability, a surface
    // that no longer exists on `DomainEvent`.

    #[test]
    fn sweep_started_structural_round_trip() {
        let event = DomainEvent::SweepStarted {
            org: "my-org".into(),
            repo_count: 42,
            batch_id: "batch-001".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
            snapshot_signature: None,
        };
        assert_eq!(event.event_type(), "SweepStarted");
        let DomainEvent::SweepStarted {
            org, repo_count, ..
        } = &event
        else {
            panic!("expected SweepStarted, got {event:?}");
        };
        assert_eq!(org, "my-org");
        assert_eq!(*repo_count, 42);
    }

    #[test]
    fn repo_evaluated_structural_round_trip() {
        let event = DomainEvent::RepoEvaluated {
            domain_key: "id-my-repo".into(),
            repo_name: "my-repo".into(),
            success: true,
            source: "scheduled_batch".into(),
            duration_ms: 1234,
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: None,
        };
        assert_eq!(event.event_type(), "RepoEvaluated");
        let DomainEvent::RepoEvaluated {
            repo_name,
            success,
            evidence,
            ..
        } = &event
        else {
            panic!("expected RepoEvaluated, got {event:?}");
        };
        assert_eq!(repo_name, "my-repo");
        assert!(*success);
        assert!(evidence.is_none());
    }

    #[test]
    fn repo_evaluated_with_evidence_structural_round_trip() {
        // The B6' `evidence: Some(_)` payload survives construction and
        // destructuring; sub-04 will reinstate the wire-format probe
        // through pardosa-genome.
        use crate::test_fixtures;

        let evidence = test_fixtures::all_passing_evidence("repo-1");
        let event = DomainEvent::RepoEvaluated {
            domain_key: "id-repo-1".into(),
            repo_name: "repo-1".into(),
            success: true,
            source: "scheduled_batch".into(),
            duration_ms: 0,
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: Some(Box::new(evidence.clone())),
        };
        assert_eq!(event.event_type(), "RepoEvaluated");
        match event {
            DomainEvent::RepoEvaluated {
                evidence: Some(ev), ..
            } => {
                assert_eq!(*ev, evidence);
            }
            other => panic!("expected RepoEvaluated with Some(evidence), got {other:?}"),
        }
    }

    #[test]
    fn repo_removed_structural_round_trip() {
        let event = DomainEvent::RepoRemoved {
            domain_key: "id-old-repo".into(),
            repo_name: "old-repo".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
        };
        assert_eq!(event.event_type(), "RepoRemoved");
        let DomainEvent::RepoRemoved { repo_name, .. } = &event else {
            panic!("expected RepoRemoved, got {event:?}");
        };
        assert_eq!(repo_name, "old-repo");
    }

    #[test]
    fn sweep_completed_structural_round_trip() {
        let event = DomainEvent::SweepCompleted {
            batch_id: "batch-001".into(),
            duration_ms: 5000,
            repo_count: 42,
            timestamp: "2026-04-20T12:00:00Z".into(),
        };
        assert_eq!(event.event_type(), "SweepCompleted");
        let DomainEvent::SweepCompleted {
            duration_ms,
            repo_count,
            ..
        } = &event
        else {
            panic!("expected SweepCompleted, got {event:?}");
        };
        assert_eq!(*duration_ms, 5000);
        assert_eq!(*repo_count, 42);
    }

    #[test]
    fn webhook_received_structural_round_trip() {
        let event = DomainEvent::WebhookReceived {
            action: "enqueue".into(),
            repo: Some("my-repo".into()),
            timestamp: "2026-04-20T12:00:00Z".into(),
        };
        assert_eq!(event.event_type(), "WebhookReceived");
        let DomainEvent::WebhookReceived { action, repo, .. } = &event else {
            panic!("expected WebhookReceived, got {event:?}");
        };
        assert_eq!(action, "enqueue");
        assert_eq!(repo.as_deref(), Some("my-repo"));
    }

    #[test]
    fn evidence_published_structural_round_trip() {
        let event = DomainEvent::EvidencePublished {
            page_count: 5,
            warm_start: false,
            timestamp: "2026-04-20T12:00:00Z".into(),
        };
        assert_eq!(event.event_type(), "EvidencePublished");
        let DomainEvent::EvidencePublished {
            page_count,
            warm_start,
            ..
        } = &event
        else {
            panic!("expected EvidencePublished, got {event:?}");
        };
        assert_eq!(*page_count, 5);
        assert!(!*warm_start);
    }

    #[test]
    fn partial_evidence_rendered_structural_round_trip() {
        let event = DomainEvent::PartialEvidenceRendered {
            batch_id: "b1".into(),
            page_count: 3,
            pending_repos: 7,
            timestamp: "2026-04-20T12:00:00Z".into(),
        };
        assert_eq!(event.event_type(), "PartialEvidenceRendered");
        let DomainEvent::PartialEvidenceRendered {
            batch_id,
            page_count,
            pending_repos,
            ..
        } = &event
        else {
            panic!("expected PartialEvidenceRendered, got {event:?}");
        };
        assert_eq!(batch_id, "b1");
        assert_eq!(*page_count, 3);
        assert_eq!(*pending_repos, 7);
    }

    #[test]
    fn sweep_failed_structural_round_trip() {
        let event = DomainEvent::SweepFailed {
            batch_id: "batch-001".into(),
            error: "timeout after 7200s".into(),
            duration_ms: 7_200_000,
            timestamp: "2026-04-20T14:00:00Z".into(),
        };
        assert_eq!(event.event_type(), "SweepFailed");
        let DomainEvent::SweepFailed {
            error, duration_ms, ..
        } = &event
        else {
            panic!("expected SweepFailed, got {event:?}");
        };
        assert_eq!(error, "timeout after 7200s");
        assert_eq!(*duration_ms, 7_200_000);
    }

    #[test]
    fn sweep_progress_structural_round_trip() {
        let event = DomainEvent::SweepProgress {
            batch_id: "batch-001".into(),
            completed: 25,
            total: 100,
            timestamp: "2026-04-20T12:30:00Z".into(),
        };
        assert_eq!(event.event_type(), "SweepProgress");
        let DomainEvent::SweepProgress {
            completed, total, ..
        } = &event
        else {
            panic!("expected SweepProgress, got {event:?}");
        };
        assert_eq!(*completed, 25);
        assert_eq!(*total, 100);
    }

    #[test]
    fn display_impl_covers_all_variants() {
        let ts = now_str();
        let events = vec![
            DomainEvent::SweepStarted {
                org: "org".into(),
                repo_count: 10,
                batch_id: "b".into(),
                timestamp: ts.clone(),
                snapshot_signature: None,
            },
            DomainEvent::RepoEvaluated {
                domain_key: "k".into(),
                repo_name: "r".into(),
                success: true,
                source: "s".into(),
                duration_ms: 100,
                timestamp: ts.clone(),
                evidence: None,
            },
            DomainEvent::RepoRemoved {
                domain_key: "k".into(),
                repo_name: "r".into(),
                timestamp: ts.clone(),
            },
            DomainEvent::SweepCompleted {
                batch_id: "b".into(),
                duration_ms: 1000,
                repo_count: 10,
                timestamp: ts.clone(),
            },
            DomainEvent::WebhookReceived {
                action: "enqueue".into(),
                repo: None,
                timestamp: ts.clone(),
            },
            DomainEvent::EvidencePublished {
                page_count: 3,
                warm_start: true,
                timestamp: ts.clone(),
            },
            DomainEvent::PartialEvidenceRendered {
                batch_id: "b".into(),
                page_count: 2,
                pending_repos: 4,
                timestamp: ts.clone(),
            },
            DomainEvent::SweepFailed {
                batch_id: "b".into(),
                error: "timeout".into(),
                duration_ms: 5000,
                timestamp: ts.clone(),
            },
            DomainEvent::SweepProgress {
                batch_id: "b".into(),
                completed: 5,
                total: 10,
                timestamp: ts,
            },
        ];

        for event in &events {
            let display = format!("{event}");
            assert!(!display.is_empty(), "Display should produce output");
        }
    }

    /// Byte-equality guard for the hand-rolled `Encode` impl on `DomainEvent`
    /// (sub-mission δ.1 of pardosa-genome-adoption-schism).
    ///
    /// Locks the wire format of all 9 variants — plus the `evidence:
    /// Some(_)` arm of `RepoEvaluated` — to literal byte sequences. δ.2's
    /// `#[derive(GenomeSafe)]` swap must reproduce these bytes exactly; any
    /// divergence is a wire-format break that would silently invalidate
    /// the persisted event log (δ.3c-ii retired the prior
    /// `baseline.msgpack` snapshot; the event log is now the only
    /// durable representation).
    ///
    /// Determinism: every payload uses literal `&str`/`u64`/`bool` values and
    /// a stable `test_fixtures::all_passing_evidence` fixture (no
    /// `jiff::Timestamp::now()`, no RNG, no `HashMap` on the encoded path).
    ///
    /// Maintenance: when adding a new `DomainEvent` variant, append a new
    /// case here and capture its bytes via the `RECAPTURE` workflow below.
    /// Editing an existing case's literal is a wire-format break — bounce
    /// to the pardosa-genome-adoption-schism mission tree, do not silently
    /// re-baseline.
    //
    // 10 variant cases as struct-literal payloads are structurally bound by
    // the enum shape (see the parallel allow on `Encode::encode` above);
    // extracting per-case helpers would add noise without reducing risk.
    #[allow(clippy::too_many_lines)]
    #[test]
    fn wire_format_byte_equality() {
        use crate::test_fixtures;
        use pardosa_encoding::Encode;

        // Deterministic Some(evidence) payload: builder is timestamp-free
        // (constant fixture timestamps) and contains no HashMap-backed
        // fields on the Encode path.
        let evidence = test_fixtures::all_passing_evidence("repo-1");

        let cases: Vec<(&'static str, DomainEvent, &'static [u8])> = vec![
            (
                "SweepStarted",
                DomainEvent::SweepStarted {
                    org: "test-org".into(),
                    repo_count: 42,
                    batch_id: "batch-001".into(),
                    timestamp: "2024-01-01T00:00:00Z".into(),
                    snapshot_signature: None,
                },
                EXPECTED_SWEEP_STARTED,
            ),
            (
                "RepoEvaluated/None",
                DomainEvent::RepoEvaluated {
                    domain_key: "id-repo-1".into(),
                    repo_name: "repo-1".into(),
                    success: true,
                    source: "scheduled_batch".into(),
                    duration_ms: 1234,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                    evidence: None,
                },
                EXPECTED_REPO_EVALUATED_NONE,
            ),
            (
                "RepoEvaluated/Some",
                DomainEvent::RepoEvaluated {
                    domain_key: "id-repo-1".into(),
                    repo_name: "repo-1".into(),
                    success: true,
                    source: "scheduled_batch".into(),
                    duration_ms: 1234,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                    evidence: Some(Box::new(evidence.clone())),
                },
                EXPECTED_REPO_EVALUATED_SOME,
            ),
            (
                "RepoRemoved",
                DomainEvent::RepoRemoved {
                    domain_key: "id-old-repo".into(),
                    repo_name: "old-repo".into(),
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_REPO_REMOVED,
            ),
            (
                "SweepCompleted",
                DomainEvent::SweepCompleted {
                    batch_id: "batch-001".into(),
                    duration_ms: 5000,
                    repo_count: 42,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_SWEEP_COMPLETED,
            ),
            (
                "WebhookReceived",
                DomainEvent::WebhookReceived {
                    action: "enqueue".into(),
                    repo: Some("my-repo".into()),
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_WEBHOOK_RECEIVED,
            ),
            (
                "EvidencePublished",
                DomainEvent::EvidencePublished {
                    page_count: 5,
                    warm_start: true,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_EVIDENCE_PUBLISHED,
            ),
            (
                "PartialEvidenceRendered",
                DomainEvent::PartialEvidenceRendered {
                    batch_id: "batch-001".into(),
                    page_count: 3,
                    pending_repos: 7,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_PARTIAL_EVIDENCE_RENDERED,
            ),
            (
                "SweepFailed",
                DomainEvent::SweepFailed {
                    batch_id: "batch-001".into(),
                    error: "timeout".into(),
                    duration_ms: 7200,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_SWEEP_FAILED,
            ),
            (
                "SweepProgress",
                DomainEvent::SweepProgress {
                    batch_id: "batch-001".into(),
                    completed: 25,
                    total: 100,
                    timestamp: "2024-01-01T00:00:00Z".into(),
                },
                EXPECTED_SWEEP_PROGRESS,
            ),
        ];

        // Determinism check (package_abort_if #1): encode twice, compare.
        // Any divergence here means the hand-rolled encoder is
        // non-deterministic and δ.2 cannot proceed.
        for (name, event, _) in &cases {
            let mut a = Vec::new();
            let mut b = Vec::new();
            event.encode(&mut a);
            event.encode(&mut b);
            assert_eq!(a, b, "non-deterministic encoding for variant {name}");
        }

        // Byte-equality assertion. We collect ALL mismatches first so a
        // single run surfaces every diverged variant — useful when
        // recapturing after an authorized wire-format change (paste each
        // printed literal back into its EXPECTED_* constant in one go).
        let mut mismatches: Vec<String> = Vec::new();
        for (name, event, expected) in &cases {
            let mut actual = Vec::new();
            event.encode(&mut actual);
            if actual.as_slice() != *expected {
                mismatches.push(format!(
                    "  {name}: const for this variant should be:\n    {}",
                    fmt_bytes_as_literal(&actual),
                ));
            }
        }
        assert!(
            mismatches.is_empty(),
            "wire-format mismatch in {} variant(s):\n{}",
            mismatches.len(),
            mismatches.join("\n"),
        );
    }

    /// Render a byte slice as a Rust `&[u8]` literal for easy paste-back
    /// into the `EXPECTED_*` constants when (re)capturing the wire format.
    fn fmt_bytes_as_literal(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        let mut s = String::from("&[");
        for (i, b) in bytes.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            let _ = write!(s, "{b}");
        }
        s.push(']');
        s
    }

    // ── Captured wire-format snapshots (RECAPTURE workflow) ──
    //
    // To regenerate after an authorized wire-format change: set the
    // EXPECTED_* slice to `&[]`, run the test, copy the printed
    // `actual = &[…]` literal back into the constant, re-run green.
    // Do NOT recapture casually — the whole point is to detect drift.

    // δ.3c-i (bead adr-fmt-syjan): SweepStarted gained
    // `snapshot_signature: Option<String>` at the tail of its payload
    // (additive evolution per CHE-0022:R5 + PAR-0024:R5). Pardosa-encoding
    // emits payload fields in declaration order, so the new bytes are
    // the prior bytes followed by the `Option` discriminant (and value
    // when `Some`). The regenerated literal for `snapshot_signature:
    // None` is the original δ.1 bytes (commit 1eeedeb) with a single
    // trailing `0u8` — mechanical confirmation of prefix-compatibility.
    // δ.3c-ii (bead adr-fmt-baao9) will populate the production emit
    // with `Some(build_snapshot_signature(...))` but this fixture stays
    // `None` because byte-equality cases need stable small payloads.
    const EXPECTED_SWEEP_STARTED: &[u8] = &[
        0, 8, 0, 0, 0, 116, 101, 115, 116, 45, 111, 114, 103, 42, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0,
        98, 97, 116, 99, 104, 45, 48, 48, 49, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49,
        84, 48, 48, 58, 48, 48, 58, 48, 48, 90, 0,
    ];
    const EXPECTED_REPO_EVALUATED_NONE: &[u8] = &[
        1, 9, 0, 0, 0, 105, 100, 45, 114, 101, 112, 111, 45, 49, 6, 0, 0, 0, 114, 101, 112, 111,
        45, 49, 1, 15, 0, 0, 0, 115, 99, 104, 101, 100, 117, 108, 101, 100, 95, 98, 97, 116, 99,
        104, 210, 4, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48,
        48, 58, 48, 48, 58, 48, 48, 90, 0,
    ];
    const EXPECTED_REPO_EVALUATED_SOME: &[u8] = &[
        1, 9, 0, 0, 0, 105, 100, 45, 114, 101, 112, 111, 45, 49, 6, 0, 0, 0, 114, 101, 112, 111,
        45, 49, 1, 15, 0, 0, 0, 115, 99, 104, 101, 100, 117, 108, 101, 100, 95, 98, 97, 116, 99,
        104, 210, 4, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48,
        48, 58, 48, 48, 58, 48, 48, 90, 1, 9, 0, 0, 0, 105, 100, 45, 114, 101, 112, 111, 45, 49, 0,
        6, 0, 0, 0, 114, 101, 112, 111, 45, 49, 0, 0, 4, 0, 0, 0, 109, 97, 105, 110, 0, 9, 0, 0, 0,
        105, 100, 45, 114, 101, 112, 111, 45, 49, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 25,
        0, 0, 0, 50, 48, 50, 54, 45, 48, 52, 45, 48, 57, 84, 49, 50, 58, 48, 48, 58, 48, 48, 43,
        48, 48, 58, 48, 48, 0, 1, 0, 1, 0, 25, 0, 0, 0, 50, 48, 50, 54, 45, 48, 52, 45, 48, 57, 84,
        49, 50, 58, 48, 48, 58, 48, 48, 43, 48, 48, 58, 48, 48, 0, 0, 25, 0, 0, 0, 50, 48, 50, 54,
        45, 48, 52, 45, 48, 57, 84, 49, 50, 58, 48, 48, 58, 48, 48, 43, 48, 48, 58, 48, 48, 0, 4,
        0, 0, 0, 109, 97, 105, 110, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 25, 0, 0, 0, 50, 48,
        50, 54, 45, 48, 52, 45, 48, 57, 84, 49, 50, 58, 48, 48, 58, 48, 48, 43, 48, 48, 58, 48, 48,
        0, 1, 18, 0, 0, 0, 46, 103, 105, 116, 104, 117, 98, 47, 67, 79, 68, 69, 79, 87, 78, 69, 82,
        83, 25, 0, 0, 0, 50, 48, 50, 54, 45, 48, 52, 45, 48, 57, 84, 49, 50, 58, 48, 48, 58, 48,
        48, 43, 48, 48, 58, 48, 48, 0, 0, 0,
    ];
    const EXPECTED_REPO_REMOVED: &[u8] = &[
        2, 11, 0, 0, 0, 105, 100, 45, 111, 108, 100, 45, 114, 101, 112, 111, 8, 0, 0, 0, 111, 108,
        100, 45, 114, 101, 112, 111, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48,
        48, 58, 48, 48, 58, 48, 48, 90,
    ];
    const EXPECTED_SWEEP_COMPLETED: &[u8] = &[
        3, 9, 0, 0, 0, 98, 97, 116, 99, 104, 45, 48, 48, 49, 136, 19, 0, 0, 0, 0, 0, 0, 42, 0, 0,
        0, 0, 0, 0, 0, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48, 48, 58, 48, 48,
        58, 48, 48, 90,
    ];
    const EXPECTED_WEBHOOK_RECEIVED: &[u8] = &[
        4, 7, 0, 0, 0, 101, 110, 113, 117, 101, 117, 101, 1, 7, 0, 0, 0, 109, 121, 45, 114, 101,
        112, 111, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48, 48, 58, 48, 48, 58,
        48, 48, 90,
    ];
    const EXPECTED_EVIDENCE_PUBLISHED: &[u8] = &[
        5, 5, 0, 0, 0, 0, 0, 0, 0, 1, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48,
        48, 58, 48, 48, 58, 48, 48, 90,
    ];
    const EXPECTED_PARTIAL_EVIDENCE_RENDERED: &[u8] = &[
        6, 9, 0, 0, 0, 98, 97, 116, 99, 104, 45, 48, 48, 49, 3, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0,
        0, 0, 0, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48, 48, 58, 48, 48, 58,
        48, 48, 90,
    ];
    const EXPECTED_SWEEP_FAILED: &[u8] = &[
        7, 9, 0, 0, 0, 98, 97, 116, 99, 104, 45, 48, 48, 49, 7, 0, 0, 0, 116, 105, 109, 101, 111,
        117, 116, 32, 28, 0, 0, 0, 0, 0, 0, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49,
        84, 48, 48, 58, 48, 48, 58, 48, 48, 90,
    ];
    const EXPECTED_SWEEP_PROGRESS: &[u8] = &[
        8, 9, 0, 0, 0, 98, 97, 116, 99, 104, 45, 48, 48, 49, 25, 0, 0, 0, 0, 0, 0, 0, 100, 0, 0, 0,
        0, 0, 0, 0, 20, 0, 0, 0, 50, 48, 50, 52, 45, 48, 49, 45, 48, 49, 84, 48, 48, 58, 48, 48,
        58, 48, 48, 90,
    ];

    #[test]
    fn event_type_returns_correct_discriminator() {
        let ts = now_str();
        let cases: Vec<(DomainEvent, &str)> = vec![
            (
                DomainEvent::SweepStarted {
                    org: "o".into(),
                    repo_count: 1,
                    batch_id: "b".into(),
                    timestamp: ts.clone(),
                    snapshot_signature: None,
                },
                "SweepStarted",
            ),
            (
                DomainEvent::RepoEvaluated {
                    domain_key: "k".into(),
                    repo_name: "r".into(),
                    success: true,
                    source: "s".into(),
                    duration_ms: 0,
                    timestamp: ts.clone(),
                    evidence: None,
                },
                "RepoEvaluated",
            ),
            (
                DomainEvent::RepoRemoved {
                    domain_key: "k".into(),
                    repo_name: "r".into(),
                    timestamp: ts.clone(),
                },
                "RepoRemoved",
            ),
            (
                DomainEvent::SweepCompleted {
                    batch_id: "b".into(),
                    duration_ms: 0,
                    repo_count: 0,
                    timestamp: ts.clone(),
                },
                "SweepCompleted",
            ),
            (
                DomainEvent::WebhookReceived {
                    action: "a".into(),
                    repo: None,
                    timestamp: ts.clone(),
                },
                "WebhookReceived",
            ),
            (
                DomainEvent::EvidencePublished {
                    page_count: 0,
                    warm_start: false,
                    timestamp: ts.clone(),
                },
                "EvidencePublished",
            ),
            (
                DomainEvent::PartialEvidenceRendered {
                    batch_id: "b".into(),
                    page_count: 0,
                    pending_repos: 0,
                    timestamp: ts.clone(),
                },
                "PartialEvidenceRendered",
            ),
            (
                DomainEvent::SweepFailed {
                    batch_id: "b".into(),
                    error: "e".into(),
                    duration_ms: 0,
                    timestamp: ts.clone(),
                },
                "SweepFailed",
            ),
            (
                DomainEvent::SweepProgress {
                    batch_id: "b".into(),
                    completed: 0,
                    total: 0,
                    timestamp: ts,
                },
                "SweepProgress",
            ),
        ];

        for (event, expected) in &cases {
            assert_eq!(
                event.event_type(),
                *expected,
                "event_type() mismatch for {expected}",
            );
        }
    }
}
