//! Read-side projection for repository evidence.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::evidence::{AssessmentMetadata, RepositoryEvidence};

/// Read-side projection materialising governance evidence from
/// native pardosa events.
///
/// Replaces the v1 bespoke `EvidenceStore`. Stores per-repository evidence
/// keyed by `domain_key` (the `Repository::inventory_key` of the form
/// `"id-<repo-name>"`) plus run-level [`AssessmentMetadata`].
///
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
    /// `None` when projection was built from durable per-repo snapshots only.
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

    /// Bulk-load baseline evidence.
    ///
    /// Merges into existing entries; entries with the same
    /// `inventory_key` overwrite the earlier value (last-writer-wins).
    /// May be called sequentially with [`Self::load_resumed_checkpoint`]
    /// (saga warm-load is W4-then-W3 per `app/collect.rs:537,543`);
    /// the second call adds to rather than replaces the first call's
    /// entries.
    ///
    /// Keyed by `repository.inventory_key`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_projection_is_empty() {
        let p = EvidenceProjection::default();
        assert!(p.repositories.is_empty());
        assert!(p.assessment_metadata.is_none());
    }

    fn ev(domain_key: &str, name: &str) -> RepositoryEvidence {
        use crate::test_fixtures;
        let mut evidence = test_fixtures::all_passing_evidence(name);
        evidence.repository.inventory_key = domain_key.to_string();
        evidence
    }

    #[test]
    fn get_returns_some_after_apply_and_none_otherwise() {
        let mut p = EvidenceProjection::default();
        assert!(p.get("id-repo-1").is_none());
        p.load_baseline(vec![ev("id-repo-1", "repo-1")]);
        let got = p.get("id-repo-1").expect("present after apply");
        assert_eq!(got.repository.name, "repo-1");
        assert!(p.get("id-missing").is_none());
    }

    #[test]
    fn len_matches_inserted_count() {
        let mut p = EvidenceProjection::default();
        assert_eq!(p.len(), 0);
        p.load_baseline(vec![ev("id-a", "a"), ev("id-b", "b")]);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn sorted_snapshot_orders_by_id_then_name() {
        let mut p = EvidenceProjection::default();
        p.load_baseline(vec![ev("id-b", "b"), ev("id-a", "a"), ev("id-c", "c")]);
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
        p.load_baseline(vec![ev("id-prior", "prior")]);
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
