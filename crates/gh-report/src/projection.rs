//! Read-side projection for the `OrgGovernance` aggregate.
//!
//! WU-6 v2 sub-mission B2' (charter `wu6v2-charter-1778415390`): scaffolding
//! only. This module lands the type + `Projection` impl skeleton so later
//! sub-missions can wire it:
//!
//! - **B3'** wires `PardosaFileEventStore` as the `EventStore`
//!   (δ.3b, CHE-0043).
//! - **B4'** wires `FileProjectionStore<EvidenceProjection>` snapshot persistence.
//! - **B5'** wires `ProjectionDriver` + `InProcessEventBus` (snapshot-fast-path).
//! - **B6'** extends `RepoEvaluated` with a `RepositoryEvidence` payload (CHE-0022
//!   additive). Until then the [`apply`](Projection::apply) body is intentionally
//!   minimal — every [`DomainEvent`] variant is matched exhaustively (a no-op for
//!   most variants), and full payload-driven materialisation arrives at B6' + B7'
//!   (collector rewrite).
//!
//! ## Architectural posture (locked)
//!
//! - **Tension-2** — single aggregate (`OrgGovernance`), single projection
//!   (`EvidenceProjection`). `aggregate_id == org name`. All eight
//!   [`DomainEvent`] variants belong to `OrgGovernance`.
//! - **S5.b bus-only** — no `CommandGateway` / `Aggregate` impl /
//!   `HandleCommand`. [`OrgGovernance`] is a **marker type only**: a
//!   zero-sized struct documenting the consistency boundary. Collectors
//!   write events directly via `event_store.append(...)` + `bus.publish(...)`
//!   (B7'). The cherry-pit-core [`Projection`] trait does **not** carry an
//!   `Aggregate` associated type — the aggregate binding is documentary.
//! - **CHE-0048:R3** — projection consumes events; one projection per
//!   aggregate.
//! - **CHE-0018:R1** — [`apply`](Projection::apply) is synchronous.
//! - **CHE-0009** — [`apply`](Projection::apply) is infallible (no `Result`).
//! - **CHE-0048:R3 + BC-v2-6** — [`apply`](Projection::apply) is idempotent
//!   over the same envelope sequence: replaying the same envelope must
//!   produce the same projection state. The current skeleton trivially
//!   satisfies this (most arms are no-ops; the [`AssessmentMetadata`]
//!   updates are last-writer-wins by `run_id`, deterministic given a
//!   stable event stream).
//!
//! ## `AssessmentMetadata` placement (U3)
//!
//! [`AssessmentMetadata`] lives on the projection as
//! `Option<AssessmentMetadata>` — it is materialised from `SweepStarted` /
//! `SweepCompleted` envelopes (B6' + B7' will populate it from extended
//! event payloads). This is the default home per charter §8 row U3; no
//! DESIGN.md §12 update is required.

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use cherry_pit_core::{AggregateId, DomainEvent as CoreDomainEvent, EventEnvelope, Projection};
use serde::{Deserialize, Serialize};

use crate::domain::events::DomainEvent;
use crate::domain::evidence::{AssessmentMetadata, RepositoryEvidence};

/// Singleton [`AggregateId`] for the [`OrgGovernance`] aggregate.
///
/// Per the **Tension-2 lock** (charter §0 locked posture #2) gh-report runs
/// exactly one aggregate per process — the org-scoped `OrgGovernance`. The
/// cherry-pit-core [`AggregateId`] type is a [`NonZeroU64`], not a string.
/// We therefore pin a singleton numeric id of `1` here; org scoping comes
/// from the parent directory of the [`PardosaFileEventStore`] (
/// `<store_dir>/events/<org>/`), not from the id itself.
///
/// Wired at WU-6 v2 B3' (charter `wu6v2-charter-1778415390`,
/// `AdjustIntent` option 2). Reused at B5' driver wiring and B7' collectors
/// — every `event_store.create` / `event_store.append` / `event_store.load`
/// call in gh-report uses this constant.
///
/// The on-disk artefact is `<store_dir>/events/<org>/1.pardosa`. The
/// `1.pardosa` filename is owned by `PardosaFileEventStore` and is not
/// configurable (cherry-pit-pardosa hard-codes
/// `format!("{}.pardosa", id.get())` per CHE-0036:R1).
pub const ORG_GOVERNANCE_AGGREGATE_ID: AggregateId = AggregateId::new(NonZeroU64::MIN);

