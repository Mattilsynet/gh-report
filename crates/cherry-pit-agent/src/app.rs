//! Root composition struct wiring `(CommandGateway, EventStore,
//! EventBus, ProjectionDriverTuple, DeadLetterSink, [Policy…])` per
//! CHE-0051:R3 + R4 + R5 + R7 + R8.
//!
//! v0.1 surface:
//!
//! - [`App::new`] — explicit constructor (no `Default`, per
//!   CHE-0039:R2 + CHE-0051:R3) taking the four core ports plus the
//!   projection-driver tuple plus the dead-letter sink.
//! - [`App::register_policy`] — explicit per-policy registration with
//!   a user-supplied dispatch closure per CHE-0051:R4 +
//!   CHE-0017:R2 (caller writes the exhaustive output matcher).
//! - [`App::run`] — terminal wiring of the publish-loop. Stubbed in
//!   S5 (returns immediately on first shutdown signal); S6+ adds the
//!   bus subscription that drives [`crate::dispatch::dispatch_one`]
//!   per envelope and the [`cherry_pit_projection::ProjectionDriverExt`]
//!   incremental update.
//!
//! ## Type-parameter inventory
//!
//! `App<G, S, B, P, D>` is single-aggregate per CHE-0051:R9 + C12.
//! Five type parameters cover the sanctioned composition surface
//! without `Box<dyn>` over any of the CHE-0005:R1 infra ports. The
//! single aggregate `A = G::Aggregate` is reachable as
//! `<G as CommandGateway>::Aggregate` and pins the event type
//! `E = <A as Aggregate>::Event` shared by `S`, `B`, and every
//! registered policy.
//!
//! ## C1 boundary note (carried from S3 + S5 dispatch.rs)
//!
//! The policy registry stores
//! `Vec<Box<dyn ErasedPolicyDispatcher<E, G>>>`. The boxed trait
//! object is a per-policy `(Policy, dispatch_closure)` adapter, NOT
//! `Box<dyn Policy>`. CHE-0005:R1 forbids object-erasure of
//! aggregate-bound infra ports; the erasure here is at the agent's
//! internal dispatcher boundary over user closures, which is the
//! same closure-vs-port reasoning linus approved for
//! `InProcessEventBus` (`event_bus.rs:22-37`) and re-stated in
//! `dispatch.rs:21-37`.

use std::future::Future;
use std::sync::Arc;

use cherry_pit_core::{
    Aggregate, CommandGateway, CorrelationContext, EventBus, EventStore, Policy,
};

use crate::dead_letter::DeadLetterSink;
use crate::dispatch::{self, ErasedPolicyDispatcher, make_adapter};
use crate::error::AgentError;
use crate::event_bus::InProcessEventBus;
use cherry_pit_projection::ProjectionDriverTuple;

/// Convenience alias for the event type bound by an [`Aggregate`].
type EventOf<G> = <<G as CommandGateway>::Aggregate as Aggregate>::Event;

/// Heterogeneous policy registry shape per CHE-0051:R4. See `dispatch.rs`
/// C1 boundary note for why the erasure is sound.
type PolicyRegistry<G> = Arc<Vec<Box<dyn ErasedPolicyDispatcher<EventOf<G>, G>>>>;

