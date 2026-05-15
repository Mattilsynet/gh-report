# CHE-0017. Policy Output as Static Associated Type

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: CHE-0004

## Context

Policies react to domain events by producing outputs — typically
commands to dispatch to other aggregates. The output type must be
defined somewhere. Options:

1. **`Box<dyn Any>`** — fully erased output. Policies can produce
   anything. Infrastructure must downcast at runtime. No compile-time
   verification of output types.
2. **`Box<dyn Command>`** — type-erased but constrained to commands.
   Still requires runtime downcasting for dispatch. Heap allocation
   per output.
3. **Static associated type** — `type Output: Send + Sync + 'static`.
   The agent defines an enum of possible outputs. The compiler
   verifies exhaustive matching when infrastructure dispatches them.

## Decision

```rust
pub trait Policy: Send + Sync + 'static {
    type Event: DomainEvent;
    type Output: Send + Sync + 'static;
    fn react(&self, event: &EventEnvelope<Self::Event>) -> Vec<Self::Output>;
}
```

`Output` is a static associated type with minimal bounds
(`Send + Sync + 'static`). It is deliberately NOT bounded by
`Command`.

The agent defines an output enum:

```rust
enum OrderPolicyOutput {
    NotifyWarehouse(NotifyWarehouseCommand),
    UpdateInventory(AggregateId, UpdateInventoryCommand),
}
```

Infrastructure dispatches by matching on the enum — the compiler
verifies all variants are handled.

R1 [5]: Policy::Output is a static associated type with Send + Sync
  + 'static bounds, not bounded by Command
R2 [5]: Define policy output as an enum so the compiler verifies
  exhaustive matching at all dispatch sites

## Consequences

- No runtime type errors. The output enum is exhaustively matched at compile time — new variants cause compilation failures until handled.
- No heap allocation per output.
- **`Output` is not bounded by `Command`.** Policy outputs may carry routing context (target `AggregateId`, cross-context metadata) beyond the command itself. Requiring `Command` on `Output` would conflate routing with intent.
- The infrastructure dispatch layer must know the output enum type, creating coupling resolved by `cherry-pit-agent`.
- Policies receive `EventEnvelope`, not raw events — they need metadata to construct correctly targeted outputs.
