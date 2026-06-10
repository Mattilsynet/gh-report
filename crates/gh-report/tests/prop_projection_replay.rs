use std::num::NonZeroU64;

use cherry_pit_core::{AggregateId, EventEnvelope, Projection};
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::events::{DomainEvent, RepoPresence};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};
use gh_report::projection::EvidenceProjection;
use proptest::collection::vec;
use proptest::prelude::*;

const TS: &str = "2026-04-20T12:00:00Z";

fn ev(name: &str) -> RepositoryEvidence {
    let ts = TS.to_string();
    RepositoryEvidence {
        repository: Repository {
            id: format!("id-{name}"),
            node_id: None,
            name: name.to_string(),
            visibility: Visibility::Public,
            language: None,
            default_branch: "main".to_string(),
            archived: false,
            has_issues: true,
            inventory_key: format!("id-{name}"),
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
                timestamp: ts.clone(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: None,
                timestamp: ts.clone(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: None,
                timestamp: ts.clone(),
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
                timestamp: ts.clone(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp: ts,
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
    }
}

fn ev_envelope(name: &str, seq: u64) -> EventEnvelope<DomainEvent> {
    EventEnvelope::new(
        uuid::Uuid::now_v7(),
        AggregateId::new(NonZeroU64::new(1).expect("non-zero")),
        NonZeroU64::new(seq).expect("non-zero"),
        jiff::Timestamp::now(),
        None,
        None,
        DomainEvent::RepositoryStateCaptured {
            domain_key: format!("id-{name}"),
            repo_name: name.into(),
            timestamp: TS.into(),
            evidence: Some(Box::new(ev(name))),
            presence: RepoPresence::Active,
        },
    )
    .expect("valid envelope")
}

fn rm_envelope(name: &str, seq: u64) -> EventEnvelope<DomainEvent> {
    EventEnvelope::new(
        uuid::Uuid::now_v7(),
        AggregateId::new(NonZeroU64::new(1).expect("non-zero")),
        NonZeroU64::new(seq).expect("non-zero"),
        jiff::Timestamp::now(),
        None,
        None,
        DomainEvent::RepositoryStateCaptured {
            domain_key: format!("id-{name}"),
            repo_name: name.into(),
            timestamp: TS.into(),
            evidence: None,
            presence: RepoPresence::Removed,
        },
    )
    .expect("valid envelope")
}

#[derive(Debug, Clone)]
enum Op {
    Eval(&'static str),
    Remove(&'static str),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    let names: &[&'static str] = &["a", "b", "c", "d"];
    prop_oneof![
        prop::sample::select(names.to_vec()).prop_map(Op::Eval),
        prop::sample::select(names.to_vec()).prop_map(Op::Remove),
    ]
}

fn apply_op(p: &mut EvidenceProjection, op: &Op, seq: u64) {
    match op {
        Op::Eval(name) => p.apply(&ev_envelope(name, seq)),
        Op::Remove(name) => p.apply(&rm_envelope(name, seq)),
    }
}

fn shuffle<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut out = items.to_vec();
    let n = out.len();
    let mut h = DefaultHasher::new();
    seed.hash(&mut h);
    for i in (1..n).rev() {
        let mut hh = DefaultHasher::new();
        (h.finish(), i as u64).hash(&mut hh);
        let j = usize::try_from(hh.finish()).unwrap_or(usize::MAX) % (i + 1);
        out.swap(i, j);
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn apply_is_idempotent_on_repeated_envelope(
        idx in 0usize..4usize,
        repeats in 1u8..=4u8,
    ) {
        let names = ["a", "b", "c", "d"];
        let name = names[idx];
        let env = ev_envelope(name, 1);

        let mut once = EvidenceProjection::default();
        once.apply(&env);
        let mut many = EvidenceProjection::default();
        for _ in 0..repeats {
            many.apply(&env);
        }
        prop_assert_eq!(once.sorted_snapshot(), many.sorted_snapshot());
        prop_assert_eq!(once.len(), 1);
    }

    #[test]
    fn remove_eval_remove_converges_to_empty(idx in 0usize..4usize) {
        let names = ["a", "b", "c", "d"];
        let name = names[idx];
        let mut p = EvidenceProjection::default();
        p.apply(&rm_envelope(name, 1));
        prop_assert!(p.is_empty());
        p.apply(&ev_envelope(name, 2));
        prop_assert_eq!(p.len(), 1);
        p.apply(&rm_envelope(name, 3));
        prop_assert!(p.is_empty());
    }

    #[test]
    fn final_key_set_equals_last_op_eval_set(
        ops in vec(op_strategy(), 1..16),
    ) {
        let mut p = EvidenceProjection::default();
        for (i, op) in ops.iter().enumerate() {
            apply_op(&mut p, op, (i + 1) as u64);
        }
        let actual: std::collections::BTreeSet<String> =
            p.sorted_snapshot().into_iter().map(|e| e.repository.id).collect();

        let mut last: std::collections::BTreeMap<&'static str, bool> =
            std::collections::BTreeMap::new();
        for op in &ops {
            match op {
                Op::Eval(n) => { last.insert(n, true); }
                Op::Remove(n) => { last.insert(n, false); }
            }
        }
        let expected: std::collections::BTreeSet<String> = last.into_iter()
            .filter_map(|(n, eval)| if eval { Some(format!("id-{n}")) } else { None })
            .collect();
        prop_assert_eq!(&actual, &expected,
            "Per-key last-writer-wins: key in final state iff last op on key is Eval");
    }

    #[test]
    fn load_baseline_order_independence(
        idxs in vec(0usize..4usize, 1..6),
        perm_seed in any::<u64>(),
    ) {
        let names = ["a", "b", "c", "d"];
        let entries: Vec<RepositoryEvidence> = idxs.iter().map(|&i| ev(names[i])).collect();
        let mut p1 = EvidenceProjection::default();
        p1.load_baseline(entries.clone());
        let permuted = shuffle(&entries, perm_seed);
        let mut p2 = EvidenceProjection::default();
        p2.load_baseline(permuted);
        prop_assert_eq!(p1.sorted_snapshot(), p2.sorted_snapshot());
    }
}