/// Marker type for the gh-report consistency boundary.
///
/// A zero-sized type documenting that all eight [`DomainEvent`] variants and
/// the [`EvidenceProjection`] read model belong to a single aggregate per
/// the **Tension-2 lock** (charter §0 locked posture #2).
///
/// `OrgGovernance` is **not** an [`cherry_pit_core::Aggregate`] impl — the
/// **S5.b bus-only lock** (charter §0 locked posture #3) forbids
/// `Aggregate` / `HandleCommand` / `CommandGateway` introduction. This type
/// exists to give the aggregate boundary a name in code and docs; the
/// `aggregate_id` for envelopes is the GitHub organization name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgGovernance;

/// Read-side projection materialising governance evidence from
/// [`DomainEvent`] envelopes.
///
/// Replaces the v1 bespoke `EvidenceStore`. Stores per-repository evidence
/// keyed by `domain_key` (the `Repository::inventory_key` of the form
/// `"id-<repo-name>"`) plus run-level [`AssessmentMetadata`].
///
/// **B2' state**: skeleton. Fields are populated by later sub-missions:
///
/// - B6' extends [`DomainEvent::RepoEvaluated`] with a
///   `RepositoryEvidence` payload; `apply` will then insert into
///   [`Self::repositories`].
/// - B6' / B7' extend `SweepStarted` / `SweepCompleted` with metadata
///   payload; `apply` will then populate [`Self::assessment_metadata`].
///
/// Until B6' lands, [`apply`](Projection::apply) is a no-op for all
/// payload-bearing variants. The exhaustive match guards against new
/// [`DomainEvent`] variants landing without a corresponding projection
/// arm.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EvidenceProjection {
    /// Per-repository evidence keyed by `domain_key`.
    ///
    /// `BTreeMap` for deterministic iteration order — required by
    /// snapshot serialisation (BC-v2-6 idempotency: byte-identical
    /// snapshots for byte-identical event streams) and HTML render
    /// stability (B8').
    pub repositories: BTreeMap<String, RepositoryEvidence>,

    /// Last-known assessment metadata for the current/most-recent
    /// collection run.
    ///
    /// `None` until the first `SweepStarted` envelope is applied (B6').
    /// Updated last-writer-wins by `run_id`.
    pub assessment_metadata: Option<AssessmentMetadata>,
}

