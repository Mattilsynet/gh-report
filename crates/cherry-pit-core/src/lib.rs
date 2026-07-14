//! # cherry-pit-core
//!
//! The narrow, typed ports agents program against: domain logic lives
//! behind these traits, infrastructure on the other side. Dependencies
//! restricted to `{serde, uuid, jiff}` (CHE-0029:R4–R5); private
//! modules with selective `pub use` re-exports (CHE-0030:R1–R2); zero
//! async-runtime dependency (CHE-0018:R3).
//!
//! ## Single-aggregate design
//!
//! Every infrastructure port (`EventStore`, `EventBus`, `CommandBus`,
//! `CommandGateway`) binds to one aggregate/event type via associated
//! types, giving compiler-enforced type safety end-to-end from
//! dispatch through persistence and publication. Multiple aggregates
//! compose as separate bounded contexts; cross-context communication
//! uses event subscriptions (e.g. NATS subjects), not shared stores.
//!
//! ## Domain traits
//!
//! [`DomainEvent`], [`Command`], [`Aggregate`], [`HandleCommand`],
//! [`Policy`], [`Projection`], [`ReadPort`].
//!
//! ## Port traits (async, RPITIT)
//!
//! [`CommandGateway`], [`CommandBus`], [`EventStore`], [`EventBus`].
//!
//! ## Types
//!
//! [`AggregateId`], [`EventEnvelope`], [`ProjectionCheckpoint`],
//! [`CorrelationContext`], [`IdempotencyKey`], [`DispatchError`],
//! [`DispatchResult`], [`CreateResult`], [`StoreError`],
//! [`EnvelopeError`], [`BusError`], [`ErrorCategory`].

#![forbid(unsafe_code)]

mod aggregate;
mod aggregate_id;
mod bus;
mod checkpoint;
mod command;
mod correlation;
mod error;
mod event;
mod gateway;
mod idempotency;
mod policy;
mod projection;
mod scheduler;
mod store;
mod work;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use aggregate::{Aggregate, HandleCommand};
pub use aggregate_id::AggregateId;
pub use bus::{CommandBus, EventBus};
pub use checkpoint::ProjectionCheckpoint;
pub use command::Command;
pub use correlation::CorrelationContext;
pub use error::{
    BusError, CreateResult, DispatchError, DispatchResult, EnvelopeError, ErrorCategory,
    StoreCreateResult, StoreError,
};
pub use event::{DomainEvent, EventEnvelope};
pub use gateway::CommandGateway;
pub use idempotency::IdempotencyKey;
pub use policy::Policy;
pub use projection::{Projection, ReadPort};
pub use scheduler::{
    EventScheduler, ScheduleArmed, ScheduleCancelled, ScheduleFired, ScheduleId,
    ScheduleRecoveryReport, ScheduledDomainEvent, SchedulerEvent, SchedulerState,
};
pub use store::{
    EventHistoryEventStore, EventStore, HashChainedEventStore, ListableEventStore,
    PurgeableEventStore, SingleWriterEventStore,
};
pub use work::{DomainKey, JobOutcome, JobSource};
