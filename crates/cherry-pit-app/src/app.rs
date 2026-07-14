//! Root composition struct wiring `(CommandGateway, EventStore,
//! EventBus, ProjectionDriverTuple, DeadLetterSink, [Policy…])` per
//! CHE-0051:R3 + R4 + R5 + R7 + R8.
//!
//! v0.1 surface: [`App::new`] (five-slot constructor, no `Default`
//! per CHE-0039:R2 + CHE-0051:R3), [`App::register_policy`]
//! (per-policy dispatch closure per CHE-0051:R4 + CHE-0017:R2), and
//! [`App::run`] (F2 bounded dispatch-channel publish loop; mission
//! adr-fmt-cq7vb.2 Approach A2).
//!
//! `App<G, S, B, P, D>` is single-aggregate per CHE-0051:R9; five
//! type parameters cover the composition surface without `Box<dyn>`
//! over any CHE-0005:R1 infra port, with the event type pinned by
//! `G::Aggregate`'s associated `Event`.
//!
//! The policy registry stores
//! `Vec<Box<dyn ErasedPolicyDispatcher<E, G>>>` — a per-policy
//! `(Policy, dispatch_closure)` adapter, not `Box<dyn Policy>`.
//! CHE-0005:R1 forbids erasing aggregate-bound infra ports; this
//! erasure is at the internal dispatcher boundary over user closures
//! (same reasoning approved for `InProcessEventBus`, see
//! `event_bus.rs:22-37` and `dispatch.rs:21-37`).

use std::future::Future;
use std::sync::Arc;

use cherry_pit_core::{
    Aggregate, CommandGateway, CorrelationContext, DomainEvent, EventBus, EventEnvelope,
    EventStore, Policy,
};
use tokio::sync::mpsc;

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

