use super::state::{AppendResult, Line};
use crate::error::PardosaError;
use crate::event::{Event, EventId, FiberId, Index, Precursor, event_id_to_line_position_or_panic};
use crate::fiber::Fiber;
use crate::fiber_state::{FiberMigrationPolicy, FiberState, LockedRescuePolicy};
use crate::frontier::Frontier;
use std::collections::{HashMap, HashSet};
#[test]
fn create_assigns_monotonic_event_id() {
    let mut d = Line::new();
    let r1 = d.create("first").unwrap();
    let r2 = d.create("second").unwrap();
    let r3 = d.create("third").unwrap();
    assert_eq!(r1.event_id, EventId::new(0));
    assert_eq!(r2.event_id, EventId::new(1));
    assert_eq!(r3.event_id, EventId::new(2));
}
#[test]
fn create_assigns_monotonic_fiber_id() {
    let mut d = Line::new();
    let r1 = d.create("first").unwrap();
    let r2 = d.create("second").unwrap();
    assert_eq!(r1.fiber_id, FiberId::new(0));
    assert_eq!(r2.fiber_id, FiberId::new(1));
}
#[test]
fn create_assigns_sequential_indices() {
    let mut d = Line::new();
    let r1 = d.create("first").unwrap();
    let r2 = d.create("second").unwrap();
    assert_eq!(Index::from_decoded(r1.event_id.value()), Index::new(0));
    assert_eq!(Index::from_decoded(r2.event_id.value()), Index::new(1));
}
#[test]
fn create_sets_state_to_defined() {
    let mut d = Line::new();
    let r = d.create("first").unwrap();
    assert_eq!(d.fiber_state(r.fiber_id), FiberState::Defined);
}
#[test]
fn create_event_has_none_precursor() {
    let mut d = Line::new();
    let r = d.create("first").unwrap();
    let event = &d.read_line()[event_id_to_line_position_or_panic(r.event_id)];
    assert!(event.precursor().is_genesis());
}
#[test]
fn create_update_detach_lifecycle_event_ids() {
    let mut d = Line::new();
    let r1 = d.create("created").unwrap();
    let fiber_id = r1.fiber_id;
    let r2 = d.update(fiber_id, "updated").unwrap();
    assert_eq!(d.fiber_state(fiber_id), FiberState::Defined);
    let r3 = d.detach(fiber_id, "detached").unwrap();
    assert_eq!(d.fiber_state(fiber_id), FiberState::Detached);
    assert!(r1.event_id < r2.event_id);
    assert!(r2.event_id < r3.event_id);
}
#[test]
fn update_sets_precursor_to_previous_event() {
    let mut d = Line::new();
    let r1 = d.create("created").unwrap();
    let r2 = d.update(r1.fiber_id, "updated").unwrap();
    let event = &d.read_line()[event_id_to_line_position_or_panic(r2.event_id)];
    assert_eq!(
        event.precursor(),
        Precursor::Of(Index::from_decoded(r1.event_id.value())),
    );
}
#[test]
fn detach_sets_detached_flag() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let r2 = d.detach(r.fiber_id, "detached").unwrap();
    let event = &d.read_line()[event_id_to_line_position_or_panic(r2.event_id)];
    assert!(event.detached());
}
#[test]
fn rescue_from_detached_continues_chain() {
    let mut d = Line::new();
    let r1 = d.create("created").unwrap();
    let r2 = d.detach(r1.fiber_id, "detached").unwrap();
    let r3 = d
        .rescue(
            r1.fiber_id,
            LockedRescuePolicy::PreserveAuditTrail,
            "rescued",
        )
        .unwrap();
    assert_eq!(d.fiber_state(r1.fiber_id), FiberState::Defined);
    let event = &d.read_line()[event_id_to_line_position_or_panic(r3.event_id)];
    assert_eq!(
        event.precursor(),
        Precursor::Of(Index::from_decoded(r2.event_id.value())),
    );
    assert!(!event.detached());
}
#[test]
fn rescue_from_locked_rejects_with_typed_policy_error() {
    let mut d = Line::new();
    let r1 = d.create("created").unwrap();
    let _ = d.detach(r1.fiber_id, "detached").unwrap();
    d.migrate_fiber(r1.fiber_id, FiberMigrationPolicy::LockAndPrune)
        .unwrap();
    assert_eq!(d.fiber_state(r1.fiber_id), FiberState::Locked);
    let err = d
        .rescue(
            r1.fiber_id,
            LockedRescuePolicy::PreserveAuditTrail,
            "rescued",
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            PardosaError::RescuePolicyUnsupported {
                policy: LockedRescuePolicy::PreserveAuditTrail,
                state: FiberState::Locked,
            }
        ),
        "expected RescuePolicyUnsupported(PreserveAuditTrail, Locked), got: {err:?}"
    );
    assert_eq!(
        d.fiber_state(r1.fiber_id),
        FiberState::Locked,
        "rejected rescue must not mutate fiber state"
    );
    let err = d
        .rescue(r1.fiber_id, LockedRescuePolicy::AcceptDataLoss, "rescued")
        .unwrap_err();
    assert!(
        matches!(
            err,
            PardosaError::RescuePolicyUnsupported {
                policy: LockedRescuePolicy::AcceptDataLoss,
                ..
            }
        ),
        "expected RescuePolicyUnsupported(AcceptDataLoss, ..), got: {err:?}"
    );
}
#[test]
fn rescue_from_undefined_fails() {
    let mut d = Line::<&str>::new();
    let err = d
        .rescue(
            FiberId::new(99),
            LockedRescuePolicy::PreserveAuditTrail,
            "nope",
        )
        .unwrap_err();
    assert!(
        matches!(err, PardosaError::FiberNotFound(_)),
        "expected FiberNotFound, got: {err}"
    );
}
#[test]
fn create_advances_past_purged_ids_in_long_run() {
    let mut d = Line::new();
    let mut created: Vec<FiberId> = Vec::new();
    for _ in 0..50_u64 {
        let r = d.create("x").unwrap();
        created.push(r.fiber_id);
    }
    for (i, id) in created.iter().enumerate() {
        if i % 2 == 0 {
            let _ = d.detach(*id, "det").unwrap();
            d.migrate_fiber(*id, FiberMigrationPolicy::Purge).unwrap();
        }
    }
    for _ in 3000..3050_u64 {
        let r = d
            .create("y")
            .expect("create() must not stall on liveness across purges");
        assert!(!matches!(d.fiber_state(r.fiber_id), FiberState::Purged));
    }
}
#[test]
fn create_skips_purged_when_next_id_collides() {
    let mut purged_ids = HashSet::new();
    purged_ids.insert(FiberId::new(5));
    purged_ids.insert(FiberId::new(6));
    purged_ids.insert(FiberId::new(7));
    let mut d = Line::<&str>::from_parts_unchecked(
        Vec::new(),
        HashMap::new(),
        purged_ids,
        FiberId::new(5),
        EventId::new(0),
        false,
        Frontier::GENESIS,
    )
    .unwrap();
    let r = d.create("fresh").expect("create() must skip purged ids");
    assert_eq!(
        r.fiber_id,
        FiberId::new(8),
        "create() should have skipped 5, 6, 7"
    );
    assert_eq!(d.next_fiber_id(), FiberId::new(9));
}
#[test]
fn create_overflows_when_remaining_ids_all_purged() {
    let mut purged_ids = HashSet::new();
    purged_ids.insert(FiberId::new(u64::MAX));
    let mut d = Line::<&str>::from_parts_unchecked(
        Vec::new(),
        HashMap::new(),
        purged_ids,
        FiberId::new(u64::MAX),
        EventId::new(0),
        false,
        Frontier::GENESIS,
    )
    .unwrap();
    let err = d.create("x").unwrap_err();
    assert!(
        matches!(err, PardosaError::FiberIdOverflow),
        "expected FiberIdOverflow, got: {err}"
    );
}
#[test]
fn verify_precursor_chains_valid() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.update(r.fiber_id, "u1").unwrap();
    let _ = d.update(r.fiber_id, "u2").unwrap();
    assert!(d.verify_precursor_chains().is_ok());
}
#[test]
fn verify_precursor_chains_multi_fiber_valid() {
    let mut d = Line::new();
    let r1 = d.create("a-create").unwrap();
    let r2 = d.create("b-create").unwrap();
    let _ = d.update(r1.fiber_id, "a-update").unwrap();
    let _ = d.update(r2.fiber_id, "b-update").unwrap();
    let _ = d.update(r1.fiber_id, "a-update2").unwrap();
    assert!(d.verify_precursor_chains().is_ok());
}
#[test]
fn verify_precursor_chains_broken_wrong_fiber_id() {
    let mut d = Line::new();
    let _ = d.create("a-create").unwrap();
    let _ = d.create("b-create").unwrap();
    let bad_event = Event::new_unchecked(
        99,
        FiberId::new(99),
        false,
        Precursor::Of(Index::new(0)),
        [0u8; 32],
        "broken",
    );
    d.line.force_push_unchecked(bad_event);
    let err = d.verify_precursor_chains().unwrap_err();
    assert!(
        matches!(err, PardosaError::BrokenPrecursorChain { .. }),
        "expected BrokenPrecursorChain, got: {err}"
    );
}
#[test]
fn verify_precursor_chains_broken_forward_reference() {
    let mut d = Line::new();
    let _ = d.create("created").unwrap();
    let bad_event = Event::new_unchecked(
        99,
        FiberId::new(0),
        false,
        Precursor::Of(Index::new(5)),
        [0u8; 32],
        "broken",
    );
    d.line.force_push_unchecked(bad_event);
    let err = d.verify_precursor_chains().unwrap_err();
    assert!(
        matches!(err, PardosaError::BrokenPrecursorChain { .. }),
        "expected BrokenPrecursorChain, got: {err}"
    );
}
#[test]
fn verify_precursor_chains_broken_self_reference() {
    let mut d = Line::new();
    let _ = d.create("created").unwrap();
    let bad_event = Event::new_unchecked(
        99,
        FiberId::new(0),
        false,
        Precursor::Of(Index::new(1)),
        [0u8; 32],
        "broken",
    );
    d.line.force_push_unchecked(bad_event);
    let err = d.verify_precursor_chains().unwrap_err();
    assert!(
        matches!(err, PardosaError::BrokenPrecursorChain { .. }),
        "expected BrokenPrecursorChain, got: {err}"
    );
}
#[test]
fn verify_precursor_chains_detects_tampered_predecessor() {
    let mut d = Line::<&'static str>::new();
    let r = d.create("created").unwrap();
    let _ = d.update(r.fiber_id, "u1").unwrap();
    let _ = d.update(r.fiber_id, "u2").unwrap();
    assert!(d.verify_precursor_chains().is_ok());
    let mut line = d.line.as_slice().to_vec();
    let original = line[1].clone();
    let tampered = Event::new_unchecked(
        original.event_id(),
        original.fiber_id(),
        original.detached(),
        original.precursor(),
        original.precursor_hash(),
        "TAMPERED",
    );
    let expected_mismatch_event_id = line[2].event_id();
    line[1] = tampered;
    let err = Line::from_parts_unchecked(
        line,
        d.lookup.clone(),
        d.purged_ids.clone(),
        d.next_id,
        d.next_event_id,
        false,
        Frontier::GENESIS,
    )
    .unwrap_err();
    match err {
        PardosaError::PrecursorHashMismatch {
            event_id,
            expected,
            actual,
        } => {
            assert_eq!(
                event_id,
                expected_mismatch_event_id.value(),
                "mismatch should pinpoint the successor (line[2]), not the tampered event itself",
            );
            assert_ne!(expected, actual, "hashes must differ on tamper");
        }
        other => panic!("expected PrecursorHashMismatch, got: {other}"),
    }
}
#[test]
fn verify_precursor_chains_valid_chain_is_regression_canary() {
    let mut d = Line::<&'static str>::new();
    let r = d.create("created").unwrap();
    let _ = d.update(r.fiber_id, "u1").unwrap();
    let _ = d.update(r.fiber_id, "u2").unwrap();
    let _ = d.update(r.fiber_id, "u3").unwrap();
    assert!(d.verify_precursor_chains().is_ok());
}
#[test]
fn read_defined_fiber() {
    let mut d = Line::new();
    let r = d.create("hello").unwrap();
    let event = d.read(r.fiber_id).unwrap();
    assert_eq!(*event.domain_event(), "hello");
    assert_eq!(event.event_id(), r.event_id);
}
#[test]
fn read_returns_latest_event() {
    let mut d = Line::new();
    let r = d.create("v1").unwrap();
    let _ = d.update(r.fiber_id, "v2").unwrap();
    let _ = d.update(r.fiber_id, "v3").unwrap();
    let event = d.read(r.fiber_id).unwrap();
    assert_eq!(*event.domain_event(), "v3");
}
#[test]
fn read_detached_fiber_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    assert!(d.read(r.fiber_id).is_err());
}
#[test]
fn read_unknown_fiber_id_fails() {
    let d = Line::<&str>::new();
    assert!(d.read(FiberId::new(0)).is_err());
}
#[test]
fn read_with_deleted_returns_detached() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    let event = d.read_with_deleted(r.fiber_id).unwrap();
    assert!(event.detached());
    assert_eq!(*event.domain_event(), "detached");
}
#[test]
fn read_with_deleted_returns_locked() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::LockAndPrune)
        .unwrap();
    let event = d.read_with_deleted(r.fiber_id).unwrap();
    assert!(event.detached());
}
#[test]
fn read_with_deleted_purged_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Purge)
        .unwrap();
    assert!(d.read_with_deleted(r.fiber_id).is_err());
}
#[test]
fn list_only_defined() {
    let mut d = Line::new();
    let r1 = d.create("a").unwrap();
    let r2 = d.create("b").unwrap();
    let _r3 = d.create("c").unwrap();
    let _ = d.detach(r2.fiber_id, "detached").unwrap();
    let listed = d.list();
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&r1.fiber_id));
    assert!(!listed.contains(&r2.fiber_id));
}
#[test]
fn list_with_deleted_includes_detached_and_locked() {
    let mut d = Line::new();
    let r1 = d.create("a").unwrap();
    let r2 = d.create("b").unwrap();
    let r3 = d.create("c").unwrap();
    let _ = d.detach(r2.fiber_id, "detached-b").unwrap();
    let _ = d.detach(r3.fiber_id, "detached-c").unwrap();
    d.migrate_fiber(r3.fiber_id, FiberMigrationPolicy::LockAndPrune)
        .unwrap();
    let listed = d.list_with_deleted();
    assert_eq!(listed.len(), 3);
    assert!(listed.contains(&r1.fiber_id));
    assert!(listed.contains(&r2.fiber_id));
    assert!(listed.contains(&r3.fiber_id));
}
#[test]
fn list_with_deleted_excludes_purged() {
    let mut d = Line::new();
    let r1 = d.create("a").unwrap();
    let r2 = d.create("b").unwrap();
    let _ = d.detach(r2.fiber_id, "detached").unwrap();
    d.migrate_fiber(r2.fiber_id, FiberMigrationPolicy::Purge)
        .unwrap();
    let listed = d.list_with_deleted();
    assert_eq!(listed.len(), 1);
    assert!(listed.contains(&r1.fiber_id));
}
#[test]
fn list_empty_dragline() {
    let d = Line::<&str>::new();
    assert!(d.list().is_empty());
    assert!(d.list_with_deleted().is_empty());
}
#[test]
fn history_returns_chronological_order() {
    let mut d = Line::new();
    let r = d.create("v1").unwrap();
    let _ = d.update(r.fiber_id, "v2").unwrap();
    let _ = d.update(r.fiber_id, "v3").unwrap();
    let hist = d.history(r.fiber_id).unwrap();
    assert_eq!(hist.len(), 3);
    assert_eq!(*hist[0].domain_event(), "v1");
    assert_eq!(*hist[1].domain_event(), "v2");
    assert_eq!(*hist[2].domain_event(), "v3");
}
#[test]
fn history_single_event() {
    let mut d = Line::new();
    let r = d.create("only").unwrap();
    let hist = d.history(r.fiber_id).unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(*hist[0].domain_event(), "only");
}
#[test]
fn history_includes_detach_event() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.update(r.fiber_id, "updated").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    let hist = d.history(r.fiber_id).unwrap();
    assert_eq!(hist.len(), 3);
    assert!(hist[2].detached());
}
#[test]
fn history_after_rescue_from_locked_shows_only_new_event() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.update(r.fiber_id, "updated").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::LockAndPrune)
        .unwrap();
    let err = d
        .rescue(r.fiber_id, LockedRescuePolicy::AcceptDataLoss, "rescued")
        .unwrap_err();
    assert!(
        matches!(
            err,
            PardosaError::RescuePolicyUnsupported {
                policy: LockedRescuePolicy::AcceptDataLoss,
                state: FiberState::Locked,
            }
        ),
        "expected RescuePolicyUnsupported(AcceptDataLoss, Locked), got: {err:?}"
    );
    assert_eq!(d.fiber_state(r.fiber_id), FiberState::Locked);
}
#[test]
fn history_purged_fiber_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Purge)
        .unwrap();
    assert!(d.history(r.fiber_id).is_err());
}
#[test]
fn read_line_returns_all_events() {
    let mut d = Line::new();
    let r1 = d.create("a").unwrap();
    let r2 = d.create("b").unwrap();
    let _ = d.update(r1.fiber_id, "a-update").unwrap();
    let line = d.read_line();
    assert_eq!(line.len(), 3);
    assert_eq!(line[0].fiber_id(), r1.fiber_id);
    assert_eq!(line[1].fiber_id(), r2.fiber_id);
    assert_eq!(line[2].fiber_id(), r1.fiber_id);
}
#[test]
fn migration_in_progress_rejects_create() {
    let mut d = Line::new();
    d.set_migrating(true);
    assert!(matches!(
        d.create("should fail"),
        Err(PardosaError::MigrationInProgress)
    ));
}
#[test]
fn migration_in_progress_rejects_update() {
    let mut d = Line::new();
    let r = d.create("ok").unwrap();
    d.set_migrating(true);
    assert!(matches!(
        d.update(r.fiber_id, "should fail"),
        Err(PardosaError::MigrationInProgress)
    ));
}
#[test]
fn migration_in_progress_rejects_detach() {
    let mut d = Line::new();
    let r = d.create("ok").unwrap();
    d.set_migrating(true);
    assert!(matches!(
        d.detach(r.fiber_id, "should fail"),
        Err(PardosaError::MigrationInProgress)
    ));
}
#[test]
fn migration_in_progress_rejects_rescue() {
    let mut d = Line::new();
    let r = d.create("ok").unwrap();
    let _ = d.detach(r.fiber_id, "detach").unwrap();
    d.set_migrating(true);
    assert!(matches!(
        d.rescue(
            r.fiber_id,
            LockedRescuePolicy::PreserveAuditTrail,
            "should fail",
        ),
        Err(PardosaError::MigrationInProgress)
    ));
}
#[test]
fn reads_work_during_migration() {
    let mut d = Line::new();
    let r = d.create("ok").unwrap();
    d.set_migrating(true);
    assert!(d.read(r.fiber_id).is_ok());
    assert!(!d.list().is_empty());
    assert!(!d.list_with_deleted().is_empty());
    assert!(d.history(r.fiber_id).is_ok());
    assert!(!d.read_line().is_empty());
}
#[test]
fn update_on_detached_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    assert!(matches!(
        d.update(r.fiber_id, "nope"),
        Err(PardosaError::InvalidTransition { .. })
    ));
}
#[test]
fn detach_on_detached_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    assert!(matches!(
        d.detach(r.fiber_id, "nope"),
        Err(PardosaError::InvalidTransition { .. })
    ));
}
#[test]
fn update_on_unknown_fails() {
    let mut d = Line::<&str>::new();
    assert!(matches!(
        d.update(FiberId::new(0), "nope"),
        Err(PardosaError::FiberNotFound(_))
    ));
}
#[test]
fn event_id_overflow() {
    let mut d = Line::new();
    d.next_event_id = EventId::new(u64::MAX);
    assert!(matches!(
        d.create("overflow"),
        Err(PardosaError::EventIdOverflow)
    ));
}
#[test]
fn fiber_id_overflow() {
    let mut d = Line::new();
    d.next_id = FiberId::new(u64::MAX);
    assert!(matches!(
        d.create("overflow"),
        Err(PardosaError::FiberIdOverflow)
    ));
}
#[test]
fn migrate_keep_preserves_detached() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Keep)
        .unwrap();
    assert_eq!(d.fiber_state(r.fiber_id), FiberState::Detached);
}
#[test]
fn migrate_purge_removes_from_lookup() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Purge)
        .unwrap();
    assert_eq!(d.fiber_state(r.fiber_id), FiberState::Purged);
}
#[test]
fn migrate_lock_and_prune_sets_locked() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::LockAndPrune)
        .unwrap();
    assert_eq!(d.fiber_state(r.fiber_id), FiberState::Locked);
}
#[test]
fn migrate_defined_fiber_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    assert!(matches!(
        d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Keep),
        Err(PardosaError::InvalidTransition { .. })
    ));
}
#[test]
fn migrate_locked_purge_escalation() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::LockAndPrune)
        .unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Purge)
        .unwrap();
    assert_eq!(d.fiber_state(r.fiber_id), FiberState::Purged);
}
#[test]
fn default_creates_empty_dragline() {
    let d = Line::<String>::default();
    assert_eq!(d.line_len(), 0);
    assert_eq!(d.next_event_id(), EventId::new(0));
    assert_eq!(d.next_fiber_id(), FiberId::new(0));
    assert!(!d.is_migrating());
}
#[test]
fn fiber_state_reports_undefined() {
    let d = Line::<&str>::new();
    assert_eq!(d.fiber_state(FiberId::new(0)), FiberState::Undefined);
}
#[test]
fn read_with_deleted_on_defined_fiber() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let event = d.read_with_deleted(r.fiber_id).unwrap();
    assert_eq!(*event.domain_event(), "created");
}
#[test]
fn history_through_detach_and_rescue() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.update(r.fiber_id, "updated").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    let _ = d
        .rescue(
            r.fiber_id,
            LockedRescuePolicy::PreserveAuditTrail,
            "rescued",
        )
        .unwrap();
    let _ = d.update(r.fiber_id, "post-rescue").unwrap();
    let hist = d.history(r.fiber_id).unwrap();
    assert_eq!(hist.len(), 5);
    assert_eq!(*hist[0].domain_event(), "created");
    assert_eq!(*hist[3].domain_event(), "rescued");
    assert_eq!(*hist[4].domain_event(), "post-rescue");
}
#[test]
fn migrate_fiber_unknown_fiber_id_fails() {
    let mut d = Line::<&str>::new();
    assert!(matches!(
        d.migrate_fiber(FiberId::new(99), FiberMigrationPolicy::Purge),
        Err(PardosaError::FiberNotFound(_))
    ));
}
#[test]
fn migrate_fiber_purged_fiber_id_fails() {
    let mut d = Line::new();
    let r = d.create("created").unwrap();
    let _ = d.detach(r.fiber_id, "detached").unwrap();
    d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Purge)
        .unwrap();
    assert!(matches!(
        d.migrate_fiber(r.fiber_id, FiberMigrationPolicy::Purge),
        Err(PardosaError::FiberNotFound(_))
    ));
}
mod proptests {
    use super::*;
    use proptest::prelude::*;
    #[derive(Debug, Clone)]
    enum TestAction {
        Create,
        UpdateFirst,
        DetachFirst,
        RescueFirst,
        MigrateFirstPurge,
        MigrateFirstLockAndPrune,
    }
    fn arb_action() -> impl Strategy<Value = TestAction> {
        prop_oneof![
            3 => Just(TestAction::Create), 2 => Just(TestAction::UpdateFirst), 1 =>
            Just(TestAction::DetachFirst), 1 => Just(TestAction::RescueFirst), 1 =>
            Just(TestAction::MigrateFirstPurge), 1 =>
            Just(TestAction::MigrateFirstLockAndPrune),
        ]
    }
    proptest! {
        #[test] fn arbitrary_sequences_preserve_precursor_chains(actions in
        prop::collection::vec(arb_action(), 1..100)) { let mut d = Line::< String
        >::new(); let mut defined : Vec < FiberId > = Vec::new(); let mut detached : Vec
        < FiberId > = Vec::new(); let mut locked : Vec < FiberId > = Vec::new(); let mut
        clk = 0u64; for action in & actions { clk += 1; match action { TestAction::Create
        => { let r = d.create(format!("c{clk}"),).unwrap(); defined.push(r.fiber_id); }
        TestAction::UpdateFirst => { if let Some(& id) = defined.first() { let _ = d
        .update(id, format!("u{clk}"),); } } TestAction::DetachFirst => { if let Some(id)
        = defined.pop() { if d.detach(id, format!("d{clk}")).is_ok() { detached.push(id);
        } else { defined.push(id); } } } TestAction::RescueFirst => { if let Some(id) =
        detached.pop() { if d.rescue(id, LockedRescuePolicy::PreserveAuditTrail,
        format!("r{clk}"),).is_ok() { defined.push(id); } else { detached.push(id); } } }
        TestAction::MigrateFirstPurge => { if let Some(id) = detached.pop() { if d
        .migrate_fiber(id, FiberMigrationPolicy::Purge).is_ok() { let _ = id; } else {
        detached.push(id); } } else if let Some(id) = locked.pop() { if d
        .migrate_fiber(id, FiberMigrationPolicy::Purge).is_ok() { let _ = id; } else {
        locked.push(id); } } } TestAction::MigrateFirstLockAndPrune => { if let Some(id)
        = detached.pop() { if d.migrate_fiber(id, FiberMigrationPolicy::LockAndPrune)
        .is_ok() { locked.push(id); } else { detached.push(id); } } } } } prop_assert!(d
        .verify_precursor_chains().is_ok()); prop_assert_eq!(usize::try_from(d
        .next_event_id().value()).unwrap(), d.line_len()); for (i, event) in d
        .read_line().iter().enumerate() { prop_assert_eq!(event.event_id().value(),
        u64::try_from(i).unwrap()); } } #[test] fn
        monotonic_event_ids_across_creates(count in 1..50usize) { let mut d = Line::<
        String >::new(); let mut prev_event_id = None; for i in 0..count { let r = d
        .create(format!("e{i}")).unwrap(); if let Some(prev) = prev_event_id {
        prop_assert!(r.event_id > prev, "event_id not monotonic: {} <= {}", r.event_id,
        prev); } prev_event_id = Some(r.event_id); } } #[test] fn
        commit_atomic_preserves_state_on_err(writer_pick in 0u8..4, failure_mode in 0u8
        ..3, seed_creates in 1usize..8,) { let mut d = Line::< String >::new(); let mut
        ids : Vec < FiberId > = Vec::new(); for i in 0..seed_creates { let r = d
        .create(format!("seed{i}"),).unwrap(); ids.push(r.fiber_id); } let detached_id =
        if ids.len() >= 2 { let id = ids[1]; let _ = d.detach(id, "det".into()).unwrap();
        Some(id) } else { None }; match failure_mode { 0 => { d.set_migrating(true); } 1
        => { let line : Vec < Event < String >> = d.read_line().to_vec(); let lookup :
        HashMap < FiberId, (Fiber, FiberState) > = ids.iter().filter_map(| id | { let s =
        d.fiber_state(* id); if matches!(s, FiberState::Undefined) { None } else {
        Some((* id, (d.lookup.get(id).unwrap().0.clone(), s))) } }).collect(); let
        next_event_id = d.next_event_id(); d = Line::< String
        >::from_parts_unchecked(line, lookup, HashSet::new(), FiberId::new(u64::MAX),
        next_event_id, false, Frontier::GENESIS,).unwrap(); } _ => {} } let
        pre_next_event_id = d.next_event_id(); let pre_line_len = d.line_len(); let
        pre_lookup_snapshot : HashMap < FiberId, FiberState > = ids.iter().map(| id | (*
        id, d.fiber_state(* id))).collect(); let pre_next_id = d.next_fiber_id(); let
        bogus = FiberId::new(u64::MAX); let target_id = if failure_mode == 2 { bogus }
        else { detached_id.unwrap_or(ids[0]) }; let result : Result < AppendResult,
        PardosaError > = match writer_pick { 0 => d.create("x".into(),), 1 => d
        .update(target_id, "x".into()), 2 => d.detach(ids[0], "x".into()), _ => d
        .rescue(target_id, LockedRescuePolicy::PreserveAuditTrail, "x".into(),), }; if
        result.is_err() { prop_assert_eq!(d.next_event_id(), pre_next_event_id,
        "next_event_id advanced on Err"); prop_assert_eq!(d.line_len(), pre_line_len,
        "line.len() changed on Err"); prop_assert_eq!(d.next_fiber_id(), pre_next_id,
        "next_fiber_id advanced on Err"); for id in & ids { prop_assert_eq!(d
        .fiber_state(* id), * pre_lookup_snapshot.get(id).unwrap(),
        "fiber state changed on Err for id {:?}", id); } } } #[test] fn
        fiber_advance_overflow_does_not_partial_commit(_dummy in 0..1u8) { let index0 =
        Index::new(0); let fiber = Fiber::new(index0, u64::MAX, index0).unwrap(); let mut
        lookup = HashMap::new(); let fiber_id = FiberId::new(0); lookup.insert(fiber_id,
        (fiber, FiberState::Defined)); let event = Event::new_unchecked(0u64, fiber_id,
        false, Precursor::Genesis, [0u8; 32], "seed".to_string(),); let mut d = Line::<
        String >::from_parts_unchecked(vec![event], lookup, HashSet::new(),
        FiberId::new(1), EventId::new(1), false, Frontier::GENESIS,).unwrap(); let
        pre_event_id = d.next_event_id(); let pre_line_len = d.line_len(); let err = d
        .update(fiber_id, "u".into()).unwrap_err(); prop_assert!(matches!(err,
        PardosaError::FiberInvariant(_)), "expected FiberInvariant, got: {err:?}");
        prop_assert_eq!(d.next_event_id(), pre_event_id,
        "next_event_id advanced on overflow"); prop_assert_eq!(d.line_len(),
        pre_line_len, "line gained an event on overflow"); }
    }
}
