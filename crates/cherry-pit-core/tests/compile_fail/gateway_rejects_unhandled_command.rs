/// Verifies that a CommandGateway rejects dispatching a command the
/// aggregate does not handle — the `Self::Aggregate: HandleCommand<C>`
/// bound causes a compile error.
use cherry_pit_core::{
    Aggregate, Command, CommandGateway, CorrelationContext, CreateResult, DispatchResult,
    HandleCommand, DomainEvent, AggregateId,
};
use serde::{Deserialize, Serialize};
use std::future::Future;

#[derive(Default)]
struct MyAggregate;

#[derive(Debug, Clone, Serialize, Deserialize)]
enum MyEvent {
    Created,
}

impl DomainEvent for MyEvent {
    fn event_type(&self) -> &'static str {
        "created"
    }
}

impl Aggregate for MyAggregate {
    type Event = MyEvent;
    fn apply(&mut self, _event: &Self::Event) {}
}

// CreateOrder IS a command and IS handled.
struct CreateOrder;
impl Command for CreateOrder {}

impl HandleCommand<CreateOrder> for MyAggregate {
    type Error = std::io::Error;
    fn handle(&self, _cmd: CreateOrder) -> Result<Vec<Self::Event>, Self::Error> {
        Ok(vec![MyEvent::Created])
    }
}

// DeleteOrder IS a command but is NOT handled by MyAggregate.
struct DeleteOrder;
impl Command for DeleteOrder {}

struct MyGateway;

impl CommandGateway for MyGateway {
    type Aggregate = MyAggregate;

    fn create<C>(
        &self,
        _cmd: C,
        _context: CorrelationContext,
    ) -> impl Future<Output = CreateResult<Self::Aggregate, C>> + Send
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        async { unimplemented!() }
    }

    fn send<C>(
        &self,
        _id: AggregateId,
        _cmd: C,
        _context: CorrelationContext,
    ) -> impl Future<Output = DispatchResult<Self::Aggregate, C>> + Send
    where
        Self::Aggregate: HandleCommand<C>,
        C: Command,
    {
        async { unimplemented!() }
    }
}

fn main() {
    let gw = MyGateway;
    // This should fail — MyAggregate does NOT handle DeleteOrder.
    let _fail = gw.create(DeleteOrder, CorrelationContext::none());
}