/// Root composition struct per CHE-0051:R3.
///
/// Owns the four core ports + projection-driver tuple + dead-letter
/// sink + the heterogeneous policy registry. Constructed via
/// [`App::new`]; policies wired via [`App::register_policy`]; driven
/// via [`App::run`].
///
/// Multi-aggregate composition (C12) is out of scope for v0.1 —
/// CHE-0051:R9 prescribes parameter expansion at the consumer site,
/// and this struct is the single-aggregate base.
///
/// # Example — minimal composition
///
/// `no_run` because [`App::run`] requires an active multi-thread
/// tokio runtime. See the crate `README.md` for the runnable form
/// inside `#[tokio::main(flavor = "multi_thread")]`.
///
/// ```no_run
/// use cherry_pit_agent::{App, InProcessEventBus, TracingDeadLetterSink};
/// # async fn wire<G, S>(gateway: G, store: S) -> Result<(), Box<dyn std::error::Error>>
/// # where
/// #     G: cherry_pit_core::CommandGateway + Send + Sync + 'static,
/// #     S: cherry_pit_core::EventStore<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event>,
/// #     <G::Aggregate as cherry_pit_core::Aggregate>::Event: Send + Sync + 'static,
/// # {
/// let bus = InProcessEventBus::new();
/// let sink = TracingDeadLetterSink::new();
/// let app = App::new(gateway, store, bus, (), sink);
/// app.run_until_ctrl_c().await?;
/// # Ok(()) }
/// ```
pub struct App<G, S, B, P, D>
where
    G: CommandGateway,
    S: EventStore<Event = EventOf<G>>,
    B: EventBus<Event = EventOf<G>>,
    P: ProjectionDriverTuple,
    D: DeadLetterSink,
{
    /// Command gateway — the dispatch ingress for both user-initiated
    /// commands and policy-emitted commands per CHE-0051:R4.
    gateway: G,

    /// Event store — persistence ingress per CHE-0051:R3.
    #[expect(
        dead_code,
        reason = "composition slot per CHE-0051:R3; CommandGateway wraps the store per CHE-0005:R3 so App has no direct read path; lifetime owned for v0.1 + future saga/outbox hooks. #[expect] fails closed when S6+ wires a direct read path."
    )]
    store: S,

    /// Event bus — publication egress per CHE-0051:R3 + CHE-0024.
    bus: B,

    /// Projection drivers — heterogeneous tuple per CHE-0051:R5 + R9.
    #[expect(
        dead_code,
        reason = "composition slot per CHE-0051:R5 + R9; ProjectionDriverTuple per-envelope hook is consumer-owned (projection state lives outside App); lifetime owned for v0.1 + future runtime hooks (S7+). #[expect] fails closed when S7+ wires the driver loop."
    )]
    projections: P,

    /// Dead-letter sink — per-Terminal route per CHE-0051:R7 +
    /// CHE-0024:R5 + CHE-0040:R3.
    dead_letter: D,

    /// Heterogeneous policy registry per CHE-0051:R4. Each element is
    /// a `(Policy, dispatch_closure)` adapter erased to the common
    /// `ErasedPolicyDispatcher<E, G>` shape so the dispatch loop can
    /// iterate without macro arity expansion. See module-level C1
    /// boundary note.
    ///
    /// `Arc` keeps the registry shareable for the future `App::run`
    /// publish loop (which will hand a clone to the bus subscription
    /// task) without forcing the registry through `&'static`.
    policies: PolicyRegistry<G>,
}

