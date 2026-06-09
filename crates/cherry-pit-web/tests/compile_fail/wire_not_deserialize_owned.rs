//! Verifies that `CommandRouter::Wire` must implement
//! `serde::de::DeserializeOwned + Send + 'static`
//! (see `crates/cherry-pit-web/src/command_router.rs:220`).
//!
//! Per CHE-0050:R1 the wire DTO — and **only** the wire DTO — carries
//! the `DeserializeOwned` bound; `cherry-pit-core::Command` remains
//! free of `Deserialize` per CHE-0014:R2. The `CommandRouter` trait
//! enforces this at the associated-type declaration.
//!
//! This locks CHE-0049:R5 / CHE-0050:R1 — "the wire boundary carries
//! the serde contract" — from CONVENTION to COVERED. If this fixture
//! ever compiles green, the bound has been silently relaxed and
//! non-deserialisable types could leak into the router's wire surface.
//!
//! Pattern mirrors `build_router_mismatched_gateway.rs` (header doc
//! block, ASCII-only, `fn main() {}`).
use std::num::NonZeroU64;

use cherry_pit_core::{
    Aggregate, AggregateId, Command, CommandGateway, CorrelationContext, CreateResult,
    DispatchResult, DomainEvent, EventEnvelope, EventStore, HandleCommand, StoreCreateResult,
    StoreError,
};
use cherry_pit_web::correlation::IdempotencyKey;
use cherry_pit_web::errors::ErrorEnvelope;
use cherry_pit_web::{CommandRouter, DispatchOutcome};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum E {
    Created,
}
impl DomainEvent for E {
    fn event_type(&self) -> &'static str {
        "e"
    }
}
#[derive(Default)]
struct A;
impl Aggregate for A {
    type Event = E;
    fn apply(&mut self, _: &Self::Event) {}
}
#[derive(Debug)]
struct C;
impl Command for C {}
impl HandleCommand<C> for A {
    type Error = std::convert::Infallible;
    fn handle(&self, _: C) -> Result<Vec<E>, Self::Error> {
        Ok(vec![])
    }
}

#[derive(Clone)]
struct G;
impl CommandGateway for G {
    type Aggregate = A;
    async fn create<Cmd>(&self, _: Cmd, _: CorrelationContext) -> CreateResult<A, Cmd>
    where
        A: HandleCommand<Cmd>,
        Cmd: Command,
    {
        Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
    }
    async fn send<Cmd>(
        &self,
        _: AggregateId,
        _: Cmd,
        _: CorrelationContext,
    ) -> DispatchResult<A, Cmd>
    where
        A: HandleCommand<Cmd>,
        Cmd: Command,
    {
        Ok(vec![])
    }
}

struct S;
impl EventStore for S {
    type Event = E;
    async fn load(&self, _: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        Ok(vec![])
    }
    async fn create(&self, _: Vec<E>, _: CorrelationContext) -> StoreCreateResult<E> {
        Ok((AggregateId::new(NonZeroU64::new(1).unwrap()), vec![]))
    }
    async fn append(
        &self,
        _: AggregateId,
        _: NonZeroU64,
        _: Vec<E>,
        _: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        Ok(vec![])
    }
}

#[derive(Clone)]
struct W;

#[derive(Clone)]
struct R;
impl CommandRouter for R {
    type Gateway = G;
    type Wire = W;
    async fn dispatch(
        &self,
        _: &G,
        _: CorrelationContext,
        _: Option<IdempotencyKey>,
        _: W,
    ) -> Result<DispatchOutcome, ErrorEnvelope> {
        Ok(DispatchOutcome::Sent)
    }
}

fn main() {
    let _ = std::marker::PhantomData::<(A, C, G, S)>;
}
