use std::num::NonZeroU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, ScheduleArmed, ScheduleCancelled,
    ScheduleFired, ScheduleId, SchedulerEvent, SchedulerState,
};

fn id(n: u64) -> AggregateId {
    AggregateId::new(NonZeroU64::new(n).unwrap())
}

fn at(second: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(second).unwrap()
}

fn schedule_id(n: u128) -> ScheduleId {
    ScheduleId::from_uuid(uuid::Uuid::from_u128(n))
}

fn context() -> CorrelationContext {
    CorrelationContext::new(uuid::Uuid::from_u128(10), uuid::Uuid::from_u128(11))
}

fn armed(schedule_id: ScheduleId, fire_at: jiff::Timestamp) -> ScheduleArmed {
    ScheduleArmed::new(
        schedule_id,
        fire_at,
        id(7),
        uuid::Uuid::from_u128(12),
        "audit.recorded",
        vec![1, 2, 3],
        context(),
    )
}

fn envelope(sequence: u64, event: SchedulerEvent) -> EventEnvelope<SchedulerEvent> {
    EventEnvelope::new(
        uuid::Uuid::from_u128(u128::from(sequence)),
        id(1),
        NonZeroU64::new(sequence).unwrap(),
        at(1_700_000_000 + i64::try_from(sequence).unwrap()),
        None,
        None,
        event,
    )
    .unwrap()
}

#[test]
fn schedule_event_type_has_separate_schema_identity() {
    assert_eq!(SchedulerEvent::SCHEMA_SOURCE, "cherry-pit/SchedulerEvent");
    assert_ne!(
        SchedulerEvent::SCHEMA_HASH,
        130_161_851_149_130_176_976_202_983_483_756_427_020_u128,
    );
    assert_eq!(
        SchedulerEvent::Armed(armed(schedule_id(1), at(5))).event_type(),
        "scheduler.schedule_armed",
    );
}

#[test]
fn schedule_fired_carries_caller_event_transport() {
    let armed = armed(schedule_id(2), at(5));
    let fired = ScheduleFired::from_armed(&armed);

    assert_eq!(fired.schedule_id(), armed.schedule_id());
    assert_eq!(fired.target_aggregate(), armed.target_aggregate());
    assert_eq!(fired.caller_event_id(), armed.caller_event_id());
    assert_eq!(fired.caller_event_type(), armed.caller_event_type());
    assert_eq!(fired.payload(), armed.payload());
    assert_eq!(fired.correlation(), armed.correlation());
}

#[test]
fn fold_derives_pending_fired_and_cancelled_from_history() {
    let due = armed(schedule_id(3), at(10));
    let fired = armed(schedule_id(4), at(11));
    let cancelled = armed(schedule_id(5), at(12));
    let history = vec![
        envelope(1, SchedulerEvent::Armed(due.clone())),
        envelope(2, SchedulerEvent::Armed(fired.clone())),
        envelope(3, SchedulerEvent::Fired(ScheduleFired::from_armed(&fired))),
        envelope(4, SchedulerEvent::Armed(cancelled.clone())),
        envelope(
            5,
            SchedulerEvent::Cancelled(ScheduleCancelled::new(cancelled.schedule_id())),
        ),
    ];

    let state = SchedulerState::from_history(&history);

    assert_eq!(state.pending().count(), 1);
    assert!(state.pending_schedule(due.schedule_id()).is_some());
    assert!(state.pending_schedule(fired.schedule_id()).is_none());
    assert!(state.pending_schedule(cancelled.schedule_id()).is_none());
    assert_eq!(state.fired().count(), 1);
    assert!(state.fired_schedule(fired.schedule_id()).is_some());
    assert!(state.is_cancelled(cancelled.schedule_id()));
    assert_eq!(state.due_pending(at(10)).count(), 1);
}
