# COM-0029. Behavioral Simplification — Explicit State Models Over Ad-Hoc Control Flow

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: COM-0017, COM-0001, CHE-0002

## Context

CHE-0002 makes illegal *states* unrepresentable — structural. It
does not constrain how behavior moves between legal states. Long-
lived components (aggregates, sagas, retry loops) grow implicit
state machines via nested booleans, optionals, and scattered guards.
Flat FSMs explode; HFSMs, behavior trees, strategy, and command
patterns each compress different shapes.

Three options:

1. **Ad-hoc control flow** — every component reinvents transitions.
2. **One model everywhere** — forces behavior trees on pipelines.
3. **Explicit, shape-matched model** — name the model and pick the
   simplest fitting the behavior.

Option 3 chosen: explicit models compress behavior the way types
compress structure.

## Decision

Components whose behavior depends on prior events must declare an
explicit behavioral model — finite state machine, hierarchical state
machine, behavior tree, strategy, command, or pipeline — and
implement transitions through that model rather than ad-hoc guards.

R1 [5]: Represent any component with two or more behavioral phases
  as an explicit state enum or sum type; transitions occur only
  through methods that consume the prior state and return the next
R2 [5]: When a flat state set exceeds roughly seven cases or shares
  transitions across many states, lift shared transitions into a
  parent state using a hierarchical state machine encoding
R3 [6]: For decision-making components with composable subtasks —
  retry, fallback, sequence — use a behavior tree node algebra
  rather than nested conditionals so subtrees remain independently
  testable
R4 [5]: Model swappable algorithm families as a strategy trait with
  one implementation per variant; collapse the variant choice to a
  single construction site rather than per-call branching
R5 [5]: Reify externally-triggered actions as command values stored
  before execution so retry, undo, audit, and replay derive from
  the same source

## Consequences

- **Pairs with CHE-0002.** Structural MISU and behavioral state
  models are complements: one constrains shape, the other
  constrains motion. Together they bound legal trajectories.
- **Pairs with COM-0017.** Explicit transitions become testable and
  often statically checkable via typestate, raising mechanization.
- **Tension with COM-0002.** Behavior trees and HFSMs add nodes
  and types that a deep module would consolidate. Resolution:
  apply behavioral models when ad-hoc flow exceeds the cognitive
  budget of the surrounding module.
- **Cost.** Migrating existing implicit state machines is invasive
  and should follow COM-0026 — delete dead branches first.
