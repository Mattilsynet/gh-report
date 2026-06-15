use std::num::NonZeroU64;

use cherry_pit_core::{
    AggregateId, CorrelationContext, EventBus, EventStore, ScheduleArmed, ScheduleCancelled,
    ScheduleFired, ScheduleId, ScheduleRecoveryReport, ScheduledDomainEvent, SchedulerEvent,
    SchedulerState, StoreError,
};

use crate::AgentError;

/// Decode the opaque caller payload carried by a fired schedule.
pub trait SchedulePayloadDecoder<E>: Send + Sync + 'static
where
    E: ScheduledDomainEvent,
{
    /// Error surfaced when payload decoding fails.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Decode a caller event from a fired scheduler fact.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] when the opaque scheduler payload cannot be
    /// decoded into the caller event type.
    fn decode(&self, fired: &ScheduleFired) -> Result<E, Self::Error>;
}

/// Outcome of asking the driver to fire one schedule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleFireOutcome {
    /// A pending due schedule recorded a fire and completed the caller event.
    Fired,
    /// The schedule was already terminal or its caller event already exists.
    Terminal,
    /// No schedule with the requested identity exists.
    Unknown,
    /// The schedule exists but its fire instant is still in the future.
    NotDue,
}

impl ScheduleFireOutcome {
    /// Return whether this outcome is terminal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Terminal)
    }

    /// Return whether this outcome is unknown.
    #[must_use]
    pub const fn is_unknown(self) -> bool {
        matches!(self, Self::Unknown)
    }
}

/// Async durable scheduler driver over typed event stores.
pub struct DurableScheduler<'a, SS, TS, B, D, E>
where
    SS: EventStore<Event = SchedulerEvent>,
    TS: EventStore<Event = E>,
    B: EventBus<Event = E>,
    D: SchedulePayloadDecoder<E>,
    E: ScheduledDomainEvent,
{
    scheduler_store: &'a SS,
    target_store: &'a TS,
    target_bus: &'a B,
    decoder: D,
    _event: std::marker::PhantomData<E>,
}