impl EvidenceProjection {
    /// Look up evidence for a single repository by `domain_key`.
    ///
    /// Returns an owned clone so read sites stay a literal call-site
    /// rename across the M2.c cutover.
    ///
    /// Per CHE-0048:R2 this projection is the sole reader/writer pair
    /// of its read-model; direct access via this method is the
    /// authorised read path at v0.1 (CHE-0054:R10 — no
    /// `CommandGateway`).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<RepositoryEvidence> {
        self.repositories.get(key).cloned()
    }

    /// Number of repositories currently materialised in the projection.
    ///
    /// Per CHE-0048:R2 the projection owns this count as the sole
    /// reader of its read-model.
    #[must_use]
    pub fn len(&self) -> usize {
        self.repositories.len()
    }

    /// True when no repositories are materialised. Pairs with [`Self::len`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.repositories.is_empty()
    }

    /// Snapshot of all repositories, sorted by `(repository.id,
    /// repository.name)`.
    ///
    /// Per CHE-0048:R2 the projection owns the sort discipline of its
    /// read-model; ordering is `(repository.id, repository.name)` —
    /// required for snapshot byte-identity (BC-v2-6) and HTML render
    /// stability. Underlying storage is already a `BTreeMap<String, _>`
    /// ordered by `domain_key`, but we re-sort by `(id, name)`
    /// explicitly: callers may not rely on `domain_key == id-name`
    /// always agreeing with `(id, name)` lexicographic order.
    ///
    /// Cost: O(n log n) per call; clones the underlying entries.
    #[must_use]
    pub fn sorted_snapshot(&self) -> Vec<RepositoryEvidence> {
        let mut entries: Vec<RepositoryEvidence> = self.repositories.values().cloned().collect();
        entries.sort_by(|a, b| {
            a.repository
                .id
                .cmp(&b.repository.id)
                .then_with(|| a.repository.name.cmp(&b.repository.name))
        });
        entries
    }

    /// Bulk-load baseline evidence at startup, before any bus dispatch.
    ///
    /// Merges into existing entries; entries with the same
    /// `inventory_key` overwrite the earlier value (last-writer-wins).
    /// Per CHE-0048:R2 the projection is the sole writer of its
    /// read-model; this direct mutation is authorised only at
    /// startup, before `build_services` returns and before the bus
    /// is observable (M2 parent brief D2 + pre-mortem #7).
    /// Documentation contract — not runtime-enforced.
    ///
    /// May be called sequentially with [`Self::load_resumed_checkpoint`]
    /// (saga warm-load is W4-then-W3 per `app/collect.rs:537,543`);
    /// the second call adds to rather than replaces the first call's
    /// entries.
    ///
    /// Keyed by `repository.inventory_key` per the baseline-file
    /// loading contract (CHE-0048:R2 — projection owns this key
    /// discipline).
    pub fn load_baseline(&mut self, entries: Vec<RepositoryEvidence>) {
        self.bulk_load(entries);
    }

    /// Bulk-load resumed-checkpoint evidence at startup.
    ///
    /// Same authorisation and merge-semantics as
    /// [`Self::load_baseline`]; separate method to keep call-site
    /// intent visible (W4 vs W3 per `app/collect.rs:1044-1048` vs
    /// `app/collect.rs:618-621`). Bodies are intentionally identical
    /// at v0.1 — the distinction is documentary, mirroring the M2
    /// parent brief D2.
    ///
    /// May be called sequentially with [`Self::load_baseline`]
    /// (saga warm-load calls this first, then `load_baseline`);
    /// the second call adds to rather than replaces this call's
    /// entries.
    pub fn load_resumed_checkpoint(&mut self, entries: Vec<RepositoryEvidence>) {
        self.bulk_load(entries);
    }

    /// Private helper: merge-by-key bulk load.
    ///
    /// Extends the existing map; on `inventory_key` collision the
    /// incoming entry overwrites the existing one (last-writer-wins,
    /// per `BTreeMap::extend` semantics). Used by both
    /// [`Self::load_baseline`] and [`Self::load_resumed_checkpoint`]
    /// so the saga can call them sequentially without one evicting
    /// the other's entries.
    fn bulk_load(&mut self, entries: Vec<RepositoryEvidence>) {
        self.repositories.extend(
            entries
                .into_iter()
                .map(|ev| (ev.repository.inventory_key.clone(), ev)),
        );
    }
}

impl Projection for EvidenceProjection {
    type Event = DomainEvent;

