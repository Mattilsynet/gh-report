use cherry_pit_core::{Aggregate, HandleCommand};
use gh_report::domain::aggregates::repo::{
    RecordEvaluation, RecordRemoval, Repo, RepoError, RepoPhase,
};
use proptest::prelude::*;

const TS: &str = "2026-05-10T12:00:00Z";

#[derive(Debug, Clone)]
enum RepoCmd {
    Evaluate(bool),
    Remove,
}

fn arb_repo_cmd() -> impl Strategy<Value = RepoCmd> {
    prop_oneof![
        any::<bool>().prop_map(RepoCmd::Evaluate),
        Just(RepoCmd::Remove),
    ]
}

fn apply_repo_cmd(repo: &mut Repo, cmd: &RepoCmd) -> Result<(), RepoError> {
    let events = match cmd {
        RepoCmd::Evaluate(success) => repo.handle(RecordEvaluation {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            success: *success,
            source: "scheduled_batch".into(),
            duration_ms: 1,
            timestamp: TS.into(),
            evidence: None,
        })?,
        RepoCmd::Remove => repo.handle(RecordRemoval {
            domain_key: "id-r".into(),
            repo_name: "r".into(),
            timestamp: TS.into(),
        })?,
    };
    for ev in &events {
        repo.apply(ev);
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn repo_removal_is_terminal(
        prefix in proptest::collection::vec(arb_repo_cmd(), 0..16usize),
        post in proptest::collection::vec(arb_repo_cmd(), 1..16usize),
    ) {
        let mut repo = Repo::default();
        for c in &prefix {
            let _ = apply_repo_cmd(&mut repo, c);
        }
        if repo.phase == RepoPhase::Removed {
            return Ok(());
        }
        apply_repo_cmd(&mut repo, &RepoCmd::Remove).expect("removal from non-Removed succeeds");
        prop_assert_eq!(repo.phase, RepoPhase::Removed);
        let count_before = repo.evaluation_count;
        for c in &post {
            let res = apply_repo_cmd(&mut repo, c);
            prop_assert!(res.is_err(), "repository tombstone is terminal");
            prop_assert_eq!(res.unwrap_err(), RepoError::AlreadyRemoved);
            prop_assert_eq!(repo.phase, RepoPhase::Removed);
            prop_assert_eq!(repo.evaluation_count, count_before);
        }
    }

    #[test]
    fn repo_evaluation_count_matches_successful_active_snapshots(
        cmds in proptest::collection::vec(arb_repo_cmd(), 0..64usize),
    ) {
        let mut repo = Repo::default();
        let mut expected: u64 = 0;
        for c in cmds {
            let pre_phase = repo.phase;
            let res = apply_repo_cmd(&mut repo, &c);
            if res.is_ok()
                && matches!(c, RepoCmd::Evaluate(_))
                && pre_phase != RepoPhase::Removed
            {
                expected = expected.saturating_add(1);
            }
        }
        prop_assert_eq!(repo.evaluation_count, expected);
    }
}
