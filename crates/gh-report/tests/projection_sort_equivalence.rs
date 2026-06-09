//! D-CD-4 — `EvidenceProjection::sorted_snapshot()` sort discipline:
//! the snapshot is sorted by `(repository.id, repository.name)` —
//! not by insertion order and not by the underlying `BTreeMap` key
//! (`inventory_key`) order.
//!
//! Mitigates F-LOW-1 from the M2.b linus review (review bead
//! `adr-fmt-1oqi`, report `.ooda/review-linus-m2b-1778488527.md`): the
//! M2.b unit test `sorted_snapshot_orders_by_id_then_name` uses fixtures
//! whose `inventory_key` and `(id, name)` agree, so it cannot
//! discriminate between `BTreeMap` iteration order and the documented
//! `(id, name)` sort contract. This integration test does.
//!
//! Parent: `.ooda/brief-m2cd-readwrite-cutover.md` D-CD-4.
//!
//! ## Fixture design (three-way distinct orderings)
//!
//! Three repositories chosen so insertion order, BTreeMap-key
//! (`inventory_key`) order, and `(id, name)` order are pairwise
//! distinct:
//!
//! | repo | `inventory_key` | `id`     | `name`     |
//! |------|-----------------|----------|------------|
//! | A    | `z-key-1`       | `b-id`   | `a-name`   |
//! | B    | `a-key-2`       | `c-id`   | `b-name`   |
//! | C    | `m-key-3`       | `a-id`   | `c-name`   |
//!
//! - **Insertion order** (A, B, C)             → ids `b, c, a`
//! - **`BTreeMap` key order** (`inventory_key`)  → B, C, A  (`c, a, b`)
//! - **`(id, name)` sort order** (target)      → C, A, B  (`a-id, b-id, c-id`)
//!
//! All three orderings are pairwise distinct. The target-sort
//! assertion therefore falsifies any regression that returns
//! insertion order *or* `BTreeMap` iteration order.

use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};
use gh_report::projection::EvidenceProjection;

/// Build a `RepositoryEvidence` with explicitly-chosen `id`, `name`,
/// and `inventory_key`. The `test_fixtures` helper ties all three to
/// the same `name` argument; we cannot use it here because the whole
/// point of this fixture is to make those three values disagree.
fn ev(inventory_key: &str, id: &str, name: &str) -> RepositoryEvidence {
    let ts = "2026-04-09T12:00:00+00:00";
    RepositoryEvidence {
        repository: Repository {
            id: id.to_string(),
            node_id: None,
            name: name.to_string(),
            visibility: Visibility::Public,
            language: None,
            default_branch: "main".to_string(),
            archived: false,
            has_issues: true,
            inventory_key: inventory_key.to_string(),
            updated_at: None,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            html_url: None,
            topics: vec![],
            license_spdx: None,
        },
        checks: RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: SecurityPolicyStatus::Pass,
                evidence: SecurityPolicyEvidence::Setting,
                path: None,
                timestamp: ts.to_string(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: None,
                timestamp: ts.to_string(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: None,
                timestamp: ts.to_string(),
            },
            branch_protection: BranchProtectionResult {
                status: BranchProtectionStatus::Pass,
                details: BranchProtectionDetails {
                    default_branch: "main".to_string(),
                    has_pr: Some(true),
                    required_reviewers: Some(1),
                    has_status_checks: Some(true),
                    admin_equivalent: Some(true),
                    has_broad_bypass: Some(false),
                    reason: None,
                },
                timestamp: ts.to_string(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp: ts.to_string(),
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
    }
}

#[test]
fn projection_sorted_snapshot_orders_by_id_then_name_distinct_from_insertion_and_key_order() {
    let a = ev("z-key-1", "b-id", "a-name");
    let b = ev("a-key-2", "c-id", "b-name");
    let c = ev("m-key-3", "a-id", "c-name");

    let mut projection = EvidenceProjection::default();
    projection.load_baseline(vec![a.clone(), b.clone(), c.clone()]);

    let snapshot = projection.sorted_snapshot();
    let ids: Vec<&str> = snapshot.iter().map(|e| e.repository.id.as_str()).collect();

    assert_eq!(
        ids,
        vec!["a-id", "b-id", "c-id"],
        "projection sort must be by (repository.id, repository.name), \
         not insertion order (would yield [b-id, c-id, a-id]) and not \
         BTreeMap inventory_key order (would yield [c-id, a-id, b-id])"
    );

    let insertion_ids = ["b-id", "c-id", "a-id"];
    let key_order_ids = ["c-id", "a-id", "b-id"];
    assert_ne!(
        ids.as_slice(),
        insertion_ids,
        "fixture invariant: (id, name) order must differ from insertion order"
    );
    assert_ne!(
        ids.as_slice(),
        key_order_ids,
        "fixture invariant: (id, name) order must differ from inventory_key order"
    );
}
