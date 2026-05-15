# CHE-0002. Make Illegal States Unrepresentable

Date: 2026-04-25
Last-reviewed: 2026-04-27
Tier: S
Status: Accepted

## Related

References: CHE-0001

## Context

P1 (CHE-0001) mandates correctness first. Runtime guards (`assert!`, `if`-checks) run on every invocation, can be bypassed, and scale O(call sites). Type-level encoding (e.g., `AggregateId(NonZeroU64)`) enforces invariants once at construction; every subsequent use inherits the guarantee at zero cost, scaling O(1). As the codebase grows, runtime guards increasingly miss paths while type-level invariants cannot be circumvented. This principle informs `AggregateId(NonZeroU64)` (CHE-0011), associated types (CHE-0005), exhaustive event enums (CHE-0022), and infallible apply (CHE-0009).

## Decision

Every cherry-pit type must encode its invariants at the type level.

- Newtypes prevent primitive confusion (`AggregateId` vs raw `u64`).
- `NonZero*` types eliminate zero holes without runtime checks.
- Associated types fix relationships at compile time, not per-call.
- Exhaustive enums force handling of every variant — no `_` wildcards
  on domain types.
- Validated constructors return `Result`, preventing invalid instances.

Runtime guards (e.g., `expected_sequence` on `append`) are
defense-in-depth, not primary enforcement.

R1 [2]: Encode every type invariant at the type level so the compiler
  rejects invalid values at construction time
R2 [2]: Use newtypes to prevent primitive type confusion across
  domain boundaries
R3 [2]: Use validated constructors returning Result to prevent
  invalid instances from existing
R4 [2]: Reserve runtime guards for defense-in-depth only, never as
  primary enforcement

## Consequences

- Higher type complexity — more newtypes, associated types, and
  `where` clauses than a permissive design.
- More verbose APIs at definition sites. Dramatically fewer runtime
  failure modes at call sites.
- Users defining aggregates, events, and commands inherit this
  discipline — the framework's trait bounds enforce it transitively.
- At serialization boundaries (CHE-0022, CHE-0031), external data re-introduces the possibility of invalid states — validated constructors and `Deserialize` implementations become the primary enforcement, not just defense-in-depth.
- See `docs/agent-guidance.md` for prescriptive application of this
  principle during framework customization.
