# CHE-0008. Pure Command Handling

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: CHE-0004

## Context

In event-sourced systems, command handling is the decision point:
inspect current state, validate invariants, produce events. The purity
of this function determines testability, determinism, and the ability
to reason about aggregate behavior.

Two design approaches:

1. **Mutable handler** — `handle(&mut self, cmd: C)` mutates state
   directly. Events are a side product or afterthought. Testing
   requires inspecting internal state.
2. **Pure handler** — `handle(&self, cmd: C) -> Result<Vec<Event>,
   Error>`. The handler inspects state (shared reference) and returns
   events. State changes happen only when events are applied via
   `apply`. Testing is pure function testing: given state + command,
   assert events or error.

## Decision

`HandleCommand::handle(&self, cmd: C) -> Result<Vec<Self::Event>,
Self::Error>`.

Three constraints enforce the pattern:

1. **`&self` receiver** — shared reference prevents direct mutation
   of aggregate fields. State changes happen exclusively through
   events returned by `handle`, then applied via `apply`.
2. **Command consumed by value** (`cmd: C`, not `&C`) — a command
   represents one-time intent. After handling, it is consumed. No
   cloning, no re-use without explicit reconstruction.
3. **Pure return type** — `Result<Vec<Event>, Error>` is plain data.
   No futures, no side effects in the type signature.

Documentation mandates: "Must be pure — no I/O, no side effects."

R1 [4]: HandleCommand::handle takes &self (shared reference) to
  prevent direct mutation of aggregate state
R2 [4]: Commands are consumed by value to enforce one-time intent
  semantics
R3 [4]: handle returns Result<Vec<Event>, Error> as plain data with
  no futures or side effects in the type signature

## Consequences

- Command handlers are trivially testable: construct aggregate, apply setup events, call `handle`, assert events or error. No mocks.
- `&self` + `apply` creates two-phase update: `handle` decides, `apply` mutates — the canonical event-sourcing pattern.
- **Purity is convention, not compiler-enforced.** `&self` prevents mutation but nothing prevents I/O, global state, or non-deterministic calls. Enforcement relies on code review.
- No `Clone` on commands (CHE-0014) means the framework cannot retry — callers must reconstruct.
- Zero events returned (`Ok(vec![])`) means idempotent acceptance: no persistence, no publication.
- Pure handlers enable deterministic replay — given the same aggregate state and the same command, the same events are produced, which is critical for testing and diagnosing issues in distributed deployments.
