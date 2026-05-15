# CHE-0004. Event-Driven Architecture with DDD and Hexagonal Architecture

Date: 2026-04-24
Last-reviewed: 2026-04-27
Tier: S
Status: Accepted

## Related

Root: CHE-0004

## Context

Cherry-pit is a composable systems-kernel for agent-first building. Three forces are in tension: audit completeness (agents need full replayable history), domain model fidelity (command intent must be preserved separately from state mutations), and infrastructure decoupling (domain code must not know about serialization or databases). CRUD+CDC loses command intent. Bi-temporal databases preserve temporal state but lack aggregate decomposition. The chosen approach composes EDA (data model via event sourcing), DDD (consistency model via aggregates), and hexagonal architecture (integration model via ports and adapters).

## Decision

Cherry-pit combines EDA, DDD, and hexagonal architecture as
its foundational pattern. All domain logic lives behind trait-based
ports. All infrastructure lives in adapter crates. Events are the
source of truth; aggregates are the consistency boundary.

R1 [1]: Use events as the source of truth for all state, persisted as
  an immutable append-only log
R2 [1]: Place all domain logic behind trait-based ports and all
  infrastructure in adapter crates
R3 [1]: Use aggregates as the consistency boundary for all write
  operations

## Consequences

- Users must understand the event-driven mental model (commands → events → state, not direct mutation).
- Read models are eventually consistent (CQRS separation). Developers accustomed to read-after-write must adapt.
- Migrating away from event sourcing is extremely difficult once committed.
- **Steep learning curve.** EDA + DDD is one of the most demanding architectural patterns. Developers must internalize commands, events, aggregates, projections, policies, and eventual consistency before being productive.
- **Event schema is append-only forever.** Once persisted, an event type cannot be removed or renamed without migration infrastructure (CHE-0022).
