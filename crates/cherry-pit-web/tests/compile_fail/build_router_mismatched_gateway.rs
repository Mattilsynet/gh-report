//! Verifies that `build_router<G, S, R>` (and `AppState::new`) refuses
//! a `CommandRouter` whose associated `Gateway` differs from the `G`
//! passed alongside it.
//!
//! `crates/cherry-pit-web/src/state.rs` and
//! `crates/cherry-pit-web/src/router.rs` both carry the bound
//! `R: CommandRouter<Gateway = G>` (state.rs:123, router.rs:124). The
//! associated-type equality keeps the gateway plumbed into the router
//! and the gateway plumbed into `AppState` referring to the same
//! aggregate.
//!
//! This locks CHE-0049:R3 / CHE-0050:R2 — "gateway and router agree on
//! the aggregate" — from CONVENTION to COVERED. If this fixture ever
//! compiles green, the associated-type binding has been silently
//! relaxed and a router for aggregate `A2` could be glued to a gateway
//! for aggregate `A1`.
//!
//! Pattern mirrors `projection_source_not_object_safe.rs` (header doc
//! block, ASCII-only, `fn main() {}`).
use std::num::NonZeroU64;

use cherry_pit_core::{
    Aggregate, AggregateId, Command, CommandGateway, CorrelationContext, CreateResult,
    DispatchResult, DomainEvent, EventEnvelope, EventStore, HandleCommand, StoreCreateResult,
    StoreError,
};
use cherry_pit_web::correlation::IdempotencyKey;
use cherry_pit_web::errors::ErrorEnvelope;
use cherry_pit_web::{AppState, CommandRouter, DispatchOutcome};
use serde::{Deserialize, Serialize};

// --- Aggregate A1 + event E1 ---
#[derive(Debug, Clone, Serialize, Deserialize)]
enum E1 {
    Created,
}
impl DomainEvent for E1 {
    fn event_type(&self) -> &'static str {
        "e1"
    }
}
// CHE-0064:R2 — Encode hand-rolled so the cascade does not obscure
// the real assertion (Gateway = G mismatch at AppState::new).
impl pardosa_encoding::Encode for E1 {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Created => out.push(0u8),
        }
    }
}
#[derive(Default)]
struct A1;
impl Aggregate for A1 {
    type Event = E1;
    fn apply(&mut self, _: &Self::Event) {}
}
#[derive(Debug)]
struct C1;
impl Command for C1 {}
impl HandleCommand<C1> for A1 {
    type Error = std::convert::Infallible;
    fn handle(&self, _: C1) -> Result<Vec<E1>, Self::Error> {
        Ok(vec![])
    }
}

// --- Aggregate A2 + event E2 (distinct from A1) ---
#[derive(Debug, Clone, Serialize, Deserialize)]
enum E2 {
    Created,
}
impl DomainEvent for E2 {
    fn event_type(&self) -> &'static str {
        "e2"
    }
}
// CHE-0064:R2 — Encode hand-rolled (see E1 above).
impl pardosa_encoding::Encode for E2 {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Created => out.push(0u8),
        }
    }
}
#[derive(Default)]
struct A2;
impl Aggregate for A2 {
    type Event = E2;
    fn apply(&mut self, _: &Self::Event) {}
}
#[derive(Debug)]
struct C2;
impl Command for C2 {}
impl HandleCommand<C2> for A2 {
    type Error = std::convert::Infallible;
    fn handle(&self, _: C2) -> Result<Vec<E2>, Self::Error> {
        Ok(vec![])
    }
}

// --- Gateway G1 over A1 ---
#[derive(Clone)]
struct G1;
impl CommandGateway for G1 {
    type Aggregate = A1;
    async fn create<Cmd>(&self, _: Cmd, _: CorrelationContext) -> CreateResult<A1, Cmd>
    where
        A1: HandleCommand<Cmd>,
        Cmd: Command,
    {
        Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
    }
    async fn send<Cmd>(
        &self,
        _: AggregateId,
        _: Cmd,
        _: CorrelationContext,
    ) -> DispatchResult<A1, Cmd>
    where
        A1: HandleCommand<Cmd>,
        Cmd: Command,
    {
        Ok(vec![])
    }
}

// --- Gateway G2 over A2 ---
#[derive(Clone)]
struct G2;
impl CommandGateway for G2 {
    type Aggregate = A2;
    async fn create<Cmd>(&self, _: Cmd, _: CorrelationContext) -> CreateResult<A2, Cmd>
    where
        A2: HandleCommand<Cmd>,
        Cmd: Command,
    {
        Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
    }
    async fn send<Cmd>(
        &self,
        _: AggregateId,
        _: Cmd,
        _: CorrelationContext,
    ) -> DispatchResult<A2, Cmd>
    where
        A2: HandleCommand<Cmd>,
        Cmd: Command,
    {
        Ok(vec![])
    }
}

// --- EventStore matching G1 (Event = E1) ---
struct S1;
impl EventStore for S1 {
    type Event = E1;
    async fn load(&self, _: AggregateId) -> Result<Vec<EventEnvelope<E1>>, StoreError> {
        Ok(vec![])
    }
    async fn create(&self, _: Vec<E1>, _: CorrelationContext) -> StoreCreateResult<E1> {
        Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
    }
    async fn append(
        &self,
        _: AggregateId,
        _: NonZeroU64,
        _: Vec<E1>,
        _: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E1>>, StoreError> {
        Ok(vec![])
    }
}

// --- Wire DTO ---
#[derive(Deserialize)]
struct W;

// --- CommandRouter R bound to G2 (NOT G1) ---
#[derive(Clone)]
struct R;
impl CommandRouter for R {
    type Gateway = G2;
    type Wire = W;
    async fn dispatch(
        &self,
        _: &G2,
        _: CorrelationContext,
        _: Option<IdempotencyKey>,
        _: W,
    ) -> Result<DispatchOutcome, ErrorEnvelope> {
        Ok(DispatchOutcome::Sent)
    }
}

fn main() {
    // G1 + S1 align, but R::Gateway = G2 ≠ G1. The `R: CommandRouter<Gateway = G>`
    // bound on AppState::new must reject this.
    let _state = AppState::new(G1, S1, R);
}
