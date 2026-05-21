//! Negative test: `App::register_policy`'s closure shape is
//! `Fn(P::Output, &G, CorrelationContext) -> Future`. Omitting the
//! third `CorrelationContext` parameter must fail to compile
//! (CHE-0051:R4 + R6 amendment).

use std::num::NonZeroU64;

use cherry_pit_agent::{App, InProcessEventBus, TracingDeadLetterSink};
use cherry_pit_core::{
    Aggregate, AggregateId, BusError, Command, CommandGateway, CorrelationContext, CreateResult,
    DispatchResult, DomainEvent, EventBus, EventEnvelope, EventStore, HandleCommand, Policy,
    StoreCreateResult, StoreError,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum Ev {
    X,
}
impl DomainEvent for Ev {
    fn event_type(&self) -> &'static str {
        "ev.x"
    }
}

#[derive(Debug, Default)]
struct Agg;
impl Aggregate for Agg {
    type Event = Ev;
    fn apply(&mut self, _: &Ev) {}
}

#[derive(Debug)]
struct Gw;
impl CommandGateway for Gw {
    type Aggregate = Agg;
    async fn create<C>(&self, _cmd: C, _ctx: CorrelationContext) -> CreateResult<Agg, C>
    where
        Agg: HandleCommand<C>,
        C: Command,
    {
        unimplemented!()
    }
    async fn send<C>(
        &self,
        _id: AggregateId,
        _cmd: C,
        _ctx: CorrelationContext,
    ) -> DispatchResult<Agg, C>
    where
        Agg: HandleCommand<C>,
        C: Command,
    {
        unimplemented!()
    }
}

#[derive(Debug)]
struct St;
impl EventStore for St {
    type Event = Ev;
    async fn load(&self, _id: AggregateId) -> Result<Vec<EventEnvelope<Ev>>, StoreError> {
        unimplemented!()
    }
    async fn create(&self, _events: Vec<Ev>, _ctx: CorrelationContext) -> StoreCreateResult<Ev> {
        unimplemented!()
    }
    async fn append(
        &self,
        _id: AggregateId,
        _expected: NonZeroU64,
        _events: Vec<Ev>,
        _ctx: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Ev>>, StoreError> {
        unimplemented!()
    }
}

struct P;
impl Policy for P {
    type Event = Ev;
    type Output = ();
    fn react(&self, _env: &EventEnvelope<Ev>) -> Vec<()> {
        Vec::new()
    }
}

fn main() {
    let mut app = App::new(
        Gw,
        St,
        InProcessEventBus::<Ev>::new(),
        (),
        TracingDeadLetterSink::new(),
    );
    // Closure missing the third `CorrelationContext` parameter — must fail.
    app.register_policy(
        P,
        |_out, _gw: &Gw| async move { Ok(()) },
        "P",
        "()",
    );
}