impl<G, S, B, P, D> App<G, S, B, P, D>
where
    G: CommandGateway,
    S: EventStore<Event = EventOf<G>>,
    B: EventBus<Event = EventOf<G>>,
    P: ProjectionDriverTuple,
    D: DeadLetterSink,
{
    /// Construct a fully-wired `App` per CHE-0051:R3.
    ///
    /// All five composition slots are required; there is no `Default`
    /// (CHE-0039:R2 + CHE-0051:R3 mandate explicit construction).
    /// The policy registry starts empty — wire policies via
    /// [`Self::register_policy`].
    ///
    /// **Known ADR-text drift (CHE-0051:R3 vs code).** The ADR text of
    /// CHE-0051:R3 currently lists four slots
    /// (`App::new(gateway, store, bus, projections)`); this signature
    /// is five-slot because the `dead_letter: D` route is required by
    /// CHE-0051:R7 + CHE-0024:R5 + CHE-0040:R3. The code, the
    /// type-parameter list `App<G, S, B, P, D>`, and this rustdoc are
    /// consistent at five slots; the ADR text is the lone outlier.
    /// The proposed CHE-0051:R3 amendment delta is tracked at
    /// bd `adr-fmt-ur04` (S8+, post-v0.1 per FOCUS §8 which forbids
    /// agent-side ADR edits during closure).
    pub fn new(gateway: G, store: S, bus: B, projections: P, dead_letter: D) -> Self {
        Self {
            gateway,
            store,
            bus,
            projections,
            dead_letter,
            policies: Arc::new(Vec::new()),
        }
    }

    /// Register a policy + its dispatch closure per CHE-0051:R4.
    ///
    /// The closure shape `Fn(P::Output, &G, CorrelationContext) ->
    /// Future<Output = Result<(), AgentError>>` mirrors CHE-0017:R2
    /// (caller writes the exhaustive output matcher) — no
    /// agent-internal `Box<dyn Command>` or runtime routing.
    ///
    /// Per CHE-0051:R4 + R6, the dispatcher constructs
    /// `CorrelationContext::new(envelope.correlation_id,
    /// envelope.event_id)` per envelope and passes it as `ctx` (the
    /// third closure argument). The closure threads `ctx` into any
    /// `gateway.send(...)` call so policy-emitted commands inherit
    /// the correlation chain mechanically (no re-derivation by the
    /// caller, no risk of a fresh chain breaking the invariant).
    ///
    /// `policy_identity` and `output_type` are stable strings used
    /// for dead-letter records (CHE-0024:R5 + CHE-0040:R3 +
    /// CHE-0051:R7); pass the policy's type name and the output
    /// enum's type name (or other stable identifiers).
    ///
    /// # Example
    ///
    /// `no_run` because the surrounding `App::run` needs a multi-thread
    /// tokio runtime; the closure shape itself is what this example
    /// demonstrates.
    ///
    /// ```no_run
    /// use cherry_pit_agent::{App, AgentError, CorrelationContext};
    /// # async fn wire<G, S, B, P, D, Pol>(mut app: App<G, S, B, P, D>, policy: Pol)
    /// # where
    /// #     G: cherry_pit_core::CommandGateway,
    /// #     S: cherry_pit_core::EventStore<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event>,
    /// #     B: cherry_pit_core::EventBus<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event>,
    /// #     P: cherry_pit_agent::ProjectionDriverTuple,
    /// #     D: cherry_pit_agent::DeadLetterSink,
    /// #     Pol: cherry_pit_core::Policy<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event, Output = ()>,
    /// # {
    /// app.register_policy(
    ///     policy,
    ///     // Caller-written exhaustive output matcher per CHE-0017:R2.
    ///     // `ctx` is threaded into any gateway.send(...) call so
    ///     // policy-emitted commands inherit the correlation chain
    ///     // (CHE-0051:R6).
    ///     |_output, _gateway, _ctx: CorrelationContext| async move {
    ///         Ok::<(), AgentError>(())
    ///     },
    ///     "MyPolicy",
    ///     "MyPolicyOutput",
    /// );
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the internal `Arc<Vec<…>>` registry has outstanding
    /// clones. Currently this panic is **unreachable**: `App::run`
    /// consumes `self`, so `register_policy` cannot be called after
    /// `run`, and no other code path clones the registry `Arc` before
    /// `run` consumes the `App`. The defensive `expect` exists for
    /// the S6+ wiring step that hands a registry clone to the publish
    /// loop — once that lands, calling `register_policy` after `run`
    /// (or while a concurrent registry clone is alive) becomes a
    /// programming error and the panic becomes reachable.
    pub fn register_policy<Pol, F, Fut>(
        &mut self,
        policy: Pol,
        dispatch: F,
        policy_identity: &'static str,
        output_type: &'static str,
    ) where
        Pol: Policy<Event = EventOf<G>>,
        F: Fn(Pol::Output, &G, CorrelationContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), AgentError>> + Send + 'static,
    {
        let adapter =
            make_adapter::<Pol, F, Fut, G>(policy, dispatch, policy_identity, output_type);
        // Pre-`run` registration: registry is still uniquely owned.
        let registry = Arc::get_mut(&mut self.policies).expect(
            "App::register_policy called after App::run handed the registry to the \
             publish loop; register all policies before calling run",
        );
        registry.push(adapter);
    }

    /// Number of registered policies. Primarily for testing /
    /// diagnostic logging.
    #[must_use]
    pub fn policy_count(&self) -> usize {
        self.policies.len()
    }
}

