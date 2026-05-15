# ADR-001: Fiber state machine as inspectable data table

**Status:** Accepted

### Context

Pardosa's fiber lifecycle defines a partial function over S × A → S where |S| = 5,
|A| = 7, yielding 35 possible pairs. Only 10 transitions are valid; the remaining
25 are rejected. The Rust ecosystem offers several state machine crates (`statig`,
`rust-fsm`, `sm`), but they encode transitions in macros or trait impls that are
opaque to tooling and visualization.

### Decision

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

### Consequences

- **Positive:** Single source of truth — runtime logic, DOT visualization, and
  test exhaustiveness checks all derive from one table.
- **Positive:** The 35-pair exhaustive test (`exhaustive_35_pairs`) validates
  that exactly 10 transitions succeed and 25 are rejected.
- **Positive:** No macro complexity. The table is plain Rust data, readable
  without understanding a DSL.
- **Negative:** Linear scan of 10 entries for lookup. O(1) with a `match`
  statement, but the table approach was chosen for inspectability. Negligible
  at N=10.
- **Negative:** A `match` statement would give compile-time exhaustiveness
  guarantees. The runtime table relies on the `no_duplicate_state_action_pairs`
  test and the `exhaustive_35_pairs` test for equivalent coverage.

### References

- `src/fiber_state.rs`: `TRANSITIONS`, `transition()`
- `src/dot.rs`: DOT visualization from `TRANSITIONS`
- pardosa.md §Fiber State Machine, §Resolved Decisions