/// Default capacity for the bounded dispatch channel between the bus
/// callback and the single sequential consumer task (F2 / mission
/// adr-fmt-cq7vb.2, Approach A2).
///
/// 1024 is a generous default for in-process EDA workloads: it
/// absorbs typical bursts (publisher fan-out from a single command
/// dispatch) while remaining small enough that overflow under
/// sustained pressure surfaces quickly as `try_send` failures (logged
/// at warn) rather than as silent unbounded growth. Callers with
/// known burst shapes can override via
/// [`App::with_dispatch_buffer_capacity`].
const DEFAULT_DISPATCH_BUFFER_CAPACITY: usize = 1024;

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
/// use cherry_pit_app::{App, InProcessEventBus, TracingDeadLetterSink};
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

    /// Capacity of the bounded dispatch channel between the bus
    /// callback and the single sequential consumer task (F2 /
    /// adr-fmt-cq7vb.2, Approach A2). Configured via
    /// [`Self::with_dispatch_buffer_capacity`]; defaults to
    /// [`DEFAULT_DISPATCH_BUFFER_CAPACITY`]. Read by
    /// [`Self::run`] when standing up the consumer.
    dispatch_buffer_capacity: usize,
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
    /// bd `adr-fmt-ur04` (S8+, post-v0.1).
    pub fn new(gateway: G, store: S, bus: B, projections: P, dead_letter: D) -> Self {
        Self {
            gateway,
            store,
            bus,
            projections,
            dead_letter,
            policies: Arc::new(Vec::new()),
            dispatch_buffer_capacity: DEFAULT_DISPATCH_BUFFER_CAPACITY,
        }
    }

    /// Override the bounded dispatch-channel capacity used by
    /// [`Self::run`] (F2 / mission adr-fmt-cq7vb.2 Approach A2).
    ///
    /// Saturation surfaces as a dropped envelope plus `tracing::warn!`
    /// inside the bus callback; the publisher never blocks on a full
    /// channel. Pick capacity from expected per-aggregate burst size —
    /// the default ([`DEFAULT_DISPATCH_BUFFER_CAPACITY`] = `1024`)
    /// covers typical EDA workloads where one command fans out a
    /// handful of envelopes synchronously inside `publish`.
    ///
    /// # Panics
    ///
    /// Panics if `capacity == 0`, failing closed here with a clearer
    /// message than the panic inside `tokio::sync::mpsc::channel`,
    /// so misconfiguration surfaces at construction time rather than
    /// inside [`Self::run`].
    #[must_use]
    pub fn with_dispatch_buffer_capacity(mut self, capacity: usize) -> Self {
        assert!(capacity > 0, "dispatch_buffer_capacity must be > 0; got 0");
        self.dispatch_buffer_capacity = capacity;
        self
    }

    /// Register a policy + its dispatch closure per CHE-0051:R4.
    ///
    /// The closure shape `Fn(P::Output, &G, CorrelationContext) ->
    /// Future<Output = Result<(), AgentError>>` mirrors CHE-0017:R2 —
    /// caller writes the exhaustive output matcher.
    ///
    /// Per CHE-0051:R4 + R6, the dispatcher constructs a
    /// `CorrelationContext` per envelope and passes it as `ctx`; the
    /// closure threads `ctx` into `gateway.send(...)` so
    /// policy-emitted commands inherit the correlation chain.
    /// `policy_identity`/`output_type` are stable strings used for
    /// dead-letter records (CHE-0024:R5 + CHE-0040:R3 + CHE-0051:R7).
    ///
    /// # Example
    ///
    /// `no_run` — needs a multi-thread tokio runtime; demonstrates the
    /// closure shape only.
    ///
    /// ```no_run
    /// use cherry_pit_app::{App, AgentError, CorrelationContext};
    /// # async fn wire<G, S, B, P, D, Pol>(mut app: App<G, S, B, P, D>, policy: Pol)
    /// # where
    /// #     G: cherry_pit_core::CommandGateway,
    /// #     S: cherry_pit_core::EventStore<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event>,
    /// #     B: cherry_pit_core::EventBus<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event>,
    /// #     P: cherry_pit_app::ProjectionDriverTuple,
    /// #     D: cherry_pit_app::DeadLetterSink,
    /// #     Pol: cherry_pit_core::Policy<Event = <G::Aggregate as cherry_pit_core::Aggregate>::Event, Output = ()>,
    /// # {
    /// app.register_policy(
    ///     policy,
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
    /// clones. Currently **unreachable**: [`Self::run`] consumes
    /// `self`, so no path clones the registry before `run`. The guard
    /// becomes load-bearing once a future registry clone is handed to
    /// the publish loop.
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

/// Forward a published envelope into the dispatch channel without
/// blocking the publisher.
///
/// Called from the bus callback installed by [`App::run`]. Per F2
/// (mission adr-fmt-cq7vb.2 / Approach A2), the publisher-side
/// contract is **non-blocking**: a full channel surfaces as a
/// `tracing::warn!` and a dropped envelope, never as a blocked
/// publisher.
///
/// Three outcomes:
///
/// - `Ok` — envelope is now in the consumer's queue.
/// - `Err(Full)` — bounded channel saturated; envelope dropped with
///   `tracing::warn!`. The intended back-pressure surface.
/// - `Err(Closed)` — the consumer is gone (post-shutdown drain).
///   Logged at `tracing::debug!` since this is expected during
///   teardown when the bus has not yet been dropped.
#[doc(hidden)]
pub fn enqueue_or_log<E>(tx: &mpsc::Sender<EventEnvelope<E>>, envelope: &EventEnvelope<E>)
where
    E: DomainEvent,
{
    let event_id = envelope.event_id();
    match tx.try_send(envelope.clone()) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(
                %event_id,
                capacity = tx.max_capacity(),
                "dispatch channel full; dropping envelope (F2 back-pressure surface, mission adr-fmt-cq7vb.2). \
                 Increase App::with_dispatch_buffer_capacity or reduce publish rate.",
            );
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::debug!(
                %event_id,
                "dispatch channel closed; envelope dropped (post-shutdown drain in progress)",
            );
        }
    }
}