impl<G, S, P, D> App<G, S, InProcessEventBus<EventOf<G>>, P, D>
where
    G: CommandGateway + Send + Sync + 'static,
    S: EventStore<Event = EventOf<G>>,
    P: ProjectionDriverTuple,
    D: DeadLetterSink,
    EventOf<G>: Send + Sync + 'static,
{
    /// Drive the publish loop until `shutdown` resolves.
    ///
    /// Subscribes a synchronous fan-out handler against
    /// [`InProcessEventBus::register`] (CHE-0024:R2 + CHE-0051:R2). Each
    /// published envelope is routed through the agent's internal
    /// per-policy dispatcher across the policy registry; per
    /// CHE-0051:R7 + CHE-0024:R5 + CHE-0046:R2 `Terminal` failures are
    /// routed to the dead-letter sink internally and do NOT abort the
    /// loop. `Retryable` failures surface as a `tracing::error!` but
    /// are likewise non-aborting (the sync callback has no `Result`
    /// channel; one bad envelope must not stop the bus).
    ///
    /// One run impl per concrete bus type per CHE-0024:R2 —
    /// `InProcessEventBus` uses synchronous fan-out via `register`;
    /// remote bus runtimes ship their own `run` impl in their own
    /// `impl` block.
    ///
    /// # Panics
    ///
    /// Panics if called outside an active multi-thread tokio runtime.
    /// The synchronous `register` callback bridges into async
    /// dispatch via [`tokio::runtime::Handle::try_current`] +
    /// `block_on`; both require a running multi-thread runtime
    /// (single-thread flavour deadlocks on `block_on` of a future
    /// that needs the same worker).
    ///
    /// # Hazards
    ///
    /// **Re-entrant `block_on` + caller-held mutex (CHE-0051:R8
    /// advisory).** The `bus.register(...)` callback installed below
    /// is invoked *synchronously* by the bus inside its `publish()`
    /// call (see [`InProcessEventBus::publish`] and CHE-0024:§7).
    /// That callback then re-enters the async runtime via
    /// `Handle::block_on(...)` to drive policy dispatch. If the
    /// publishing code path holds a `std::sync::Mutex` (or any
    /// blocking lock) across the `publish().await` point, and any
    /// dispatch closure or downstream `gateway.send(...)` future
    /// awaits on that same lock, the chain deadlocks: the publisher
    /// holds the lock, the dispatcher's `block_on`-ed future cannot
    /// acquire it, and the multi-thread runtime cannot make progress
    /// on the publisher's side because the worker is parked inside
    /// `block_on`.
    ///
    /// Mitigation: never hold a blocking mutex across a
    /// `publish().await` from caller code, and never await on a
    /// publisher-held mutex from inside a registered policy dispatch
    /// closure. `tokio::sync::Mutex` is not a workaround — the
    /// hazard is the *re-entrant* shape, not the lock kind.
    ///
    /// # Errors
    ///
    /// Currently never returns `Err` — per-envelope failures are
    /// dead-lettered or logged, not propagated. The signature
    /// preserves the option for future bus-level errors per
    /// CHE-0051:R8.
    ///
    /// Per **COM-0025:R1**, retry orchestration for `Retryable`
    /// per-envelope failures is the responsibility of the consumer
    /// holding the `CommandGateway` / command bus, not `App::run`.
    /// `App::run` surfaces such failures via `tracing::error!` so
    /// caller-side retry loops can observe them; it does not itself
    /// re-drive the failed dispatch (that would conflate bus-loop
    /// liveness with policy-output retry semantics).
    pub async fn run<Sd>(self, shutdown: Sd) -> Result<(), AgentError>
    where
        Sd: Future<Output = ()> + Send,
    {
        let policies = Arc::clone(&self.policies);
        let gateway = Arc::new(self.gateway);
        let dead_letter = Arc::new(self.dead_letter);

        self.bus.register(move |envelope| {
            let envelope = envelope.clone();
            let policies = Arc::clone(&policies);
            let gateway = Arc::clone(&gateway);
            let dead_letter = Arc::clone(&dead_letter);
            let handle = tokio::runtime::Handle::try_current().expect(
                "App::run requires a multi-thread tokio runtime; \
                 Handle::try_current() returned no active runtime. \
                 Wrap your main in #[tokio::main(flavor = \"multi_thread\")].",
            );
            handle.block_on(async move {
                if let Err(err) =
                    dispatch::dispatch_one(&policies, &envelope, &*gateway, &*dead_letter).await
                {
                    // Per CHE-0046:R1–R2: Terminal failures are
                    // dead-lettered inside dispatch_one and return
                    // Ok; this branch only fires on Retryable
                    // failures, which the sync callback cannot
                    // propagate to a caller. Log and move on so the
                    // bus stays live for subsequent envelopes.
                    tracing::error!(
                        error = %err,
                        event_id = %envelope.event_id(),
                        "policy dispatch failed (Retryable); surfaced for caller-side retry but bus loop continues",
                    );
                }
            });
        });

        shutdown.await;
        Ok(())
    }

    /// Drive the publish loop until `Ctrl-C` is received.
    ///
    /// Convenience wrapper around [`Self::run`] using
    /// [`tokio::signal::ctrl_c`] as the shutdown signal.
    ///
    /// # Errors
    ///
    /// Forwarded from [`Self::run`].
    pub async fn run_until_ctrl_c(self) -> Result<(), AgentError> {
        self.run(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    }
}

impl<G, S, B, P, D> std::fmt::Debug for App<G, S, B, P, D>
where
    G: CommandGateway,
    S: EventStore<Event = EventOf<G>>,
    B: EventBus<Event = EventOf<G>>,
    P: ProjectionDriverTuple,
    D: DeadLetterSink,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("policy_count", &self.policy_count())
            .field("projection_arity", &P::ARITY)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherry_pit_core::{
        Aggregate, BusError, Command, CommandGateway, CorrelationContext, CreateResult,
        DispatchResult, DomainEvent, EventBus, EventEnvelope, EventStore, HandleCommand,
        StoreCreateResult, StoreError,
    };
    use serde::{Deserialize, Serialize};
    use std::error::Error;
    use std::num::NonZeroU64;
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum E {
        Happened,
    }
    impl DomainEvent for E {
        fn event_type(&self) -> &'static str {
            "e.happened"
        }
    }

    impl pardosa_encoding::Encode for E {
        fn encode(&self, out: &mut Vec<u8>) {
            match self {
                Self::Happened => out.push(0u8),
            }
        }
    }

    #[derive(Debug, Default)]
    struct Agg;
    impl Aggregate for Agg {
        type Event = E;
        fn apply(&mut self, _e: &E) {}
    }

    #[derive(Debug)]
    struct StoreStub;
    impl EventStore for StoreStub {
        type Event = E;
        async fn load(
            &self,
            _id: cherry_pit_core::AggregateId,
        ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
            Ok(Vec::new())
        }
        async fn create(&self, _events: Vec<E>, _ctx: CorrelationContext) -> StoreCreateResult<E> {
            Err(StoreError::Infrastructure("stub".into()))
        }
        async fn append(
            &self,
            _id: cherry_pit_core::AggregateId,
            _expected: NonZeroU64,
            _events: Vec<E>,
            _ctx: CorrelationContext,
        ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
            Err(StoreError::Infrastructure("stub".into()))
        }
    }

    #[derive(Debug)]
    struct BusStub;
    impl EventBus for BusStub {
        type Event = E;
        async fn publish(&self, _events: &[EventEnvelope<E>]) -> Result<(), BusError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct GatewayStub;
    impl CommandGateway for GatewayStub {
        type Aggregate = Agg;
        async fn create<C>(&self, _cmd: C, _ctx: CorrelationContext) -> CreateResult<Agg, C>
        where
            Agg: HandleCommand<C>,
            C: Command,
        {
            panic!("stub gateway create not used in S5 tests")
        }
        async fn send<C>(
            &self,
            _id: cherry_pit_core::AggregateId,
            _cmd: C,
            _ctx: CorrelationContext,
        ) -> DispatchResult<Agg, C>
        where
            Agg: HandleCommand<C>,
            C: Command,
        {
            panic!("stub gateway send not used in S5 tests")
        }
    }

    struct SinkStub;
    impl DeadLetterSink for SinkStub {
        async fn record(
            &self,
            _record: crate::dead_letter::DeadLetterRecord,
        ) -> Result<(), Box<dyn Error + Send + Sync>> {
            Ok(())
        }
    }

    struct PolicyA;
    impl Policy for PolicyA {
        type Event = E;
        type Output = ();
        fn react(&self, _env: &EventEnvelope<E>) -> Vec<()> {
            Vec::new()
        }
    }

    struct PolicyB;
    impl Policy for PolicyB {
        type Event = E;
        type Output = ();
        fn react(&self, _env: &EventEnvelope<E>) -> Vec<()> {
            Vec::new()
        }
    }

    fn fresh_app() -> App<GatewayStub, StoreStub, BusStub, (), SinkStub> {
        App::new(GatewayStub, StoreStub, BusStub, (), SinkStub)
    }

    fn fresh_app_inproc() -> App<GatewayStub, StoreStub, InProcessEventBus<E>, (), SinkStub> {
        App::new(
            GatewayStub,
            StoreStub,
            InProcessEventBus::new(),
            (),
            SinkStub,
        )
    }

    #[test]
    fn new_constructs_empty_registry() {
        let app = fresh_app();
        assert_eq!(app.policy_count(), 0);
    }

    #[test]
    fn register_policy_increments_count() {
        let mut app = fresh_app();
        app.register_policy(
            PolicyA,
            |_out, _gw, _ctx| async move { Ok(()) },
            "PolicyA",
            "Out",
        );
        assert_eq!(app.policy_count(), 1);
    }

    #[test]
    fn register_two_policies_independent_of_order() {
        // Both orderings yield count=2; identity strings are not order-sensitive.
        let mut app1 = fresh_app();
        app1.register_policy(PolicyA, |_o, _g, _c| async move { Ok(()) }, "A", "Out");
        app1.register_policy(PolicyB, |_o, _g, _c| async move { Ok(()) }, "B", "Out");

        let mut app2 = fresh_app();
        app2.register_policy(PolicyB, |_o, _g, _c| async move { Ok(()) }, "B", "Out");
        app2.register_policy(PolicyA, |_o, _g, _c| async move { Ok(()) }, "A", "Out");

        assert_eq!(app1.policy_count(), 2);
        assert_eq!(app2.policy_count(), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn run_returns_on_shutdown_signal() {
        // S6: run() now subscribes to the InProcessEventBus and awaits
        // shutdown; the multi-thread flavour is required by the
        // block_on bridge inside the register callback.
        let app = fresh_app_inproc();
        let result = app.run(async {}).await;
        assert!(result.is_ok());
    }

    #[test]
    fn debug_includes_arity_and_count() {
        let mut app = fresh_app();
        app.register_policy(PolicyA, |_o, _g, _c| async move { Ok(()) }, "A", "Out");
        let s = format!("{app:?}");
        assert!(s.contains("policy_count: 1"));
        assert!(s.contains("projection_arity: 0"));
    }
}
