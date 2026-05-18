//! Application state shared across all `cherry-pit-web` route handlers.
//!
//! Realises **CHE-0049 R1**: state is generic over the consumer's
//! concrete [`CommandGateway`] and [`EventStore`] implementations,
//! never `Box<dyn _>`. Combined with axum's `with_state(state)` this
//! preserves zero-cost dispatch end-to-end (CHE-0005 R1, CHE-0025 R1).
//!
//! Realises **CHE-0050 R2**: a third type parameter `R: CommandRouter`
//! follows `G, S` and is bound to the consumer's wire-deserialize-and-
//! dispatch impl. The order `(G, S, R)` is load-bearing and reflected
//! in [`build_router`](crate::build_router).

use std::sync::Arc;

use cherry_pit_core::{Aggregate, CommandGateway, EventStore};

use crate::command_router::CommandRouter;

/// Application state for a [`build_router`](crate::build_router) call.
///
/// Generic over:
///
/// * `G` — the consumer's concrete [`CommandGateway`].
/// * `S` — the consumer's concrete [`EventStore`], whose
///   [`EventStore::Event`] matches the aggregate's
///   [`Aggregate::Event`] (CHE-0005 R1).
/// * `R` — the consumer's concrete [`CommandRouter`] impl, bound to
///   the same `G` via `R: CommandRouter<Gateway = G>` (CHE-0050 R2).
///
/// # Why generics over `Box<dyn _>`
///
/// Per CHE-0049 R1, dynamic dispatch over the gateway and store is
/// forbidden. Per CHE-0050 R4, the router trait is object-unsafe by
/// construction (associated `Wire` type + generic `Gateway`).
/// `Box<dyn _>` is therefore impossible across all three parameters.
///
/// # Cloning
///
/// `AppState` is cheaply [`Clone`] regardless of whether `G` or `S`
/// implement `Clone`: gateway and store are wrapped in [`Arc`]. The
/// router is required to be `Clone` directly (it tends to be a small
/// stateless or `Arc`-internal value the consumer constructs once at
/// `main`-time). The manual `Clone` impl avoids the over-tight bound a
/// derive would emit (`G: Clone, S: Clone`). axum requires
/// `Clone + Send + Sync + 'static` for any value passed to
/// [`axum::Router::with_state`]; the corresponding bounds appear on `R`.
///
/// # Example
///
/// ```
/// use std::num::NonZeroU64;
/// use std::sync::Arc;
/// use cherry_pit_core::{
///     Aggregate, AggregateId, Command, CommandGateway, CorrelationContext,
///     CreateResult, DispatchResult, DomainEvent, EventEnvelope, EventStore,
///     HandleCommand, StoreCreateResult, StoreError,
/// };
/// use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome};
/// use cherry_pit_web::correlation::IdempotencyKey;
/// use cherry_pit_web::errors::ErrorEnvelope;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum E { Created }
/// impl DomainEvent for E { fn event_type(&self) -> &'static str { "e" } }
/// // CHE-0064:R2 — hand-rolled Encode is mandatory; not derive-able.
/// impl pardosa_encoding::Encode for E {
///     fn encode(&self, out: &mut Vec<u8>) { match self { Self::Created => out.push(0u8) } }
/// }
/// #[derive(Default)] struct A;
/// impl Aggregate for A {
///     type Event = E;
///     fn apply(&mut self, _: &Self::Event) {}
/// }
/// #[derive(Debug)] struct C;
/// impl Command for C {}
/// impl HandleCommand<C> for A {
///     type Error = std::convert::Infallible;
///     fn handle(&self, _: C) -> Result<Vec<E>, Self::Error> { Ok(vec![]) }
/// }
///
/// #[derive(Clone)] struct G;
/// impl CommandGateway for G {
///     type Aggregate = A;
///     async fn create<Cmd>(&self, _: Cmd, _: CorrelationContext) -> CreateResult<A, Cmd>
///         where A: HandleCommand<Cmd>, Cmd: Command
///     { Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![])) }
///     async fn send<Cmd>(&self, _: AggregateId, _: Cmd, _: CorrelationContext) -> DispatchResult<A, Cmd>
///         where A: HandleCommand<Cmd>, Cmd: Command
///     { Ok(vec![]) }
/// }
///
/// struct S;
/// impl EventStore for S {
///     type Event = E;
///     async fn load(&self, _: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> { Ok(vec![]) }
///     async fn create(&self, _: Vec<E>, _: CorrelationContext) -> StoreCreateResult<E> {
///         Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
///     }
///     async fn append(&self, _: AggregateId, _: NonZeroU64, _: Vec<E>, _: CorrelationContext)
///         -> Result<Vec<EventEnvelope<E>>, StoreError>
///     { Ok(vec![]) }
/// }
///
/// #[derive(Deserialize)] struct W;
/// #[derive(Clone)] struct R;
/// impl CommandRouter for R {
///     type Gateway = G;
///     type Wire = W;
///     async fn dispatch(&self, _: &G, _: CorrelationContext, _: Option<IdempotencyKey>, _: W)
///         -> Result<DispatchOutcome, ErrorEnvelope>
///     { Ok(DispatchOutcome::Sent) }
/// }
///
/// // Construct AppState by value — CHE-0049 R1 + CHE-0050 R2.
/// let state = AppState::new(G, S, R);
/// assert!(Arc::strong_count(state.gateway()) >= 1);
///
/// // Or from already-shared Arcs — handy when sharing infra across
/// // multiple routers (e.g. public + admin surfaces).
/// let _ = AppState::from_arcs(Arc::new(G), Arc::new(S), R);
/// ```
pub struct AppState<G, S, R>
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    gateway: Arc<G>,
    store: Arc<S>,
    router: R,
}

impl<G, S, R> AppState<G, S, R>
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    /// Construct a new `AppState` from a gateway, event store, and
    /// router.
    ///
    /// The gateway and store are wrapped in [`Arc`] internally; the
    /// router is stored by value (it is itself required to be `Clone`).
    pub fn new(gateway: G, store: S, router: R) -> Self {
        Self {
            gateway: Arc::new(gateway),
            store: Arc::new(store),
            router,
        }
    }

    /// Construct from already-shared `Arc`s — useful when a single
    /// gateway/store backs more than one router (e.g. a public + admin
    /// surface sharing infrastructure).
    pub fn from_arcs(gateway: Arc<G>, store: Arc<S>, router: R) -> Self {
        Self {
            gateway,
            store,
            router,
        }
    }

    /// Shared reference to the command gateway.
    #[must_use]
    pub fn gateway(&self) -> &Arc<G> {
        &self.gateway
    }

    /// Shared reference to the event store.
    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Reference to the consumer-supplied [`CommandRouter`].
    #[must_use]
    pub fn router(&self) -> &R {
        &self.router
    }
}

impl<G, S, R> Clone for AppState<G, S, R>
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            gateway: Arc::clone(&self.gateway),
            store: Arc::clone(&self.store),
            router: self.router.clone(),
        }
    }
}
