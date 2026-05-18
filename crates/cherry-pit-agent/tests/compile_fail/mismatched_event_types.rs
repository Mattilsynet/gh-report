//! Negative test: constructing `App::new` with a bus whose `Event`
//! doesn't match the gateway's aggregate event must fail to compile
//! per CHE-0051:R3 + the `EventStore<Event = EventOf<G>>` /
//! `EventBus<Event = EventOf<G>>` bounds in `App`.

use std::convert::Infallible;
use std::num::NonZeroU64;

use cherry_pit_agent::{App, InProcessEventBus, TracingDeadLetterSink};
use cherry_pit_core::{
    Aggregate, AggregateId, BusError, Command, CommandGateway, CorrelationContext, CreateResult,
    DispatchResult, DomainEvent, EventBus, EventEnvelope, EventStore, HandleCommand,
    StoreCreateResult, StoreError,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum EvA {
    A,
}
impl DomainEvent for EvA {
    fn event_type(&self) -> &'static str {
        "a"
    }
}
// CHE-0064:R2 — Encode hand-rolled so the cascade does not obscure
// the real assertion (EventBus<EvB> vs EventStore<EvA> mismatch).
impl pardosa_encoding::Encode for EvA {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::A => out.push(0u8),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum EvB {
    B,
}
impl DomainEvent for EvB {
    fn event_type(&self) -> &'static str {
        "b"
    }
}
// CHE-0064:R2 — Encode hand-rolled (see EvA above).
impl pardosa_encoding::Encode for EvB {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::B => out.push(0u8),
        }
    }
}

#[derive(Debug, Default)]
struct AggA;
impl Aggregate for AggA {
    type Event = EvA;
    fn apply(&mut self, _: &EvA) {}
}

#[derive(Debug)]
struct GwA;
impl CommandGateway for GwA {
    type Aggregate = AggA;
    async fn create<C>(&self, _cmd: C, _ctx: CorrelationContext) -> CreateResult<AggA, C>
    where
        AggA: HandleCommand<C>,
        C: Command,
    {
        unimplemented!()
    }
    async fn send<C>(
        &self,
        _id: AggregateId,
        _cmd: C,
        _ctx: CorrelationContext,
    ) -> DispatchResult<AggA, C>
    where
        AggA: HandleCommand<C>,
        C: Command,
    {
        unimplemented!()
    }
}

#[derive(Debug)]
struct StoreA;
impl EventStore for StoreA {
    type Event = EvA;
    async fn load(&self, _id: AggregateId) -> Result<Vec<EventEnvelope<EvA>>, StoreError> {
        unimplemented!()
    }
    async fn create(
        &self,
        _events: Vec<EvA>,
        _ctx: CorrelationContext,
    ) -> StoreCreateResult<EvA> {
        unimplemented!()
    }
    async fn append(
        &self,
        _id: AggregateId,
        _expected: NonZeroU64,
        _events: Vec<EvA>,
        _ctx: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<EvA>>, StoreError> {
        unimplemented!()
    }
}

fn main() {
    // Bus is parameterised over the WRONG event type — App::new must reject.
    let _app = App::new(
        GwA,
        StoreA,
        InProcessEventBus::<EvB>::new(),
        (),
        TracingDeadLetterSink::new(),
    );
    let _ = Infallible::from;
}
