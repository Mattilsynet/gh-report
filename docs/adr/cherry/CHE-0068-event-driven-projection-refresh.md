# CHE-0068. Event-Driven Projection Refresh

Date: 2026-06-01
Last-reviewed: 2026-06-10
Tier: C
Status: Accepted
Crates: gh-report, cherry-pit-projection

## Related

References: CHE-0073, CHE-0048, CHE-0024

## Context

CHE-0048 fixes projection persistence and rebuild but is silent on *when* a running projection re-renders. CHE-0073 collapses gh-report's persisted model to `RepositoryStateCaptured` and demotes sweep lifecycle events to in-memory state, so the barrier can no longer be a durable `SweepCompleted` event. We need a freshness model driven by repository snapshots, an in-memory collect-cycle-complete barrier for terminal render and broadcast, bounded render frequency, and unchanged correctness floor — replay still rebuilds truth per CHE-0024:R3 / CHE-0048.

## Decision

Projection freshness is driven by `RepositoryStateCaptured`; the in-memory collect-cycle-complete barrier is the terminal render and broadcast barrier; intermediate render/broadcast may coalesce within a bounded staleness window; replay remains the correctness mechanism.

R1 [7]: Within gh-report's evidence projection, each persisted-and-published `RepositoryStateCaptured` drives projection freshness directly; the projection handler applies it on receipt without waiting for a collect-cycle boundary, preserving CHE-0024 persist-then-publish semantics for repository snapshots.

R2 [8]: The in-memory collect-cycle-complete barrier acts as the render and finalization barrier; terminal render plus client broadcast occur only after all repository snapshot writes for the cycle have completed and applied, never mid-cycle.

R3 [9]: Render and broadcast MAY coalesce intermediate `RepositoryStateCaptured` arrivals within a configurable window; default maximum visible staleness is one second, balancing user-perceived freshness against render and broadcast cost.

R4 [7]: Replay from `EventStore` using durable checkpoints per CHE-0024:R3 and CHE-0048 remains the projection correctness mechanism; coalescing is a render-side optimisation and never modifies persisted state or the canonical event sequence.

R5 [8]: Coalescing must never delay the collect-cycle-complete barrier; finalization flushes any pending coalesced render before completion is exposed to the server or WebSocket clients, so barriers always observe the latest applied state.

## Consequences

+ becomes easier: render cost scales with collect-cycle cardinality rather than per-repo arrivals; the terminal view is fresh without polling while the durable log stays repository-only.

− becomes harder: the barrier is no longer a replayable event; observability must distinguish in-window staleness, lagging-handler staleness, and in-memory finalization lag.

risks/migration: no checkpoint change (CHE-0048:R1–R2 unaffected). Migration is internal to gh-report wiring; persisted sweep-barrier consumers must move to the in-memory barrier or to a future analytics service.
