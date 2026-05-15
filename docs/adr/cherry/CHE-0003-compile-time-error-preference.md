# CHE-0003. Compile-Time Error Preference

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: S
Status: Accepted

## Related

References: CHE-0002

## Context

Runtime errors are discovered in production; compile-time errors
during development. Cherry-pit uses trait bounds, associated types,
and the Rust type system to reject incorrect code before it runs.

This strategy is the operational arm of CHE-0002 (illegal states):
CHE-0002 says "design types that prevent invalid values"; this ADR
says "verify constraints at compile time, not runtime."

## Decision

Prefer compile-time rejection over runtime validation at every layer.

- Trait bounds over runtime type checks (`HandleCommand<C>` vs
  match-on-command-type).
- Associated types over generic parameters (fixed per instance,
  not per call) — cross-aggregate confusion rejected at compile time.
- `where` clauses to express requirements at call sites — callers
  see constraints in the signature, not in runtime panics.
- Compile-fail tests (CHE-0028) to verify type contracts don't
  regress during refactoring.

Runtime checks remain as defense-in-depth: `expected_sequence` on
`append` guards against bugs, not as primary enforcement.

R1 [2]: Prefer compile-time rejection over runtime validation at
  every layer
R2 [2]: Use trait bounds and associated types instead of runtime
  type checks to enforce constraints
R3 [2]: Add compile-fail tests for every type safety contract to
  prevent regression

## Consequences

- APIs require more type annotations and `where` clauses. Error
  messages surface during `cargo check`, not in production.
- Fewer runtime tests needed for type-level invariants — the
  compiler tests them continuously.
- Users learn framework constraints from compiler errors, not from
  documentation or runtime panics.
- At serialization boundaries (CHE-0022, CHE-0031), the compile-time/runtime boundary shifts — deserialized data requires runtime validation as primary enforcement since the compiler cannot verify wire-format correctness.
- See `docs/agent-guidance.md` for prescriptive application of this
  principle during framework customization.
