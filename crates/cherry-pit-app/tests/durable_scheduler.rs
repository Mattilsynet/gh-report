use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_app::{DurableScheduler, InProcessEventBus, SchedulePayloadDecoder};
use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventStore, ScheduleArmed, ScheduleCancelled,
    ScheduleFired, ScheduleId, ScheduledDomainEvent, SchedulerEvent,
};
use cherry_pit_gateway::MsgpackFileStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum AuditEvent {
    Opened { event_id: uuid::Uuid },
    Recorded { event_id: uuid::Uuid, value: String },
}

impl DomainEvent for AuditEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Opened { .. } => "audit.opened",
            Self::Recorded { .. } => "audit.recorded",
        }
    }
}

impl ScheduledDomainEvent for AuditEvent {
    fn scheduled_event_id(&self) -> uuid::Uuid {
        match self {
            Self::Opened { event_id } | Self::Recorded { event_id, .. } => *event_id,
        }
    }
}

#[derive(Clone)]
struct AuditDecoder;

impl SchedulePayloadDecoder<AuditEvent> for AuditDecoder {
    type Error = std::convert::Infallible;

    fn decode(&self, fired: &ScheduleFired) -> Result<AuditEvent, Self::Error> {
        let value = String::from_utf8_lossy(fired.payload()).into_owned();
        Ok(AuditEvent::Recorded {
            event_id: fired.caller_event_id(),
            value,
        })
    }
}

fn id(n: u64) -> AggregateId {
    AggregateId::new(NonZeroU64::new(n).unwrap())
}

fn scheduler_stream() -> AggregateId {
    id(1)
}

fn sid(n: u128) -> ScheduleId {
    ScheduleId::from_uuid(uuid::Uuid::from_u128(n))
}

fn at(second: i64) -> jiff::Timestamp {
    jiff::Timestamp::from_second(second).unwrap()
}

fn ctx() -> CorrelationContext {
    CorrelationContext::new(uuid::Uuid::from_u128(90), uuid::Uuid::from_u128(91))
}

async fn opened_target(
    store: &MsgpackFileStore<AuditEvent>,
) -> (AggregateId, Vec<cherry_pit_core::EventEnvelope<AuditEvent>>) {
    store
        .create(
            vec![AuditEvent::Opened {
                event_id: uuid::Uuid::from_u128(1),
            }],
            CorrelationContext::none(),
        )
        .await
        .unwrap()
}

fn schedule(
    schedule_id: ScheduleId,
    target: AggregateId,
    caller_event_id: uuid::Uuid,
) -> ScheduleArmed {
    ScheduleArmed::new(
        schedule_id,
        at(10),
        target,
        caller_event_id,
        "audit.recorded",
        b"scheduled".to_vec(),
        ctx(),
    )
}

fn recording_bus() -> (InProcessEventBus<AuditEvent>, Arc<Mutex<Vec<AuditEvent>>>) {
    let bus = InProcessEventBus::<AuditEvent>::new();
    let published = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&published);
    bus.register(move |envelope| {
        captured.lock().unwrap().push(envelope.payload().clone());
    });
    (bus, published)
}

