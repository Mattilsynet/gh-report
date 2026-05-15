# CHE-0028. Compile-Fail Tests as Type Safety Contracts

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: CHE-0003, CHE-0005, GND-0005

## Context

Cherry-pit's type system enforces several invariants at compile time (CHE-0005): a `CommandGateway` rejects unhandled commands, an `EventStore` typed for one event cannot persist a different type, `HandleCommand<C>` requires `Command`. These guarantees are architectural — the reason for associated types over runtime routing. But they are invisible in tests unless explicitly verified. A refactoring could silently weaken a bound with no test catching it.

## Decision

Architectural type safety guarantees are verified by `trybuild`
compile-fail tests. Each test is a `.rs` file in
`tests/compile_fail/` that contains code violating one specific
invariant.

R1 [6]: Verify every architectural type safety guarantee with a
  trybuild compile-fail test
R2 [6]: Each compile-fail test file verifies exactly one type safety
  contract
R3 [6]: When a new trait bound or associated type constraint is
  introduced, add a corresponding compile-fail test

Current contracts:

| File | Invariant tested |
|------|------------------|
| `gateway_rejects_unhandled_command.rs` | `CommandGateway::create(DeleteOrder)` fails when `MyAggregate` does not implement `HandleCommand<DeleteOrder>` |
| `wrong_event_store_type.rs` | An `EventStore` typed for `OrderEvent` cannot produce `Vec<EventEnvelope<UserEvent>>` |
| `handle_non_command.rs` | `HandleCommand<NotACommand>` fails when `NotACommand` does not implement `Command` |

Test harness:

```rust
#[test]
fn compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
```

Dev dependency:

```toml
[dev-dependencies]
trybuild = { workspace = true }
```

## Consequences

- **Invariant regression detection** — if a refactoring removes a trait bound, the compile-fail test starts passing and the suite fails.
- **Living documentation** — each file in `tests/compile_fail/` documents what the type system prevents.
- **Compiler-version sensitivity** — major Rust version bumps may require updating expected `.stderr` files.
- **One invariant per file** — failures precisely identify which contract was broken.
- New safety guarantees should add compile-fail tests. They are architectural tests verifying the framework's compile-time contract with users.
