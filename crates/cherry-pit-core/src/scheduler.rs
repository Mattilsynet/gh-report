use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::{AggregateId, CorrelationContext, DomainEvent, EventEnvelope};

/// Stable identity for one durable schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ScheduleId(uuid::Uuid);

impl ScheduleId {
    /// Construct a [`ScheduleId`] from a caller-supplied UUID.
    #[must_use]
    pub const fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id)
    }

    /// Return the wrapped UUID.
    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

/// Domain events emitted by the scheduler aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulerEvent {
    /// A caller event is scheduled for a future instant.
    Armed(ScheduleArmed),
    /// The scheduler decided the caller event must be completed.
    Fired(ScheduleFired),
    /// The caller cancelled a schedule before it fired.
    Cancelled(ScheduleCancelled),
}

impl SchedulerEvent {
    /// Schema identity for the separate scheduler event type.
    pub const SCHEMA_SOURCE: &'static str = "cherry-pit/SchedulerEvent";

    /// Stable schema hash for the separate scheduler event type.
    pub const SCHEMA_HASH: u128 = 226_768_822_733_184_891_089_447_062_462_252_163_919_u128;
}

impl DomainEvent for SchedulerEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Armed(_) => "scheduler.schedule_armed",
            Self::Fired(_) => "scheduler.schedule_fired",
            Self::Cancelled(_) => "scheduler.schedule_cancelled",
        }
    }
}

/// Durable fact that a caller event must fire at `fire_at`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleArmed {
    schedule_id: ScheduleId,
    fire_at: jiff::Timestamp,
    target_aggregate: AggregateId,
    caller_event_id: uuid::Uuid,
    caller_event_type: String,
    payload: Vec<u8>,
    correlation: CorrelationContext,
}

impl ScheduleArmed {
    /// Construct a [`ScheduleArmed`] event.
    #[must_use]
    pub fn new(
        schedule_id: ScheduleId,
        fire_at: jiff::Timestamp,
        target_aggregate: AggregateId,
        caller_event_id: uuid::Uuid,
        caller_event_type: impl Into<String>,
        payload: Vec<u8>,
        correlation: CorrelationContext,
    ) -> Self {
        Self {
            schedule_id,
            fire_at,
            target_aggregate,
            caller_event_id,
            caller_event_type: caller_event_type.into(),
            payload,
            correlation,
        }
    }

    /// Return the schedule identity.
    #[must_use]
    pub const fn schedule_id(&self) -> ScheduleId {
        self.schedule_id
    }

    /// Return the requested fire instant.
    #[must_use]
    pub const fn fire_at(&self) -> jiff::Timestamp {
        self.fire_at
    }

    /// Return the target aggregate for the caller event.
    #[must_use]
    pub const fn target_aggregate(&self) -> AggregateId {
        self.target_aggregate
    }

    /// Return the caller event identity carried for completion.
    #[must_use]
    pub const fn caller_event_id(&self) -> uuid::Uuid {
        self.caller_event_id
    }

    /// Return the caller event type string.
    #[must_use]
    pub fn caller_event_type(&self) -> &str {
        &self.caller_event_type
    }

    /// Return the opaque caller payload transport.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Return the correlation context carried into the caller event.
    #[must_use]
    pub const fn correlation(&self) -> &CorrelationContext {
        &self.correlation
    }
}

/// Durable fact that the scheduler decided to fire a schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleFired {
    schedule_id: ScheduleId,
    target_aggregate: AggregateId,
    caller_event_id: uuid::Uuid,
    caller_event_type: String,
    payload: Vec<u8>,
    correlation: CorrelationContext,
}

impl ScheduleFired {
    /// Copy the caller-event transport from an armed schedule.
    #[must_use]
    pub fn from_armed(armed: &ScheduleArmed) -> Self {
        Self {
            schedule_id: armed.schedule_id,
            target_aggregate: armed.target_aggregate,
            caller_event_id: armed.caller_event_id,
            caller_event_type: armed.caller_event_type.clone(),
            payload: armed.payload.clone(),
            correlation: armed.correlation.clone(),
        }
    }

    /// Return the schedule identity.
    #[must_use]
    pub const fn schedule_id(&self) -> ScheduleId {
        self.schedule_id
    }

    /// Return the target aggregate for the caller event.
    #[must_use]
    pub const fn target_aggregate(&self) -> AggregateId {
        self.target_aggregate
    }

    /// Return the caller event identity carried for completion.
    #[must_use]
    pub const fn caller_event_id(&self) -> uuid::Uuid {
        self.caller_event_id
    }

