# CHE-0005. Single-Aggregate Design with Compile-Time Type Safety

Date: 2026-04-24
Last-reviewed: 2026-04-28
Tier: S
Status: Accepted

## Related

References: CHE-0001

## Context

Infrastructure ports (EventStore, EventBus, CommandBus, CommandGateway) need to handle aggregate-specific event and command types. Generic per-call ports accept any type per method with runtime type checking — flexible but allows cross-aggregate confusion (loading Order events as Inventory events). Single-aggregate binding via associated types locks each port instance to one aggregate/event type — cross-aggregate confusion becomes a compile error, not a runtime error. The tradeoff: associated types sacrifice object safety (`Box<dyn EventStore>` is impossible) for compile-time guarantees.

## Decision

Every infrastructure port is bound to a single aggregate/event type via
associated types. Multiple aggregates require separate bounded contexts,
each with its own typed infrastructure stack. Cross-context communication
happens through event subscriptions (e.g. NATS subjects), not shared
stores.

Compile-fail tests in cherry-pit-core prove the guarantees hold.

R1 [2]: Bind every infrastructure port to a single aggregate type via
  associated types
R2 [2]: Require separate bounded contexts with typed infrastructure
  stacks for multiple aggregates
R3 [2]: Use event subscriptions for cross-context communication, not
  shared stores

## Consequences

- The compiler prevents cross-aggregate type confusion at every layer. No runtime downcasting, no `Any`, no type-erased envelopes.
- Wiring complexity scales linearly with aggregate count. `cherry-pit-agent` must solve this ergonomically.
- Object safety is sacrificed — no `Box<dyn EventStore>`. No heterogeneous store collections, no runtime implementation selection. Every aggregate requires its own concrete store instance wired at compile time.
- Cross-aggregate coordination (sagas) uses `Policy` traits reacting to events and emitting commands to another aggregate.
- `Policy::Output` is typed as `Send + Sync + 'static` without a `Command` bound, allowing flexible output enums.
