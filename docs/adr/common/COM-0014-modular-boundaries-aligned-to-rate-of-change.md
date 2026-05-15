# COM-0014. Modular Boundaries Aligned to Rate of Change

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0012

## Context

Evans (DDD, Ch. 14) introduces bounded contexts for model complexity. Skelton and Pais (Team Topologies, Ch. 6) extend this: boundaries should reflect rate of change. Kaiser (Architecture for Flow) synthesizes via Wardley Mapping — module boundaries should separate components with different change rates so evolution in one area does not force coordinated changes in another.

Cherry-pit's domain taxonomy applies this: Common (COM) changes rarely, Cherry (CHE) occasionally, Pardosa (PAR) with deployment topology, Genome (GEN) with serialization requirements. Each has distinct rate of change, audience, and abstraction level. The crate DAG (CHE-0029) physically enforces these boundaries.

## Decision

Module, crate, and domain boundaries should align with rate-of-change
boundaries. Components that change for the same reason belong
together; components that change independently should be separable.

R1 [5]: Group concepts by reason for change; if two concepts always
  change together they belong in the same module
R2 [5]: Separate by audience — different audiences imply different
  rates of change and different review needs
R3 [6]: Components that must be deployable independently require
  separate crates; co-deployed components use modules within a crate
R4 [5]: The interface between components with different rates of
  change must be stable; the slow-changing component defines the
  interface
R5 [6]: ADRs spanning two domains at equal weight signal a boundary
  misalignment requiring adjustment or splitting

## Consequences

The four-domain taxonomy is validated as rate-of-change alignment, not arbitrary grouping. The crate DAG (CHE-0029) physically enforces boundaries aligned to change rates. New modules are evaluated: "Does this change at the same rate as its crate?" Cross-domain ADRs are flagged as potential boundary misalignments, resolved by adjusting boundaries or splitting with cross-references (COM-0038). In distributed deployments, rate-of-change boundaries also become deployment unit boundaries — components that change independently must be independently deployable without coordinated rollouts. Tension with COM-0002 (deep modules) is resolved: consolidate within a rate-of-change boundary; separate across boundaries.