    fn apply(&mut self, envelope: &EventEnvelope<Self::Event>) {
        // Exhaustive match — adding a new `DomainEvent` variant must
        // produce a compile error here, forcing the maintainer to decide
        // whether the projection materialises or ignores it.
        // (B6' will replace many of these no-ops with payload-driven
        // mutations.)
        match envelope.payload() {
            DomainEvent::SweepStarted { .. }
            | DomainEvent::SweepCompleted { .. }
            | DomainEvent::SweepFailed { .. }
            | DomainEvent::SweepProgress { .. }
            | DomainEvent::WebhookReceived { .. }
            | DomainEvent::EvidencePublished { .. }
            | DomainEvent::PartialEvidenceRendered { .. } => {
                // No-op until B6' extends payloads. Placeholder match
                // arms keep the projection idempotent (CHE-0048:R3) and
                // exhaustive against future variants.
            }
            DomainEvent::RepoEvaluated {
                domain_key,
                evidence,
                ..
            } => {
                // B6': insert when the envelope carries evidence. `None`
                // is a no-op (transitional / metadata-only path or pre-B6'
                // legacy envelope per CHE-0022 additive evolution).
                // Idempotent: replaying the same envelope re-inserts the
                // same value (BTreeMap::insert overwrites with identical
                // input → same end state).
                if let Some(ev) = evidence {
                    self.repositories
                        .insert(domain_key.clone(), ev.as_ref().clone());
                }
            }
            DomainEvent::RepoRemoved { domain_key, .. } => {
                // Removal is idempotent on `BTreeMap::remove` — replaying
                // the envelope produces the same end state.
                self.repositories.remove(domain_key);
            }
        }
    }
}

/// Wire gh-report's [`DomainEvent`] into the cherry-pit-core
/// [`CoreDomainEvent`] trait so it satisfies the [`Projection::Event`]
/// bound.
///
/// `event_type()` delegates to the existing inherent
/// [`DomainEvent::event_type`] method, which returns the `PascalCase`
/// variant name as the wire discriminator (post CHE-0065 pivot).
impl CoreDomainEvent for DomainEvent {
    fn event_type(&self) -> &'static str {
        // Delegate to the existing inherent method on `DomainEvent`.
        DomainEvent::event_type(self)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use cherry_pit_core::{AggregateId, EventEnvelope};

    use super::*;

