# CHE-0025. RPITIT Over async_trait

Date: 2026-04-24
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0018

## Context

Cherry-pit's port traits (EventStore, CommandBus, CommandGateway, EventBus) are async. `async_trait` wraps return types in `Box<dyn Future>`, causing a heap allocation per call. RPITIT (`-> impl Future<...> + Send`) is zero-cost: the compiler monomorphizes each impl. Command dispatch is a hot path where per-call allocation is measurable overhead.

## Decision

All async port traits use RPITIT (`impl Future` in return position)
instead of the `async_trait` proc macro. The minimum supported Rust
version is 1.95 (edition 2024).

R1 [9]: All async port traits use impl Future in return position
  instead of the async_trait proc macro
R2 [9]: No heap allocation per async trait method call via
  Box<dyn Future>

## Consequences

- Zero heap allocation per command dispatch.
- Object safety permanently sacrificed — no `dyn EventStore`. Consistent with single-aggregate design (concrete types everywhere).
- The `Send` bound on returned futures constrains adapter implementations.
- Trait signatures use explicit `-> impl Future<...> + Send` rather than `async fn` sugar.
- MSRV of 1.95 excludes older toolchains. Acceptable for a pre-1.0 project.
