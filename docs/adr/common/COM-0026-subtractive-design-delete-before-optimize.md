# COM-0026. Subtractive Design — Delete Before Optimize

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: S
Status: Accepted
Parent-cross-domain: GND-0002 — delete-before-optimize is the COM-tier expression of GND-0002's universal directive that directives express intent rather than mechanism, by removing mechanism that does not serve intent

## Related

References: GND-0002, COM-0001, COM-0013

## Context

COM-0001 frames complexity as a budget; COM-0013 prefers evolution
over speculation. Neither codifies the prior step: aggressively
removing parts, code, dependencies, requirements, and process steps
*before* refining what remains. Saint-Exupéry's "perfection when
nothing is left to take away" and Musk's five-step algorithm both
sequence subtraction ahead of optimization. Optimizing a part that
should not exist scales the wrong thing. Cherry-pit's CI, lint,
ADR, and dependency surfaces all accumulate inertia by default.

Three options evaluated:

1. **Implicit subtraction** — trust reviewers to push back. Loses to
   accumulation pressure; existing parts inherit legitimacy.
2. **Periodic prune sweeps** — schedule deletion campaigns. Reactive,
   episodic, doesn't shape day-to-day decisions.
3. **Subtractive default** — make "delete" the first design move on
   every change, codified as a rule. Consistent, cheap, compounding.

Option 3 chosen: subtraction as a phase-ordered discipline integrates
with strategic investment (COM-0001) and turns deletion from heroic
into routine.

## Decision

Order every design change as: question, delete, simplify, accelerate,
automate. Skipping ahead — for example, optimizing a step that
should have been deleted — wastes the budget and entrenches
accidental complexity.

R1 [3]: Treat removal as the first move on any code, dependency,
  ADR rule, CI step, or feature; spend at least one design pass
  attempting deletion before adding alternatives
R2 [3]: Reverse-justify every retained requirement by naming the
  failure mode its absence would cause; unjustified requirements
  are deleted, not refined
R3 [3]: Order changes as question, delete, simplify, accelerate,
  automate; later phases require earlier phases to have been
  attempted in the same change set
R4 [3]: When a deletion attempt restores fewer than ten percent of
  the removed lines or steps, deletion was insufficiently
  aggressive — repeat the pass before optimizing

## Consequences

- **Pairs with COM-0001.** Strategic investment funds the deletion
  pass; without that budget, subtractive design is skipped under
  pressure.
- **Front-loads cost.** Deletion attempts that fail still consume
  time. The wager is that prevented future complexity dominates.
- **Tension with COM-0013.** Evolutionary design adds when needed;
  subtractive design removes when possible. Resolution: add only
  what survives a deletion attempt.
- **Tooling implication.** Lint and CI step lists become first-class
  deletion candidates. No step is grandfathered in.
