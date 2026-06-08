# PAR-0001. Fiber State Machine as Inspectable Data Table

Date: 2026-04-25
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: PAR-0001

## Context

Pardosa's fiber lifecycle defines a partial function over S × A → S where |S| = 5 states (Undefined → Defined → Active → Locked → Detached), |A| = 7 actions, yielding 35 pairs of which only 10 are valid. Invalid transitions must be rejected with specific errors. Rust state machine crates (`statig`, `rust-fsm`, `sm`) encode transitions in macros or trait impls opaque to tooling and visualization. The design question is how to encode the transition function so it remains inspectable and diagram-ready.

## Decision

Encode the complete transition table as a single `const` array:

```rust
pub const TRANSITIONS: &[(FiberState, FiberAction, FiberState)] = &[
    (FiberState::Undefined, FiberAction::Create, FiberState::Defined),
    // ... 9 more entries
];
```

A `transition()` function performs linear lookup. A `dot` module generates
DOT/Graphviz output directly from the same `TRANSITIONS` data, guaranteeing the
diagram always matches runtime behavior.

No external state machine crate is used.

R1 [5]: Encode the fiber transition function as a single const array
  of (FiberState, FiberAction, FiberState) triples
R2 [5]: Generate DOT/Graphviz output directly from the TRANSITIONS
  array so the diagram always matches runtime behavior
R3 [6]: Validate all 35 state-action pairs in the exhaustive_35_pairs
  test to confirm exactly 10 succeed and 25 are rejected

## Consequences

Single source of truth — runtime logic, DOT visualization, and exhaustive tests all derive from one table. The `exhaustive_35_pairs` test validates exactly 10 succeed and 25 are rejected. No macro complexity; the table is plain Rust data. Trade-off: linear scan instead of O(1) `match`, negligible at N=10. A `match` would give compile-time exhaustiveness guarantees; the runtime table relies on `no_duplicate_state_action_pairs` and `exhaustive_35_pairs` tests for equivalent coverage.