/// Sequential consumer task body for [`App::run`].
///
/// Pulls envelopes from `rx` one at a time and runs the per-policy
/// dispatcher serially in receive order, preserving per-aggregate
/// event ordering through dispatch (F2 / Approach A2). Exits when
/// `rx.recv()` returns `None` — i.e. when the sender held inside the
/// bus callback has been dropped, which [`App::run`] arranges by
/// dropping `self.bus` after `shutdown` resolves.
///
/// On dispatch error, behaviour matches the pre-F2 shape: `Terminal`
/// failures are dead-lettered inside `dispatch_one`; `Retryable`
/// failures surface as `tracing::error!` and do NOT abort the
/// consumer (CHE-0051:R7 + CHE-0046:R2). One bad envelope must not
/// stop the bus.
async fn run_dispatch_consumer<E, G, D>(
    mut rx: mpsc::Receiver<EventEnvelope<E>>,
    policies: Arc<Vec<Box<dyn ErasedPolicyDispatcher<E, G>>>>,
    gateway: Arc<G>,
    dead_letter: Arc<D>,
) where
    E: DomainEvent,
    G: Send + Sync + 'static,
    D: DeadLetterSink + Send + Sync + 'static,
{
    while let Some(envelope) = rx.recv().await {
        if let Err(err) =
            dispatch::dispatch_one(&policies, &envelope, &*gateway, &*dead_letter).await
        {
            tracing::error!(
                error = %err,
                event_id = %envelope.event_id(),
                "policy dispatch failed (Retryable); surfaced for caller-side retry but consumer continues",
            );
        }
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
    /// Drive the publish loop until `shutdown` resolves, then drain
    /// any in-flight dispatch before returning.
    ///
    /// Wires the bounded dispatch channel (F2 / mission
    /// adr-fmt-cq7vb.2, Approach A2): sizes an `mpsc::channel` per
    /// [`Self::with_dispatch_buffer_capacity`] (default
    /// [`DEFAULT_DISPATCH_BUFFER_CAPACITY`]); spawns one sequential
    /// consumer per CHE-0051:R7 + CHE-0024:R5 + CHE-0046:R2
    /// (`Terminal` dead-lettered, `Retryable` logged, neither
    /// aborts); installs a synchronous fan-out callback on
    /// [`InProcessEventBus::register`] (CHE-0024:R2 + CHE-0051:R2)
    /// forwarding via non-blocking `try_send`; awaits `shutdown`,
    /// drops the sender to drain the consumer to `None`, then awaits
    /// its completion.
    ///
    /// One `run` impl exists per concrete bus type (CHE-0024:R2).
    ///
    /// **Ordering.** Bus delivery is publication-order (CHE-0024:§7);
    /// the consumer runs `dispatch_one` serially — no cross-envelope
    /// parallelism.
    ///
    /// **Back-pressure.** A saturated channel drops the envelope with
    /// `tracing::warn!`; `publish().await` never blocks.
    ///
    /// **Drain on shutdown.** The consumer drains fully before this
    /// future resolves.
    ///
    /// # Hazards
    ///
    /// The bus callback runs synchronously inside `publish()`
    /// (CHE-0024:§7 holds) but only `try_send`s; dispatch runs on the
    /// consumer task and may finish after `publish().await` returns —
    /// consistent with CHE-0024:R1 persist-then-publish.
    ///
    /// # Panics
    ///
    /// Panics outside an active tokio runtime (`tokio::spawn`
    /// requires one). Wrap your main in `#[tokio::main]`.
    ///
    /// # Errors
    ///
    /// Never returns `Err` — failures are dead-lettered or logged
    /// inside the consumer; the signature reserves future bus-level
    /// errors per CHE-0051:R8. Per **COM-0025:R1**, `Retryable` retry
    /// orchestration is the consumer's responsibility, not `run`'s.
    pub async fn run<Sd>(self, shutdown: Sd) -> Result<(), AgentError>
    where
        Sd: Future<Output = ()> + Send,
    {
        let policies = Arc::clone(&self.policies);
        let gateway = Arc::new(self.gateway);
        let dead_letter = Arc::new(self.dead_letter);

        let (tx, rx) = mpsc::channel::<EventEnvelope<EventOf<G>>>(self.dispatch_buffer_capacity);

        let consumer = tokio::spawn(run_dispatch_consumer(
            rx,
            policies,
            Arc::clone(&gateway),
            Arc::clone(&dead_letter),
        ));

        self.bus.register(move |envelope| {
            enqueue_or_log(&tx, envelope);
        });

        shutdown.await;
        drop(self.bus);
        match consumer.await {
            Ok(()) => Ok(()),
            Err(join_err) => {
                tracing::error!(
                    error = %join_err,
                    "dispatch consumer task panicked or was cancelled; \
                     App::run returning Ok per CHE-0051:R8 (bus-loop liveness)",
                );
                Ok(())
            }
        }
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

    struct BackPressurePolicy;
    impl Policy for BackPressurePolicy {
        type Event = E;
        type Output = ();
        fn react(&self, _env: &EventEnvelope<E>) -> Vec<()> {
            vec![()]
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

    #[test]
    fn with_dispatch_buffer_capacity_overrides_default() {
        let app = fresh_app().with_dispatch_buffer_capacity(64);
        assert_eq!(app.dispatch_buffer_capacity, 64);
    }

    #[test]
    fn new_uses_default_dispatch_buffer_capacity() {
        let app = fresh_app();
        assert_eq!(
            app.dispatch_buffer_capacity,
            DEFAULT_DISPATCH_BUFFER_CAPACITY
        );
    }

    #[test]
    #[should_panic(expected = "dispatch_buffer_capacity must be > 0")]
    fn with_dispatch_buffer_capacity_zero_panics() {
        let _app = fresh_app().with_dispatch_buffer_capacity(0);
    }

    /// F2 / mission adr-fmt-cq7vb.2 back-pressure invariant.
    ///
    /// Wires a bounded channel of capacity 4 between a fast publisher
    /// (calling `enqueue_or_log` directly, the same path the bus
    /// callback installed by `App::run` takes) and a single sequential
    /// consumer running `run_dispatch_consumer` with a blocking
    /// dispatch policy. Two invariants are pinned:
    ///
    /// 1. **Publisher never blocks.** The publish loop runs to
    ///    completion synchronously and well within budget even though
    ///    the consumer is stalled — the `enqueue_or_log`/`try_send`
    ///    contract returns immediately regardless of channel state.
    /// 2. **The bound is enforced.** With capacity 4 and a blocked
    ///    consumer, total dispatches stay in `[capacity, capacity +
    ///    1]` (exact value depends on the publisher/consumer race)
    ///    and strictly below `total_published` — orders of magnitude
    ///    smaller, confirming no unbounded spawn growth.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn full_dispatch_channel_drops_overflow_without_blocking_publisher() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};
        use tokio::sync::{Semaphore, mpsc};

        let capacity = 4_usize;
        let total_published = 32_u64;
        let max_dispatchable = capacity + 1;

        let dispatched = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(Semaphore::new(0));

        let dispatched_for_policy = Arc::clone(&dispatched);
        let gate_for_policy = Arc::clone(&gate);

        let adapter = crate::dispatch::make_adapter::<BackPressurePolicy, _, _, GatewayStub>(
            BackPressurePolicy,
            move |_out, _gw, _ctx| {
                let dispatched = Arc::clone(&dispatched_for_policy);
                let gate = Arc::clone(&gate_for_policy);
                async move {
                    let permit = gate.acquire().await.expect("gate never closes");
                    permit.forget();
                    dispatched.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            "BackPressurePolicy",
            "Out",
        );

        let policies: Arc<Vec<Box<dyn ErasedPolicyDispatcher<E, GatewayStub>>>> =
            Arc::new(vec![adapter]);
        let gateway = Arc::new(GatewayStub);
        let sink = Arc::new(SinkStub);

        let (tx, rx) = mpsc::channel::<EventEnvelope<E>>(capacity);
        let consumer = tokio::spawn(run_dispatch_consumer(
            rx,
            Arc::clone(&policies),
            Arc::clone(&gateway),
            Arc::clone(&sink),
        ));

        let publish_start = Instant::now();
        for seq in 1..=total_published {
            enqueue_or_log(&tx, &back_pressure_envelope(seq));
        }
        let publish_elapsed = publish_start.elapsed();
        assert!(
            publish_elapsed < Duration::from_millis(500),
            "publisher must not block on a full channel; loop took {publish_elapsed:?} \
             which exceeds the 500ms non-blocking budget",
        );

        gate.add_permits(usize::try_from(total_published).unwrap());

        drop(tx);
        let join = tokio::time::timeout(Duration::from_secs(5), consumer)
            .await
            .expect("dispatch consumer must terminate within timeout");
        join.expect("consumer task must not panic");

        let final_count = dispatched.load(Ordering::SeqCst);
        assert!(
            final_count >= capacity && final_count <= max_dispatchable,
            "expected dispatched count in [{capacity}..={max_dispatchable}] under capacity={capacity}; \
             got {final_count} from {total_published} published — bound is broken",
        );
        assert!(
            final_count < usize::try_from(total_published).unwrap(),
            "expected strictly fewer dispatches ({final_count}) than published ({total_published}); \
             equal counts would indicate the bound is not enforced",
        );
    }

    /// F2 / mission adr-fmt-cq7vb.2 graceful-drain invariant.
    ///
    /// Wires a bounded channel of generous capacity (16) and a
    /// non-blocking dispatch closure. Publishes a small batch then
    /// immediately drops the sender (simulating `App::run`'s
    /// shutdown-drop step). The consumer must drain every queued
    /// envelope before exiting.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn consumer_drains_remaining_queue_after_sender_drop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Duration;
        use tokio::sync::mpsc;

        let dispatched = Arc::new(AtomicUsize::new(0));
        let dispatched_for_policy = Arc::clone(&dispatched);

        let adapter = crate::dispatch::make_adapter::<BackPressurePolicy, _, _, GatewayStub>(
            BackPressurePolicy,
            move |_out, _gw, _ctx| {
                let dispatched = Arc::clone(&dispatched_for_policy);
                async move {
                    dispatched.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
            "BackPressurePolicy",
            "Out",
        );
        let policies: Arc<Vec<Box<dyn ErasedPolicyDispatcher<E, GatewayStub>>>> =
            Arc::new(vec![adapter]);
        let gateway = Arc::new(GatewayStub);
        let sink = Arc::new(SinkStub);

        let (tx, rx) = mpsc::channel::<EventEnvelope<E>>(16);
        let consumer = tokio::spawn(run_dispatch_consumer(
            rx,
            Arc::clone(&policies),
            Arc::clone(&gateway),
            Arc::clone(&sink),
        ));

        let batch = 7_u64;
        for seq in 1..=batch {
            enqueue_or_log(&tx, &back_pressure_envelope(seq));
        }
        drop(tx);

        let join = tokio::time::timeout(Duration::from_secs(5), consumer)
            .await
            .expect("dispatch consumer must terminate after sender drop");
        join.expect("consumer task must not panic");

        assert_eq!(
            dispatched.load(Ordering::SeqCst),
            usize::try_from(batch).unwrap(),
            "consumer must dispatch every queued envelope before exiting on rx.recv() → None",
        );
    }

    fn back_pressure_envelope(seq: u64) -> EventEnvelope<E> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            cherry_pit_core::AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(seq).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            E::Happened,
        )
        .unwrap()
    }
}
