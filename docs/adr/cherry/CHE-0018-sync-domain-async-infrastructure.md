# CHE-0018. Synchronous Domain Logic, Asynchronous Infrastructure

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0008

## Context

Cherry-pit defines domain traits (`Aggregate::apply`, `HandleCommand::handle`, `Policy::react`, `Projection::apply`) and infrastructure ports (`EventStore`, `EventBus`, `CommandBus`, `CommandGateway`). CHE-0008 establishes pure command handling; CHE-0025 establishes RPITIT for infrastructure. Neither explicitly states the boundary rule. Making domain logic async would allow calling external services during command handling, require async runtimes in tests, and mix I/O with business logic.

## Decision

Domain traits are synchronous. Infrastructure ports are asynchronous.
The boundary is at the port trait signatures.

R1 [5]: All domain traits (Aggregate::apply, HandleCommand::handle,
  Policy::react, Projection::apply) are synchronous
R2 [5]: All infrastructure port traits (EventStore, EventBus,
  CommandBus, CommandGateway) are asynchronous
R3 [5]: cherry-pit-core has zero dependency on any async runtime

**Synchronous (no futures, no runtime dependency):**

```rust
// Aggregate::apply
fn apply(&mut self, event: &Self::Event);

// HandleCommand::handle
fn handle(&self, cmd: C) -> Result<Vec<Self::Event>, Self::Error>;

// Policy::react
fn react(&self, event: &EventEnvelope<Self::Event>) -> Vec<Self::Output>;

// Projection::apply
fn apply(&mut self, event: &EventEnvelope<Self::Event>);
```

**Asynchronous (returns `impl Future<...> + Send`):**

```rust
// EventStore::load, create, append
fn load(&self, id: AggregateId) -> impl Future<Output = ...> + Send;

// EventBus::publish
fn publish(&self, events: &[EventEnvelope<Self::Event>]) -> impl Future<Output = ...> + Send;

// CommandBus::create, dispatch
fn create<C>(&self, cmd: C) -> impl Future<Output = ...> + Send;

// CommandGateway::create, send
fn create<C>(&self, cmd: C) -> impl Future<Output = ...> + Send;
```

**Dependency consequence:** `cherry-pit-core` has zero dependency on any
async runtime (no tokio, no async-std). It depends only on `serde`,
`uuid`, and `jiff`. The domain crate is runtime-agnostic.

## Consequences

- Domain logic is testable without an async runtime — plain `#[test]` functions.
- Aggregates cannot call external services during handling; callers must provide data in the command (consistent with CHE-0008).
- Policies cannot perform async I/O; they return commands for infrastructure to dispatch asynchronously.
- Projections build read models purely from events.
- `cherry-pit-core` has no async runtime dependency — only `serde`, `uuid`, `jiff`. Any runtime can drive infrastructure ports.
- The boundary is auditable: `impl Future` return = infrastructure; plain data return = domain.
