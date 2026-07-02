# CHE-0085. Merger/App Command-Side Choice Model

Date: 2026-07-02
Last-reviewed: 2026-07-02
Tier: B
Status: Accepted
Crates: cherry-pit-merger, cherry-pit-app

## Related

References: CHE-0069, CHE-0051, CHE-0005:R1, CHE-0014, CHE-0041, CHE-0024, CHE-0025

## Context

CHE-0069 made cherry-pit-merger a sibling of cherry-pit-app; CHE-0051 made App the explicit composition primitive. Their relationship remained implicit, so adopters could read the two command-side surfaces as layers or as evidence of a missing family adapter. CHE-0024 names CommandBus as unbuilt/planned, while CHE-0041 defers its cache until external ingestion is concrete: today, consumers own driving adapters.

## Decision

Adopters choose a command-side primitive by concurrency profile. Use merger for create-or-append-under-same-domain-key single-flight. Use App for general persist-then-publish plus policy and projection composition. They are siblings, not layers: App does not compose merger, and no family consumer depending on merger is expected state. Both paths keep commands in-process; external boundaries carry DTOs or events.

R1 [5]: cherry-pit-merger is the command-side primitive when one command may create or append under the same domain key and the lookup-create-index path must be single-flighted; its task owns the sole EventStore write handle and callers dispatch through MergerHandle

R2 [5]: cherry-pit-app is the command-side primitive when the requirement is general persist-then-publish, policy-output dispatch, projection driving, and dead-letter composition; the consumer wires concrete gateway, store, bus, and projection types through App::new

R3 [5]: merger and App are alternatives selected per aggregate or bounded-context concurrency profile; App MUST NOT depend on merger or wrap MergerHandle, and an absent family consumer of merger is expected state rather than architecture drift

R4 [5]: Command objects remain in-process on both paths; external boundaries decode DTOs or events and call consumer-owned routers or adapters, so the framework MUST NOT require Serialize, Deserialize, Clone, or Debug on Command or transport commands as commands

R5 [5]: Multi-aggregate use expands concrete per-aggregate types; no dyn or type-erased command-side registry may unify App, merger, gateway, or bus, and async stays at infrastructure or driver boundaries with RPITIT rather than async-trait or boxed futures

R6 [5]: The family ships no concrete CommandGateway/CommandBus driving adapter today; consumers own that adapter through explicit wiring, and this deferred capability is not debt for this decision; concrete external ingestion may later trigger an adapter ADR under CHE-0014, CHE-0005:R1, and CHE-0025

R7 [5]: This ADR is explanatory choice-model guidance only; it creates no scaffolding, generators, runtime surface, sealed-surface change, or new port contract, and compile-time wiring remains the primary guidance per CHE-0080

## Consequences

Positive: adopters get one choice model instead of inferring topology from CHE-0069 and CHE-0051.

Negative: the family still ships no turnkey driving adapter; explicit wiring remains adopter cost.

Open / deferred: the driving-adapter capability gap is future, separately contracted work; a concrete external-ingestion requirement can author a CommandGateway/CommandBus adapter ADR.
