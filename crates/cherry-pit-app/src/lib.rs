#![forbid(unsafe_code)]
//! Root composition crate wiring Aggregate/Policy/Projection against
//! EventStore/EventBus/CommandGateway.
//!
//! Only crate sanctioned to depend on every other cherry-pit crate, per
//! [CHE-0051](../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md)
//! (acyclic DAG, [CHE-0029](../../docs/adr/cherry/CHE-0029-crate-decomposition.md)).
//! Public API is flat via `pub use` re-exports per
//! [CHE-0030](../../docs/adr/cherry/CHE-0030-flat-public-api.md):R1;
//! internal modules are implementation detail.
//!
//! # Wiring at a glance
//!
//! 1. Construct the four core ports (`CommandGateway`, `EventStore`,
//!    `EventBus`, projection-driver tuple) and a [`DeadLetterSink`].
//! 2. Pass them all to [`App::new`].
//! 3. Wire each policy with [`App::register_policy`], supplying a
//!    dispatch closure `Fn(P::Output, &G, CorrelationContext) ->
//!    Future<Output = Result<(), AgentError>>`. The closure is the
//!    exhaustive output matcher per
//!    [CHE-0017](../../docs/adr/cherry/CHE-0017-policy-output-static-type.md):R2.
//! 4. Drive the publish loop via [`App::run`] (or
//!    [`App::run_until_ctrl_c`]). Terminal failures are routed to the
//!    sink per
//!    [CHE-0051](../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md):R7;
//!    the loop continues.
//!
//! See the crate `README.md` for a runnable end-to-end example.

mod app;
mod dead_letter;
mod dispatch;
mod error;
mod event_bus;
mod projection_source;
mod scheduler;

pub use app::*;
pub use cherry_pit_projection::{ProjectionDriverExt, ProjectionDriverTuple};
pub use dead_letter::*;
pub use dispatch::correlation_for;
pub use error::*;
pub use event_bus::*;
pub use projection_source::*;
pub use scheduler::*;

/// Re-export of [`cherry_pit_core::CorrelationContext`] for ergonomic
/// access at the agent surface.
///
/// Per [CHE-0030](../../docs/adr/cherry/CHE-0030-flat-public-api.md):R1,
/// consumers wiring an [`App`] should not have to also depend on
/// `cherry-pit-core` just to name the context type threaded through
/// dispatch closures. Per
/// [CHE-0051](../../docs/adr/cherry/CHE-0051-cherry-pit-agent-design.md):R6
/// the dispatcher constructs it fresh per envelope and passes it as the
/// third closure argument so policy-emitted commands inherit the
/// correlation chain mechanically, without re-derivation by the caller.
pub use cherry_pit_core::CorrelationContext;
