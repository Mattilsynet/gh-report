//! Domain events representing state transitions in the system.
//!
//! These events model what happened in the domain, not what should happen
//! (commands) or how it was delivered (infrastructure). Each variant
//! captures the essential facts of a state transition.
//!
//! ## Serialization
//!
//! Events use `#[serde(tag = "type")]` for a discriminated union format:
//! ```json
//! { "type": "repo_evaluated", "domain_key": "...", "success": true, ... }
//! ```
//! This format is forward-compatible: new fields can be added to variants
//! without breaking existing deserializers (they ignore unknown fields).
//! Variant names are stable — renaming or removing a variant is a breaking
//! change that requires a schema version bump.
//!
//! ## Variant evolution (CHE-0022:R5)
//!
//! `DomainEvent` is **not** `#[non_exhaustive]`: CHE-0022:R5 forbids the
//! attribute on domain event enums — they are versioned via additive
//! variants and additive `#[serde(default)]` fields, not attribute hedging.
//! The exhaustive `match` in [`DomainEvent::event_type`] (no wildcard arm)
//! plus the `event_type_matches_serde_tag` test cover the documentation
//! purpose `#[non_exhaustive]` previously served: any new variant produces
//! a compile error in `event_type()`, forcing the maintainer to update
//! the discriminator table.

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEvent {
    /// A scheduled sweep has started.
    SweepStarted {
        /// Target organization.
        org: String,
        /// Number of repositories to evaluate.
        repo_count: usize,
        /// Unique identifier for this sweep batch.
        batch_id: String,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    },

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
        /// landed (CHE-0022 additive evolution via `#[serde(default)]`).
        ///
        /// Boxed because `RepositoryEvidence` is ~560 bytes and would
        /// otherwise dominate the enum's stack footprint (the other 7
        /// variants are tens of bytes); `Option<Box<_>>` also benefits
        /// from the null-pointer niche optimisation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        evidence: Option<Box<RepositoryEvidence>>,
    },

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
    },

    /// A scheduled sweep has completed.
    SweepCompleted {
        /// Unique identifier matching the originating [`SweepStarted::batch_id`].
        batch_id: String,
        /// Total sweep duration in milliseconds.
        duration_ms: u64,
        /// Number of repositories evaluated.
        repo_count: usize,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    },

    /// A webhook was received and validated.
    WebhookReceived {
        /// Mapped action (e.g., `"enqueue"`, `"remove"`, `"ignore"`).
        action: String,
        /// Repository name, if applicable.
        repo: Option<String>,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    },

    /// Evidence was published (HTML cache updated, WebSocket broadcast sent).
    EvidencePublished {
        /// Number of HTML pages in the cache.
        page_count: usize,
        /// Whether this was a warm-start publish (from baseline, no API calls).
        warm_start: bool,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    },

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
        page_count: usize,
        /// Repos still pending evaluation at render time.
        pending_repos: usize,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    },

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
    },

    /// Progress update during a sweep (emitted at phase transitions).
    SweepProgress {
        /// Unique identifier matching the originating [`SweepStarted::batch_id`].
        batch_id: String,
        /// Number of repositories completed so far (resumed + baseline + evaluated).
        completed: usize,
        /// Total number of repositories in the sweep.
        total: usize,
        /// ISO 8601 UTC timestamp.
        timestamp: String,
    },
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
    /// The returned value matches the serde `#[serde(tag = "type")]`
    /// discriminator in serialized JSON. This correspondence is enforced
    /// by the `event_type_matches_serde_tag` test.
    ///
    /// The match is intentionally exhaustive (no wildcard arm) so that
    /// adding a new `DomainEvent` variant produces a compile error here,
    /// forcing the maintainer to add the corresponding `event_type()` arm.
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::SweepStarted { .. } => "sweep_started",
            Self::RepoEvaluated { .. } => "repo_evaluated",
            Self::RepoRemoved { .. } => "repo_removed",
            Self::SweepCompleted { .. } => "sweep_completed",
            Self::WebhookReceived { .. } => "webhook_received",
            Self::EvidencePublished { .. } => "evidence_published",
            Self::PartialEvidenceRendered { .. } => "partial_evidence_rendered",
            Self::SweepFailed { .. } => "sweep_failed",
            Self::SweepProgress { .. } => "sweep_progress",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_str() -> String {
        jiff::Timestamp::now().to_string()
    }

    #[test]
    fn sweep_started_serialization_round_trip() {
        let event = DomainEvent::SweepStarted {
            org: "my-org".into(),
            repo_count: 42,
            batch_id: "batch-001".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"sweep_started""#));
        assert!(json.contains(r#""org":"my-org""#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DomainEvent::SweepStarted { repo_count: 42, .. }
        ));
    }

    #[test]
    fn repo_evaluated_serialization_round_trip() {
        let event = DomainEvent::RepoEvaluated {
            domain_key: "id-my-repo".into(),
            repo_name: "my-repo".into(),
            success: true,
            source: "scheduled_batch".into(),
            duration_ms: 1234,
            timestamp: "2026-04-20T12:00:00Z".into(),
            evidence: None,
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"repo_evaluated""#));
        assert!(json.contains(r#""success":true"#));
        // `None` evidence is skipped per `skip_serializing_if`.
        assert!(
            !json.contains(r#""evidence":"#),
            "None evidence must not serialize: {json}"
        );

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DomainEvent::RepoEvaluated { success: true, .. }
        ));
    }

    #[test]
    fn repo_evaluated_with_evidence_round_trip() {
        // B6': `evidence: Some(RepositoryEvidence)` round-trips through
        // both JSON and msgpack. CHE-0022 additive evolution probe — the
        // payload must survive serialization without losing fields.
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

        // JSON round-trip.
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            json.contains(r#""evidence":"#),
            "evidence must serialize: {json}"
        );
        let back: DomainEvent = serde_json::from_str(&json).unwrap();
        match back {
            DomainEvent::RepoEvaluated {
                evidence: Some(ev), ..
            } => {
                assert_eq!(*ev, evidence);
            }
            other => panic!("expected RepoEvaluated with Some(evidence), got {other:?}"),
        }

        // msgpack round-trip (matches the on-disk EventStore path).
        let mp = rmp_serde::to_vec_named(&event).unwrap();
        let back: DomainEvent = rmp_serde::from_slice(&mp).unwrap();
        match back {
            DomainEvent::RepoEvaluated {
                evidence: Some(ev), ..
            } => {
                assert_eq!(*ev, evidence);
            }
            other => panic!("expected RepoEvaluated with Some(evidence), got {other:?}"),
        }
    }

    #[test]
    fn repo_evaluated_pre_b6_json_deserializes_with_default_evidence() {
        // CHE-0022 backward-compat probe: a `RepoEvaluated` envelope
        // serialized BEFORE B6' added the `evidence` field must still
        // deserialize. The `#[serde(default)]` attribute on `evidence`
        // makes the field optional on the wire; missing field → `None`.
        // If this test fails, B6' has broken backward compat — abort
        // per charter §5.1.
        let pre_b6_json = r#"{
            "type": "repo_evaluated",
            "domain_key": "id-r",
            "repo_name": "r",
            "success": true,
            "source": "scheduled_batch",
            "duration_ms": 100,
            "timestamp": "2026-04-20T12:00:00Z"
        }"#;
        let ev: DomainEvent =
            serde_json::from_str(pre_b6_json).expect("pre-B6' JSON must deserialize");
        match ev {
            DomainEvent::RepoEvaluated {
                evidence, success, ..
            } => {
                assert!(success);
                assert!(
                    evidence.is_none(),
                    "missing field → None per #[serde(default)]"
                );
            }
            other => panic!("expected RepoEvaluated, got {other:?}"),
        }
    }

    #[test]
    fn repo_removed_serialization_round_trip() {
        let event = DomainEvent::RepoRemoved {
            domain_key: "id-old-repo".into(),
            repo_name: "old-repo".into(),
            timestamp: "2026-04-20T12:00:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"repo_removed""#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, DomainEvent::RepoRemoved { .. }));
    }

    #[test]
    fn sweep_completed_serialization_round_trip() {
        let event = DomainEvent::SweepCompleted {
            batch_id: "batch-001".into(),
            duration_ms: 5000,
            repo_count: 42,
            timestamp: "2026-04-20T12:00:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"sweep_completed""#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DomainEvent::SweepCompleted {
                duration_ms: 5000,
                ..
            }
        ));
    }

    #[test]
    fn webhook_received_serialization_round_trip() {
        let event = DomainEvent::WebhookReceived {
            action: "enqueue".into(),
            repo: Some("my-repo".into()),
            timestamp: "2026-04-20T12:00:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"webhook_received""#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, DomainEvent::WebhookReceived { .. }));
    }

    #[test]
    fn evidence_published_serialization_round_trip() {
        let event = DomainEvent::EvidencePublished {
            page_count: 5,
            warm_start: false,
            timestamp: "2026-04-20T12:00:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"evidence_published""#));
        assert!(json.contains(r#""warm_start":false"#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DomainEvent::EvidencePublished {
                page_count: 5,
                warm_start: false,
                ..
            }
        ));
    }

    #[test]
    fn partial_evidence_rendered_serialization_round_trip() {
        let event = DomainEvent::PartialEvidenceRendered {
            batch_id: "b1".into(),
            page_count: 3,
            pending_repos: 7,
            timestamp: "2026-04-20T12:00:00Z".into(),
        };

        // JSON round-trip.
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"partial_evidence_rendered""#));
        assert!(json.contains(r#""batch_id":"b1""#));
        assert!(json.contains(r#""page_count":3"#));
        assert!(json.contains(r#""pending_repos":7"#));

        let back: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            DomainEvent::PartialEvidenceRendered {
                page_count: 3,
                pending_repos: 7,
                ..
            }
        ));

        // msgpack round-trip (matches the on-disk EventStore path per CHE-0031).
        let mp = rmp_serde::to_vec_named(&event).unwrap();
        let back: DomainEvent = rmp_serde::from_slice(&mp).unwrap();
        match back {
            DomainEvent::PartialEvidenceRendered {
                batch_id,
                page_count,
                pending_repos,
                ..
            } => {
                assert_eq!(batch_id, "b1");
                assert_eq!(page_count, 3);
                assert_eq!(pending_repos, 7);
            }
            other => panic!("expected PartialEvidenceRendered, got {other:?}"),
        }
    }

    #[test]
    fn sweep_failed_serialization_round_trip() {
        let event = DomainEvent::SweepFailed {
            batch_id: "batch-001".into(),
            error: "timeout after 7200s".into(),
            duration_ms: 7_200_000,
            timestamp: "2026-04-20T14:00:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"sweep_failed""#));
        assert!(json.contains(r#""error":"timeout after 7200s""#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DomainEvent::SweepFailed {
                duration_ms: 7_200_000,
                ..
            }
        ));
    }

    #[test]
    fn sweep_progress_serialization_round_trip() {
        let event = DomainEvent::SweepProgress {
            batch_id: "batch-001".into(),
            completed: 25,
            total: 100,
            timestamp: "2026-04-20T12:30:00Z".into(),
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"sweep_progress""#));
        assert!(json.contains(r#""completed":25"#));
        assert!(json.contains(r#""total":100"#));

        let deserialized: DomainEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            DomainEvent::SweepProgress {
                completed: 25,
                total: 100,
                ..
            }
        ));
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

    #[test]
    fn tag_stability_known_json_strings() {
        // Verify that known JSON strings deserialize correctly.
        // If a variant is renamed, this test will catch it.
        let cases = [
            (
                r#"{"type":"sweep_started","org":"o","repo_count":1,"batch_id":"b","timestamp":"t"}"#,
                "sweep_started",
            ),
            (
                r#"{"type":"repo_evaluated","domain_key":"k","repo_name":"r","success":true,"source":"s","duration_ms":0,"timestamp":"t"}"#,
                "repo_evaluated",
            ),
            (
                r#"{"type":"repo_removed","domain_key":"k","repo_name":"r","timestamp":"t"}"#,
                "repo_removed",
            ),
            (
                r#"{"type":"sweep_completed","batch_id":"b","duration_ms":0,"repo_count":0,"timestamp":"t"}"#,
                "sweep_completed",
            ),
            (
                r#"{"type":"webhook_received","action":"a","repo":null,"timestamp":"t"}"#,
                "webhook_received",
            ),
            (
                r#"{"type":"evidence_published","page_count":0,"warm_start":false,"timestamp":"t"}"#,
                "evidence_published",
            ),
            (
                r#"{"type":"partial_evidence_rendered","batch_id":"b","page_count":0,"pending_repos":0,"timestamp":"t"}"#,
                "partial_evidence_rendered",
            ),
            (
                r#"{"type":"sweep_failed","batch_id":"b","error":"e","duration_ms":0,"timestamp":"t"}"#,
                "sweep_failed",
            ),
            (
                r#"{"type":"sweep_progress","batch_id":"b","completed":0,"total":0,"timestamp":"t"}"#,
                "sweep_progress",
            ),
        ];

        for (json, expected_tag) in cases {
            let event: DomainEvent = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("failed to deserialize {expected_tag}: {e}"));
            let reserialized = serde_json::to_string(&event).unwrap();
            assert!(
                reserialized.contains(&format!(r#""type":"{expected_tag}""#)),
                "tag mismatch for {expected_tag}"
            );
        }
    }

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
                },
                "sweep_started",
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
                "repo_evaluated",
            ),
            (
                DomainEvent::RepoRemoved {
                    domain_key: "k".into(),
                    repo_name: "r".into(),
                    timestamp: ts.clone(),
                },
                "repo_removed",
            ),
            (
                DomainEvent::SweepCompleted {
                    batch_id: "b".into(),
                    duration_ms: 0,
                    repo_count: 0,
                    timestamp: ts.clone(),
                },
                "sweep_completed",
            ),
            (
                DomainEvent::WebhookReceived {
                    action: "a".into(),
                    repo: None,
                    timestamp: ts.clone(),
                },
                "webhook_received",
            ),
            (
                DomainEvent::EvidencePublished {
                    page_count: 0,
                    warm_start: false,
                    timestamp: ts.clone(),
                },
                "evidence_published",
            ),
            (
                DomainEvent::PartialEvidenceRendered {
                    batch_id: "b".into(),
                    page_count: 0,
                    pending_repos: 0,
                    timestamp: ts.clone(),
                },
                "partial_evidence_rendered",
            ),
            (
                DomainEvent::SweepFailed {
                    batch_id: "b".into(),
                    error: "e".into(),
                    duration_ms: 0,
                    timestamp: ts.clone(),
                },
                "sweep_failed",
            ),
            (
                DomainEvent::SweepProgress {
                    batch_id: "b".into(),
                    completed: 0,
                    total: 0,
                    timestamp: ts,
                },
                "sweep_progress",
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

    #[test]
    fn event_type_matches_serde_tag() {
        // Ensures event_type() stays in sync with serde's "type" tag.
        // If serde rename_all or a variant rename changes the tag,
        // this test catches the divergence.
        let ts = now_str();
        let events: Vec<DomainEvent> = vec![
            DomainEvent::SweepStarted {
                org: "o".into(),
                repo_count: 1,
                batch_id: "b".into(),
                timestamp: ts.clone(),
            },
            DomainEvent::RepoEvaluated {
                domain_key: "k".into(),
                repo_name: "r".into(),
                success: true,
                source: "s".into(),
                duration_ms: 0,
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
                duration_ms: 0,
                repo_count: 0,
                timestamp: ts.clone(),
            },
            DomainEvent::WebhookReceived {
                action: "a".into(),
                repo: None,
                timestamp: ts.clone(),
            },
            DomainEvent::EvidencePublished {
                page_count: 0,
                warm_start: false,
                timestamp: ts.clone(),
            },
            DomainEvent::PartialEvidenceRendered {
                batch_id: "b".into(),
                page_count: 0,
                pending_repos: 0,
                timestamp: ts.clone(),
            },
            DomainEvent::SweepFailed {
                batch_id: "b".into(),
                error: "e".into(),
                duration_ms: 0,
                timestamp: ts.clone(),
            },
            DomainEvent::SweepProgress {
                batch_id: "b".into(),
                completed: 0,
                total: 0,
                timestamp: ts,
            },
        ];

        for event in &events {
            let json = serde_json::to_value(event).unwrap();
            let serde_tag = json["type"].as_str().unwrap_or("MISSING");
            assert_eq!(
                event.event_type(),
                serde_tag,
                "event_type() diverged from serde tag for {event:?}",
            );
        }
    }
}