impl<'a, SS, TS, B, D, E> DurableScheduler<'a, SS, TS, B, D, E>
where
    SS: EventStore<Event = SchedulerEvent>,
    TS: EventStore<Event = E>,
    B: EventBus<Event = E>,
    D: SchedulePayloadDecoder<E>,
    E: ScheduledDomainEvent,
{
    /// Construct a scheduler driver over typed stores and bus.
    #[must_use]
    pub const fn new(
        scheduler_store: &'a SS,
        target_store: &'a TS,
        target_bus: &'a B,
        decoder: D,
    ) -> Self {
        Self {
            scheduler_store,
            target_store,
            target_bus,
            decoder,
            _event: std::marker::PhantomData,
        }
    }

    /// Return the singleton scheduler stream id.
    #[must_use]
    pub fn singleton_stream() -> AggregateId {
        AggregateId::new(NonZeroU64::MIN)
    }

    async fn append_scheduler(
        &self,
        event: SchedulerEvent,
        context: CorrelationContext,
    ) -> Result<(), AgentError> {
        let stream = Self::singleton_stream();
        let history = self.scheduler_store.load(stream).await?;
        if let Some(last) = history.last() {
            self.scheduler_store
                .append(stream, last.sequence(), vec![event], context)
                .await?;
        } else {
            let (created, _) = self.scheduler_store.create(vec![event], context).await?;
            if created != stream {
                return Err(AgentError::Store(StoreError::Infrastructure(
                    format!("scheduler stream must be {stream}, got {created}").into(),
                )));
            }
        }
        Ok(())
    }

    /// Persist a schedule arm event.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when the scheduler store cannot persist the
    /// arm event.
    pub async fn arm(&self, event: ScheduleArmed) -> Result<(), AgentError> {
        let context = event.correlation().clone();
        self.append_scheduler(SchedulerEvent::Armed(event), context)
            .await
    }

    /// Persist a schedule cancellation.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when the scheduler store cannot persist the
    /// cancellation event.
    pub async fn cancel(
        &self,
        event: ScheduleCancelled,
        context: CorrelationContext,
    ) -> Result<(), AgentError> {
        self.append_scheduler(SchedulerEvent::Cancelled(event), context)
            .await
    }

    /// Fire one schedule if it is known, pending, and due.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when replay, append, decode, or publish
    /// fails.
    pub async fn fire_schedule(
        &self,
        schedule_id: ScheduleId,
        now: jiff::Timestamp,
    ) -> Result<ScheduleFireOutcome, AgentError> {
        let stream = Self::singleton_stream();
        let history = self.scheduler_store.load(stream).await?;
        let state = SchedulerState::from_history(&history);
        if state.fired_schedule(schedule_id).is_some() || state.is_cancelled(schedule_id) {
            return Ok(ScheduleFireOutcome::Terminal);
        }
        let Some(armed) = state.pending_schedule(schedule_id) else {
            return Ok(ScheduleFireOutcome::Unknown);
        };
        if armed.fire_at() > now {
            return Ok(ScheduleFireOutcome::NotDue);
        }
        let Some(last) = history.last() else {
            return Ok(ScheduleFireOutcome::Unknown);
        };
        let fired = ScheduleFired::from_armed(armed);
        self.scheduler_store
            .append(
                stream,
                last.sequence(),
                vec![SchedulerEvent::Fired(fired.clone())],
                fired.correlation().clone(),
            )
            .await?;
        self.complete_fired(&fired).await?;
        Ok(ScheduleFireOutcome::Fired)
    }

    async fn complete_fired(&self, fired: &ScheduleFired) -> Result<bool, AgentError> {
        let history = self.target_store.load(fired.target_aggregate()).await?;
        if history
            .iter()
            .any(|env| env.payload().scheduled_event_id() == fired.caller_event_id())
        {
            return Ok(false);
        }
        let event = self
            .decoder
            .decode(fired)
            .map_err(|err| AgentError::Policy(Box::new(err)))?;
        let Some(last) = history.last() else {
            return Err(AgentError::Store(StoreError::Infrastructure(
                format!(
                    "target aggregate {} is empty for schedule {}",
                    fired.target_aggregate(),
                    fired.schedule_id().as_uuid()
                )
                .into(),
            )));
        };
        let envelopes = self
            .target_store
            .append(
                fired.target_aggregate(),
                last.sequence(),
                vec![event],
                fired.correlation().clone(),
            )
            .await?;
        self.target_bus.publish(&envelopes).await?;
        Ok(true)
    }

    /// Complete all fired and due-pending schedules.
    ///
    /// # Errors
    ///
    /// Returns [`AgentError`] when replay, append, decode, or publish
    /// fails.
    pub async fn recover_due(
        &self,
        now: jiff::Timestamp,
    ) -> Result<ScheduleRecoveryReport, AgentError> {
        let history = self.scheduler_store.load(Self::singleton_stream()).await?;
        let state = SchedulerState::from_history(&history);
        let mut report = ScheduleRecoveryReport::default();
        for fired in state.fired() {
            if self.complete_fired(fired).await? {
                report.completed += 1;
            }
        }
        let due: Vec<ScheduleId> = state
            .due_pending(now)
            .map(cherry_pit_core::ScheduleArmed::schedule_id)
            .collect();
        for schedule_id in due {
            if self.fire_schedule(schedule_id, now).await? == ScheduleFireOutcome::Fired {
                report.fired += 1;
                report.completed += 1;
            }
        }
        Ok(report)
    }
}

impl<SS, TS, B, D, E> cherry_pit_core::EventScheduler for DurableScheduler<'_, SS, TS, B, D, E>
where
    SS: EventStore<Event = SchedulerEvent>,
    TS: EventStore<Event = E>,
    B: EventBus<Event = E>,
    D: SchedulePayloadDecoder<E>,
    E: ScheduledDomainEvent,
{
    type Error = AgentError;

    async fn arm(&self, event: ScheduleArmed) -> Result<(), Self::Error> {
        Self::arm(self, event).await
    }

    async fn cancel(
        &self,
        event: ScheduleCancelled,
        context: CorrelationContext,
    ) -> Result<(), Self::Error> {
        Self::cancel(self, event, context).await
    }

    async fn recover_due(
        &self,
        now: jiff::Timestamp,
    ) -> Result<ScheduleRecoveryReport, Self::Error> {
        Self::recover_due(self, now).await
    }
}