    /// Return the caller event type string.
    #[must_use]
    pub fn caller_event_type(&self) -> &str {
        &self.caller_event_type
    }

    /// Return the opaque caller payload transport.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Return the correlation context carried into the caller event.
    #[must_use]
    pub const fn correlation(&self) -> &CorrelationContext {
        &self.correlation
    }
}

/// Durable fact that a schedule was cancelled before firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCancelled {
    schedule_id: ScheduleId,
}

impl ScheduleCancelled {
    /// Construct a cancellation event for `schedule_id`.
    #[must_use]
    pub const fn new(schedule_id: ScheduleId) -> Self {
        Self { schedule_id }
    }

    /// Return the cancelled schedule identity.
    #[must_use]
    pub const fn schedule_id(&self) -> ScheduleId {
        self.schedule_id
    }
}

/// Replay-derived scheduler aggregate state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerState {
    pending: BTreeMap<ScheduleId, ScheduleArmed>,
    fired: BTreeMap<ScheduleId, ScheduleFired>,
    cancelled: BTreeSet<ScheduleId>,
}

impl SchedulerState {
    /// Fold a scheduler stream into replay-derived state.
    #[must_use]
    pub fn from_history(history: &[EventEnvelope<SchedulerEvent>]) -> Self {
        let mut state = Self::default();
        for envelope in history {
            match envelope.payload() {
                SchedulerEvent::Armed(event) => {
                    state.cancelled.remove(&event.schedule_id());
                    state.pending.insert(event.schedule_id(), event.clone());
                }
                SchedulerEvent::Fired(event) => {
                    state.pending.remove(&event.schedule_id());
                    state.fired.insert(event.schedule_id(), event.clone());
                }
                SchedulerEvent::Cancelled(event) => {
                    state.pending.remove(&event.schedule_id());
                    state.cancelled.insert(event.schedule_id());
                }
            }
        }
        state
    }

    /// Iterate currently pending schedules.
    pub fn pending(&self) -> impl Iterator<Item = &ScheduleArmed> {
        self.pending.values()
    }

    /// Iterate fired schedules retained for recovery completion.
    pub fn fired(&self) -> impl Iterator<Item = &ScheduleFired> {
        self.fired.values()
    }

    /// Return the pending schedule if it exists.
    #[must_use]
    pub fn pending_schedule(&self, schedule_id: ScheduleId) -> Option<&ScheduleArmed> {
        self.pending.get(&schedule_id)
    }

    /// Return the fired schedule if it exists.
    #[must_use]
    pub fn fired_schedule(&self, schedule_id: ScheduleId) -> Option<&ScheduleFired> {
        self.fired.get(&schedule_id)
    }

    /// Return whether `schedule_id` has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self, schedule_id: ScheduleId) -> bool {
        self.cancelled.contains(&schedule_id)
    }

    /// Iterate pending schedules due at or before `now`.
    pub fn due_pending(&self, now: jiff::Timestamp) -> impl Iterator<Item = &ScheduleArmed> {
        self.pending
            .values()
            .filter(move |event| event.fire_at() <= now)
    }
}

/// Summary returned by a scheduler recovery pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScheduleRecoveryReport {
    /// Number of pending schedules that recorded a new fire decision.
    pub fired: u64,
    /// Number of caller events completed from scheduler facts.
    pub completed: u64,
}

/// Async scheduler port for durable schedule drivers.
pub trait EventScheduler: Send + Sync {
    /// Error type returned by the concrete driver.
    type Error;

    /// Persist a new schedule.
    ///
    /// # Errors
    ///
    /// Returns the concrete driver error when the scheduler store cannot
    /// persist the arm event.
    fn arm(&self, event: ScheduleArmed) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Persist a cancellation.
    ///
    /// # Errors
    ///
    /// Returns the concrete driver error when the scheduler store cannot
    /// persist the cancellation event.
    fn cancel(
        &self,
        event: ScheduleCancelled,
        context: CorrelationContext,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Complete every due schedule known to the driver.
    ///
    /// # Errors
    ///
    /// Returns the concrete driver error when replay, append, decode, or
    /// publish fails.
    fn recover_due(
        &self,
        now: jiff::Timestamp,
    ) -> impl Future<Output = Result<ScheduleRecoveryReport, Self::Error>> + Send;
}

/// Domain events that expose the identity carried by a scheduler fact.
pub trait ScheduledDomainEvent: DomainEvent {
    /// Return the caller event identity carried by the scheduled event.
    #[must_use]
    fn scheduled_event_id(&self) -> uuid::Uuid;
}
