# CHE-0025. RPITIT Over async_trait

Date: 2026-04-24
Last-reviewed: 2026-08-31 — refined — Decision and Consequences MSRV numerals corrected 1.96 -> 1.98 to match rust-toolchain.toml/Cargo.toml ground truth (drift predated the 1.97 cycle); R1-R3 are numeral-free and unchanged (mission:ghr-508w6)
Tier: D
Status: Accepted

## Related

References: CHE-0018

## Context

Cherry-pit's port traits (EventStore, CommandBus, CommandGateway, EventBus) are async. `async_trait` wraps return types in `Box<dyn Future>`, causing a heap allocation per call. RPITIT (`-> impl Future<...> + Send`) is zero-cost: the compiler monomorphizes each impl. Command dispatch is a hot path where per-call allocation is measurable overhead.

## Decision

All async port traits use RPITIT (`impl Future` in return position)
instead of the `async_trait` proc macro. The minimum supported Rust
version is 1.98 (edition 2024).

R1 [9]: All async port traits use impl Future in return position
  instead of the async_trait proc macro
R2 [9]: No heap allocation per async trait method call via
  Box<dyn Future>
R3 [9]: CI enforces R1/R2 with a build-time tripwire (job id
  build-test-lint, step "deny async-trait in cherry-pit-* dep trees",
  .github/workflows/ci-reusable.yml): async-trait present in any
  cherry-pit-* crate's resolved dep tree fails the build (COM-0017:R4).

## Consequences

- Zero heap allocation per command dispatch.
- Object safety permanently sacrificed — no `dyn EventStore`. Consistent with single-aggregate design (concrete types everywhere).
- The `Send` bound on returned futures constrains adapter implementations.
- Trait signatures use explicit `-> impl Future<...> + Send` rather than `async fn` sugar.
- MSRV of 1.98 excludes older toolchains. Acceptable for a pre-1.0 project.