    fn envelope(payload: DomainEvent, sequence: u64) -> EventEnvelope<DomainEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).expect("non-zero")),
            NonZeroU64::new(sequence).expect("non-zero"),
            jiff::Timestamp::now(),
            None,
            None,
            payload,
        )
        .expect("valid envelope")
    }

    #[test]
    fn default_projection_is_empty() {
        let p = EvidenceProjection::default();
        assert!(p.repositories.is_empty());
        assert!(p.assessment_metadata.is_none());
    }

    #[test]
    fn apply_repo_removed_is_idempotent() {
        // Empty start; removing a key that isn't present is a no-op.
        // Replaying the envelope must leave the projection unchanged
        // (CHE-0048:R3 + BC-v2-6 idempotency).
        let mut p = EvidenceProjection::default();
        let env = envelope(
            DomainEvent::RepoRemoved {
                domain_key: "id-ghost".into(),
                repo_name: "ghost".into(),
                timestamp: "2026-04-20T12:00:00Z".into(),
            },
            1,
        );
        p.apply(&env);
        p.apply(&env);
        assert!(p.repositories.is_empty());
    }

    #[test]
    fn apply_skeleton_is_no_op_for_unimplemented_variants() {
        // B6': all variants here remain no-ops on the projection
        // (`RepoEvaluated` carries `evidence: None` in this test, which
        // the B6' arm treats as a no-op; payload-bearing materialisation
        // is exercised by `apply_repo_evaluated_with_evidence_inserts_into_repositories`).
        // `RepoRemoved` is exercised by `apply_repo_removed_is_idempotent`.
        // This test pins the no-op contract so future variants don't
        // silently regress idempotency.
        let mut p = EvidenceProjection::default();
        let ts = "2026-04-20T12:00:00Z".to_string();
        let cases = [
            DomainEvent::SweepStarted {
                org: "org".into(),
                repo_count: 1,
                batch_id: "b".into(),
                timestamp: ts.clone(),
            },
            DomainEvent::RepoEvaluated {
                domain_key: "id-r".into(),
                repo_name: "r".into(),
                success: true,
                source: "s".into(),
                duration_ms: 0,
                timestamp: ts.clone(),
                evidence: None,
            },
            DomainEvent::SweepCompleted {
                batch_id: "b".into(),
                duration_ms: 0,
                repo_count: 1,
                timestamp: ts.clone(),
            },
            DomainEvent::WebhookReceived {
                action: "enqueue".into(),
                repo: None,
                timestamp: ts.clone(),
            },
            DomainEvent::EvidencePublished {
                page_count: 0,
                warm_start: false,
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
                total: 1,
                timestamp: ts,
            },
        ];
        for (i, ev) in cases.into_iter().enumerate() {
            // Sequence numbers are 1-based; offset to keep them unique.
            let seq = (i as u64) + 1;
            p.apply(&envelope(ev, seq));
        }
        assert!(p.repositories.is_empty());
        assert!(p.assessment_metadata.is_none());
    }

    #[test]
    fn apply_repo_evaluated_with_evidence_inserts_into_repositories() {
        // B6': `RepoEvaluated` now carries `evidence: Option<RepositoryEvidence>`.
        // When `Some`, the projection inserts it under `domain_key`. Replaying
        // the same envelope is idempotent (BTreeMap::insert overwrites with
        // the same value — last-writer-wins on identical input).
        use crate::test_fixtures;

        let mut p = EvidenceProjection::default();
        let evidence = test_fixtures::all_passing_evidence("repo-1");
        let env = envelope(
            DomainEvent::RepoEvaluated {
                domain_key: "id-repo-1".into(),
                repo_name: "repo-1".into(),
                success: true,
                source: "scheduled_batch".into(),
                duration_ms: 0,
                timestamp: "2026-04-20T12:00:00Z".into(),
                evidence: Some(Box::new(evidence.clone())),
            },
            1,
        );
        p.apply(&env);
        assert_eq!(p.repositories.len(), 1);
        assert_eq!(p.repositories.get("id-repo-1"), Some(&evidence));

        // Idempotent replay.
        p.apply(&env);
        assert_eq!(p.repositories.len(), 1);
        assert_eq!(p.repositories.get("id-repo-1"), Some(&evidence));
    }

    #[test]
    fn apply_repo_evaluated_without_evidence_is_no_op() {
        // B6': Failure-path emissions may carry `evidence: None` (the
        // failure_evidence helper exists but emitters that don't have a
        // RepositoryEvidence handy can omit it). In that case, the
        // projection makes no entry — the read model only reflects what
        // was actually evaluated.
        let mut p = EvidenceProjection::default();
        let env = envelope(
            DomainEvent::RepoEvaluated {
                domain_key: "id-repo-1".into(),
                repo_name: "repo-1".into(),
                success: false,
                source: "scheduled_batch".into(),
                duration_ms: 0,
                timestamp: "2026-04-20T12:00:00Z".into(),
                evidence: None,
            },
            1,
        );
        p.apply(&env);
        assert!(p.repositories.is_empty());
    }

    #[test]
    fn core_domain_event_impl_returns_pascalcase_discriminator() {
        // Pin the trait impl: `CoreDomainEvent::event_type` must equal
        // the inherent method. Post CHE-0065 pivot, discriminators are
        // PascalCase variant names (no longer serde-derived).
        let ev = DomainEvent::RepoRemoved {
            domain_key: "k".into(),
            repo_name: "r".into(),
            timestamp: "t".into(),
        };
        assert_eq!(
            <DomainEvent as CoreDomainEvent>::event_type(&ev),
            "RepoRemoved"
        );
    }

    // ── M2.b inherent-impl query + bulk-load API ────────────────────

    fn ev_envelope(domain_key: &str, name: &str, seq: u64) -> EventEnvelope<DomainEvent> {
        use crate::test_fixtures;
        envelope(
            DomainEvent::RepoEvaluated {
                domain_key: domain_key.into(),
                repo_name: name.into(),
                success: true,
                source: "scheduled_batch".into(),
                duration_ms: 0,
                timestamp: "2026-04-20T12:00:00Z".into(),
                evidence: Some(Box::new(test_fixtures::all_passing_evidence(name))),
            },
            seq,
        )
    }

    #[test]
    fn get_returns_some_after_apply_and_none_otherwise() {
        let mut p = EvidenceProjection::default();
        assert!(p.get("id-repo-1").is_none());
        p.apply(&ev_envelope("id-repo-1", "repo-1", 1));
        let got = p.get("id-repo-1").expect("present after apply");
        assert_eq!(got.repository.name, "repo-1");
        assert!(p.get("id-missing").is_none());
    }

    #[test]
    fn len_matches_inserted_count() {
        let mut p = EvidenceProjection::default();
        assert_eq!(p.len(), 0);
        p.apply(&ev_envelope("id-a", "a", 1));
        p.apply(&ev_envelope("id-b", "b", 2));
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn sorted_snapshot_orders_by_id_then_name() {
        // Pre-mortem #1 mitigation: insert in non-trivial order
        // (`b`, `a`, `c`) and assert the snapshot comes back sorted
        // by `(repository.id, repository.name)` per the projection's
        // own sort discipline (CHE-0048:R2).
        let mut p = EvidenceProjection::default();
        p.apply(&ev_envelope("id-b", "b", 1));
        p.apply(&ev_envelope("id-a", "a", 2));
        p.apply(&ev_envelope("id-c", "c", 3));
        let snap = p.sorted_snapshot();
        let ids: Vec<&str> = snap.iter().map(|e| e.repository.id.as_str()).collect();
        assert_eq!(ids, vec!["id-a", "id-b", "id-c"]);
    }

    #[test]
    fn sorted_snapshot_of_empty_projection_is_empty() {
        let p = EvidenceProjection::default();
        assert!(p.sorted_snapshot().is_empty());
    }

    #[test]
    fn load_baseline_merges_into_existing_entries() {
        use crate::test_fixtures;
        let mut p = EvidenceProjection::default();
        // Seed an existing entry that load_baseline must preserve
        // (merge-semantics: saga calls W4 load_resumed_checkpoint
        // first, then W3 load_baseline; the second call must not
        // evict the first call's entries).
        p.apply(&ev_envelope("id-prior", "prior", 1));
        let entries = vec![
            test_fixtures::all_passing_evidence("a"),
            test_fixtures::all_passing_evidence("b"),
        ];
        p.load_baseline(entries);
        assert_eq!(p.len(), 3);
        assert!(p.get("id-prior").is_some());
        assert!(p.get("id-a").is_some());
        assert!(p.get("id-b").is_some());
    }

    #[test]
    fn load_baseline_overwrites_same_key_last_writer_wins() {
        use crate::test_fixtures;
        let mut p = EvidenceProjection::default();
        p.load_baseline(vec![test_fixtures::all_passing_evidence("a")]);
        // Re-load same key with a different evidence value; the
        // later call must win (last-writer-wins on key collision).
        let updated = test_fixtures::all_passing_evidence("a");
        p.load_baseline(vec![updated.clone()]);
        assert_eq!(p.len(), 1);
        assert_eq!(p.get("id-a").as_ref(), Some(&updated));
    }

    #[test]
    fn load_baseline_is_idempotent() {
        use crate::test_fixtures;
        let mut p = EvidenceProjection::default();
        let entries = vec![
            test_fixtures::all_passing_evidence("a"),
            test_fixtures::all_passing_evidence("b"),
        ];
        p.load_baseline(entries.clone());
        let first = p.sorted_snapshot();
        p.load_baseline(entries);
        let second = p.sorted_snapshot();
        assert_eq!(first, second);
    }

    #[test]
    fn load_resumed_checkpoint_matches_load_baseline_semantics() {
        use crate::test_fixtures;
        let mut p1 = EvidenceProjection::default();
        let mut p2 = EvidenceProjection::default();
        let entries = vec![
            test_fixtures::all_passing_evidence("a"),
            test_fixtures::all_passing_evidence("b"),
        ];
        p1.load_baseline(entries.clone());
        p2.load_resumed_checkpoint(entries);
        assert_eq!(p1.sorted_snapshot(), p2.sorted_snapshot());
    }
}
