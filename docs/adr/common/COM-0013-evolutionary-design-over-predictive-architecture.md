# COM-0013. Evolutionary Design Over Predictive Architecture

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0001, GND-0001

## Context

Ford, Parsons, and Kua (Building Evolutionary Architectures) define evolutionary architecture as supporting "guided, incremental change across multiple dimensions." Erder, Pureur, and Woods add: "Delay design decisions until absolutely necessary." The cost of wrong predictions compounds — abstractions built for futures that never arrive become permanent maintenance burden. Predictive anti-patterns: premature generalization, speculative infrastructure, over-abstraction.

Cherry-pit practices this through deliberate deferrals with trigger conditions: CHE-0037 (no snapshots), CHE-0040 (no sagas), GEN-0031 (Rust-only), GEN-0010 (std-only). Each makes evolution guided rather than ad hoc.

## Decision

Design for the requirements that exist now. Defer design for
requirements that may exist later. Document the deferral and the
conditions that would trigger revisiting the decision.

R1 [5]: Every abstraction, generic parameter, and infrastructure
  component must be justified by a current concrete requirement
R2 [5]: When a future need is anticipated but not yet required,
  write a deliberate-deferral ADR documenting the need, why current
  constraints do not justify building it, and the trigger condition
R3 [6]: Design decisions should minimize the cost of future change
  through deep modules, information hiding, and the dependency rule
R4 [5]: When two designs are comparable, choose the one easier to
  reverse or evolve; reversibility reduces the cost of being wrong
R5 [6]: Deliberate-deferral ADRs are reviewed periodically against
  their trigger conditions when scale or requirements change

## Consequences

Deliberate-deferral ADRs are a recognized category — first-class architecture decisions that say "not yet." The complexity budget (COM-0001) is preserved since speculative infrastructure is not built. New proposals requiring speculative infrastructure can be challenged with COM-0013. The trigger-condition pattern prevents deferrals from becoming forgotten decisions — each is actionable when its condition is met. Evolutionary design is not tactical programming (COM-0001); the strategic investment is in making the current design evolvable. In distributed systems, evolutionary design is especially critical because wrong distributed abstractions (premature consensus protocols, speculative sharding) impose coordination costs that cannot be removed without breaking wire compatibility.
