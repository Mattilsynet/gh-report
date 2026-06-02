use cherry_pit_core::{Aggregate, HandleCommand};
use gh_report::domain::aggregates::repo::{
    RecordEvaluation, RecordRemoval, Repo, RepoError, RepoPhase,
};
use gh_report::domain::aggregates::run::{
    CompleteSweep, FailSweep, PublishEvidence, RecordProgress, RenderPartial, Run, RunError,
    RunPhase,
};
use gh_report::domain::aggregates::webhook::{
    DeliveryPhase, RecordDelivery, WebhookDelivery, WebhookError,
};
use gh_report::domain::events::DomainEvent;
use proptest::prelude::*;

const TS: &str = "2026-05-10T12:00:00Z";

fn fresh_run_started(batch: &str) -> Run {
    let mut r = Run::default();
    r.apply(&DomainEvent::SweepStarted {
        org: "org".into(),
        repo_count: 1,
        batch_id: batch.into(),
        timestamp: TS.into(),
        snapshot_signature: None,
    });
    r
}

#[derive(Debug, Clone)]
enum RunCmd {
    Progress(u64, u64),
    Render(u64, u64),
    Complete,
    Fail,
    Publish,
}

fn arb_run_cmd() -> impl Strategy<Value = RunCmd> {
    prop_oneof![
        (0u64..1000, 0u64..1000).prop_map(|(a, b)| RunCmd::Progress(a, b)),
        (0u64..1000, 0u64..1000).prop_map(|(a, b)| RunCmd::Render(a, b)),
        Just(RunCmd::Complete),
        Just(RunCmd::Fail),
        Just(RunCmd::Publish),
    ]
}

fn apply_run_cmd(run: &mut Run, cmd: &RunCmd) -> Result<(), RunError> {
    let events: Vec<DomainEvent> = match cmd {
        RunCmd::Progress(c, t) => run.handle(RecordProgress {
            batch_id: "b".into(),
            completed: *c,
            total: *t,
            timestamp: TS.into(),
        })?,
        RunCmd::Render(p, q) => run.handle(RenderPartial {
            batch_id: "b".into(),
            page_count: *p,
            pending_repos: *q,
            timestamp: TS.into(),
        })?,
        RunCmd::Complete => run.handle(CompleteSweep {
            batch_id: "b".into(),
            duration_ms: 1,
            repo_count: 1,
            timestamp: TS.into(),
        })?,
        RunCmd::Fail => run.handle(FailSweep {
            batch_id: "b".into(),
            error: "e".into(),
            duration_ms: 1,
            timestamp: TS.into(),
        })?,
        RunCmd::Publish => run.handle(PublishEvidence {
            page_count: 1,
            warm_start: false,
            timestamp: TS.into(),
        })?,
    };
    for ev in &events {
        run.apply(ev);
    }
    Ok(())
}

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
    fn run_terminal_xor_holds_under_arbitrary_sequences(
        cmds in proptest::collection::vec(arb_run_cmd(), 0..32usize),
    ) {
        let mut run = fresh_run_started("b");
        let mut terminal_observed: Option<RunPhase> = None;
        for cmd in &cmds {
            let phase_before = run.phase;
            match apply_run_cmd(&mut run, cmd) {
                Ok(()) => {
                    prop_assert_ne!(
                        run.phase,
                        RunPhase::Empty,
                        "CHE-0054:R1.a — Run must not regress to Empty after SweepStarted"
                    );
                    if matches!(
                        cmd,
                        RunCmd::Complete | RunCmd::Fail | RunCmd::Publish
                    ) {
                        prop_assert!(
                            terminal_observed.is_none()
                                || matches!(
                                    (terminal_observed, &run.phase, cmd),
                                    (Some(RunPhase::Completed), RunPhase::Published, RunCmd::Publish)
                                ),
                            "CHE-0054:R1.b — terminal xor violated: prev={:?} new={:?} via {:?}",
                            terminal_observed,
                            run.phase,
                            cmd
                        );
                        if matches!(run.phase, RunPhase::Completed | RunPhase::Failed | RunPhase::Published) {
                            terminal_observed = Some(run.phase);
                        }
                    }
                }
                Err(_) => {
                    prop_assert_eq!(
                        run.phase,
                        phase_before,
                        "rejected command must not mutate phase"
                    );
                }
            }
        }
    }

    #[test]
    fn run_after_terminal_rejects_all_phase_changing_commands(
        prefix in proptest::collection::vec(arb_run_cmd(), 0..8usize),
        terminator in prop_oneof![Just(RunCmd::Complete), Just(RunCmd::Fail)],
        post in proptest::collection::vec(arb_run_cmd(), 1..8usize),
    ) {
        let mut run = fresh_run_started("b");
        for c in &prefix {
            let _ = apply_run_cmd(&mut run, c);
        }
        if run.phase != RunPhase::Started {
            return Ok(());
        }
        apply_run_cmd(&mut run, &terminator).expect("terminator from Started succeeds");
        let terminal_phase = run.phase;
        prop_assert!(matches!(terminal_phase, RunPhase::Completed | RunPhase::Failed));
        for c in &post {
            let before = run.phase;
            let res = apply_run_cmd(&mut run, c);
            let is_terminal_publish = matches!(
                (&terminal_phase, c, &res),
                (RunPhase::Completed, RunCmd::Publish, Ok(()))
            );
            if is_terminal_publish {
                prop_assert_eq!(run.phase, RunPhase::Published);
            } else {
                prop_assert!(
                    res.is_err(),
                    "post-terminal command {:?} from {:?} must be rejected",
                    c,
                    terminal_phase
                );
                prop_assert_eq!(run.phase, before, "rejected command kept phase");
            }
        }
    }

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
            prop_assert!(res.is_err(), "CHE-0054:R2.c — no events follow RepoRemoved");
            prop_assert_eq!(res.unwrap_err(), RepoError::AlreadyRemoved);
            prop_assert_eq!(repo.phase, RepoPhase::Removed);
            prop_assert_eq!(repo.evaluation_count, count_before);
        }
    }

    #[test]
    fn repo_evaluation_count_matches_successful_evaluate_handles(
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

    #[test]
    fn webhook_delivery_is_fresh_per_call(
        del_id1 in "[a-z]{3,8}",
        del_id2 in "[a-z]{3,8}",
    ) {
        let agg = WebhookDelivery::default();
        prop_assert_eq!(agg.phase, DeliveryPhase::Empty);
        let events = agg
            .handle(RecordDelivery {
                delivery_id: del_id1.clone(),
                action: "enqueue".into(),
                repo: None,
                timestamp: TS.into(),
            })
            .expect("first delivery succeeds on fresh aggregate");
        prop_assert_eq!(events.len(), 1);

        let mut agg2 = WebhookDelivery::default();
        for ev in &events {
            agg2.apply(ev);
        }
        prop_assert_ne!(agg2.phase, DeliveryPhase::Empty);
        let err = agg2
            .handle(RecordDelivery {
                delivery_id: del_id2,
                action: "enqueue".into(),
                repo: None,
                timestamp: TS.into(),
            })
            .expect_err("CHE-0054:R3 — second RecordDelivery on same aggregate rejected");
        prop_assert!(matches!(err, WebhookError::AlreadyReceived));
    }
}
