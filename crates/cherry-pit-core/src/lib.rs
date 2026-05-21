//! # cherry-pit-core
//!
//! Foundational traits for cherry-pit: the narrow, typed ports that
//! agents program against. All domain logic lives behind these traits.
//! All infrastructure lives on the other side.
//! (CHE-0029 R4–R5: dependencies restricted to {serde, uuid, jiff};
//! CHE-0030 R1–R2: private modules with selective pub use re-exports;
//! CHE-0018 R3: zero async runtime dependency in core.)
//!
//! ## Single-aggregate design
//!
//! Every infrastructure port (`EventStore`, `EventBus`, `CommandBus`,
//! `CommandGateway`) is bound to a single aggregate/event type via
//! associated types. The compiler enforces end-to-end type safety
//! from command dispatch through event persistence and publication.
//!
//! Multiple aggregates are supported at the system level by deploying
//! separate bounded contexts — each with its own typed infrastructure
//! stack. Cross-context communication happens through event
//! subscriptions (e.g. NATS subjects), not shared stores.
//!
//! ## Domain traits
//!
//! - [`DomainEvent`] — immutable facts (something that happened)
//! - [`Command`] — intent to change state
//! - [`Aggregate`] — consistency boundary, reconstructed from events
//! - [`HandleCommand`] — compile-time verified command→aggregate pairs
//! - [`Policy`] — reacts to events by producing commands
//! - [`Projection`] — folds events into read-optimized views
//!
//! ## Port traits (async, RPITIT)
//!
//! - [`CommandGateway`] — primary entry point for dispatching commands
//! - [`CommandBus`] — load → handle → persist → publish lifecycle
//! - [`EventStore`] — persistence of aggregate event streams
//! - [`EventBus`] — fan-out of persisted events
//!
//! ## Types
//!
//! - [`AggregateId`] — stream partition key (auto-assigned `u64`)
//! - [`EventEnvelope`] — infrastructure wrapper around domain events
//! - [`ProjectionCheckpoint`] — durable (aggregate, projection, sequence) cursor
//! - [`CorrelationContext`] — explicit correlation/causation propagation
//! - [`IdempotencyKey`] — consumer-supplied stability key (never synthesised)
//! - [`DispatchError`] — typed command dispatch errors
//! - [`DispatchResult`] — return type alias for bus/gateway dispatch
//! - [`CreateResult`] — return type alias for aggregate creation
//! - [`StoreError`] — event store operation errors
//! - [`EnvelopeError`] — envelope construction/validation errors
//! - [`BusError`] — event bus publication errors
//! - [`ErrorCategory`] — stable retryable/terminal error guidance

// CHE-0007: No unsafe code in any cherry-pit crate.
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
mod store;

// CHE-0058 carve-out: feature-gated `pub mod` for test fixtures.
// Visibility is opt-in via `--features testing` for downstream consumers
// (SM-4 conformance harness + adapter-crate integration tests); always
// compiled in `#[cfg(test)]` so core's own tests can exercise the
// fixtures without enabling the feature.
#[cfg(any(test, feature = "testing"))]
pub mod testing;

// Re-export `pardosa_encoding` so downstream crates implementing the
// `DomainEvent` supertrait bound (CHE-0064:R2) can reach the `Encode`
// trait without naming `pardosa-encoding` as a direct manifest entry.
// Upstream pardosa-removal mission (gh-report) requires gh-report not
// to name pardosa-* crates directly; this re-export is the load-bearing
// affordance that makes that possible without dragging the encoding
// crate's public surface inside cherry-pit-core itself.
pub use pardosa_encoding;

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
pub use projection::Projection;
pub use store::{
    EventStore, HashChainedEventStore, ListableEventStore, PurgeableEventStore,
    SingleWriterEventStore,
};