#[tokio::test]
async fn scheduled_effect_persists_then_publishes_auditable_event() {
    let scheduler_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let scheduler_store = MsgpackFileStore::<SchedulerEvent>::new(scheduler_dir.path());
    let target_store = MsgpackFileStore::<AuditEvent>::new(target_dir.path());
    let (target_id, _) = opened_target(&target_store).await;
    let (bus, published) = recording_bus();
    let driver = DurableScheduler::<_, _, _, _, AuditEvent>::new(
        &scheduler_store,
        &target_store,
        &bus,
        AuditDecoder,
    );
    let caller_event_id = uuid::Uuid::from_u128(200);

    driver
        .arm(schedule(sid(20), target_id, caller_event_id))
        .await
        .unwrap();
    let report = driver.recover_due(at(10)).await.unwrap();

    let target_history = target_store.load(target_id).await.unwrap();
    assert_eq!(report.fired, 1);
    assert_eq!(report.completed, 1);
    assert!(target_history.iter().any(|env| {
        matches!(env.payload(), AuditEvent::Recorded { event_id, .. } if *event_id == caller_event_id)
    }));
    assert_eq!(published.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn recovery_fires_due_schedule_at_most_once_after_reopen() {
    let scheduler_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let scheduler_store = MsgpackFileStore::<SchedulerEvent>::new(scheduler_dir.path());
    let target_store = MsgpackFileStore::<AuditEvent>::new(target_dir.path());
    let (target_id, _) = opened_target(&target_store).await;
    let (bus, published) = recording_bus();
    let armed = schedule(sid(30), target_id, uuid::Uuid::from_u128(300));
    {
        let driver = DurableScheduler::<_, _, _, _, AuditEvent>::new(
            &scheduler_store,
            &target_store,
            &bus,
            AuditDecoder,
        );
        driver.arm(armed.clone()).await.unwrap();
    }
    let stale = scheduler_store.load(scheduler_stream()).await.unwrap();
    drop(scheduler_store);
    drop(target_store);

    let scheduler_store = MsgpackFileStore::<SchedulerEvent>::new(scheduler_dir.path());
    let target_store = MsgpackFileStore::<AuditEvent>::new(target_dir.path());
    let driver = DurableScheduler::<_, _, _, _, AuditEvent>::new(
        &scheduler_store,
        &target_store,
        &bus,
        AuditDecoder,
    );
    driver.recover_due(at(10)).await.unwrap();
    let duplicate = scheduler_store
        .append(
            scheduler_stream(),
            stale.last().unwrap().sequence(),
            vec![SchedulerEvent::Fired(ScheduleFired::from_armed(&armed))],
            ctx(),
        )
        .await;
    let second = driver.recover_due(at(10)).await.unwrap();

    assert!(matches!(
        duplicate,
        Err(cherry_pit_core::StoreError::ConcurrencyConflict { .. })
    ));
    assert_eq!(second.fired, 0);
    assert_eq!(second.completed, 0);
    assert_eq!(published.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn no_loss_recovery_completes_schedule_fired_without_caller_event() {
    let scheduler_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let scheduler_store = MsgpackFileStore::<SchedulerEvent>::new(scheduler_dir.path());
    let target_store = MsgpackFileStore::<AuditEvent>::new(target_dir.path());
    let (target_id, _) = opened_target(&target_store).await;
    let armed = schedule(sid(40), target_id, uuid::Uuid::from_u128(400));
    let (scheduler_id, armed_envelopes) = scheduler_store
        .create(vec![SchedulerEvent::Armed(armed.clone())], ctx())
        .await
        .unwrap();
    scheduler_store
        .append(
            scheduler_id,
            armed_envelopes.last().unwrap().sequence(),
            vec![SchedulerEvent::Fired(ScheduleFired::from_armed(&armed))],
            ctx(),
        )
        .await
        .unwrap();
    drop(scheduler_store);
    drop(target_store);

    let scheduler_store = MsgpackFileStore::<SchedulerEvent>::new(scheduler_dir.path());
    let target_store = MsgpackFileStore::<AuditEvent>::new(target_dir.path());
    let (bus, published) = recording_bus();
    let driver = DurableScheduler::<_, _, _, _, AuditEvent>::new(
        &scheduler_store,
        &target_store,
        &bus,
        AuditDecoder,
    );
    let first = driver.recover_due(at(10)).await.unwrap();
    let second = driver.recover_due(at(10)).await.unwrap();
    let target_history = target_store.load(target_id).await.unwrap();
    let stored: Vec<_> = target_history
        .iter()
        .filter(|env| env.payload().scheduled_event_id() == uuid::Uuid::from_u128(400))
        .collect();

    assert_eq!(first.completed, 1);
    assert_eq!(second.completed, 0);
    assert_eq!(stored.len(), 1);
    assert_eq!(published.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancelled_and_unknown_schedules_do_not_fire() {
    let scheduler_dir = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    let scheduler_store = MsgpackFileStore::<SchedulerEvent>::new(scheduler_dir.path());
    let target_store = MsgpackFileStore::<AuditEvent>::new(target_dir.path());
    let (target_id, _) = opened_target(&target_store).await;
    let (bus, published) = recording_bus();
    let driver = DurableScheduler::<_, _, _, _, AuditEvent>::new(
        &scheduler_store,
        &target_store,
        &bus,
        AuditDecoder,
    );
    let schedule_id = sid(50);
    driver
        .arm(schedule(schedule_id, target_id, uuid::Uuid::from_u128(500)))
        .await
        .unwrap();
    driver
        .cancel(ScheduleCancelled::new(schedule_id), ctx())
        .await
        .unwrap();

    let cancelled = driver.fire_schedule(schedule_id, at(10)).await.unwrap();
    let unknown = driver.fire_schedule(sid(51), at(10)).await.unwrap();
    let recovery = driver.recover_due(at(10)).await.unwrap();

    assert!(cancelled.is_terminal());
    assert!(unknown.is_unknown());
    assert_eq!(recovery.fired, 0);
    assert_eq!(published.lock().unwrap().len(), 0);
}

#[test]
fn durable_scheduler_has_no_hidden_coordinator_surface() {
    let source = include_str!("../src/scheduler.rs");
    assert!(!source.contains("Box<dyn"));
    assert!(!source.contains("retry"));
}
