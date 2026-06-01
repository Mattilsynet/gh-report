# CHE-0068. Event-Driven Projection Refresh

Date: 2026-06-01
Last-reviewed: 2026-06-01
Tier: C
Status: Accepted
Crates: gh-report, cherry-pit-projection

## Related

References: CHE-0048, CHE-0024, CHE-0054, CHE-0063

## Context

CHE-0048 fixes projection persistence and rebuild but is silent on *when* a running projection re-renders. The gh-report evidence projection currently re-renders on ad-hoc triggers, which both wastes work on bursty `RepoEvaluated` traffic and risks a stale terminal view if a render fires mid-batch. We need an idiomatic freshness model: event-driven on the meaningful update (`RepoEvaluated`), barrier-aligned on finalization (`SweepCompleted`, batch-drain), bounded in render frequency, and unchanged in correctness floor — replay still rebuilds truth per CHE-0024:R3 / CHE-0048.

## Decision

Projection freshness is driven by `RepoEvaluated`; `SweepCompleted` and batch-drain are render and broadcast barriers; intermediate render/broadcast may coalesce within a bounded staleness window (default 10s); replay remains the correctness mechanism.

R1 [7]: Within gh-report's evidence projection, each persisted-and-published `RepoEvaluated` drives projection freshness directly; the projection handler applies it on receipt without waiting for a sweep boundary, preserving CHE-0024 persist-then-publish semantics.

R2 [8]: `SweepCompleted` and batch-drain events act as render and finalization barriers; terminal render plus client broadcast occur only after the barrier event has applied, never mid-batch, preserving CHE-0054:R1.c ordering on terminal publication.

R3 [9]: Render and broadcast MAY coalesce intermediate `RepoEvaluated` arrivals within a configurable window; default maximum visible staleness is ten seconds, balancing user-perceived freshness against render and broadcast cost.

R4 [7]: Replay from `EventStore` using durable checkpoints per CHE-0024:R3 and CHE-0048 remains the projection correctness mechanism; coalescing is a render-side optimisation and never modifies persisted state or the canonical event sequence.

R5 [8]: Coalescing must never delay barrier events; `SweepCompleted` and equivalent finalization events flush any pending coalesced render before the projection handler acknowledges them, so barriers always observe the latest applied state.

## Consequences

+ becomes easier: render cost scales with sweep cardinality rather than per-repo arrivals; the post-sweep terminal view is fresh without polling; aligns with CHE-0063 partial-evidence semantics.

− becomes harder: the coalescing window is a new operator tunable; observability must distinguish in-window staleness from lagging-handler staleness; per-event render tests move to barrier-aligned assertions.

risks/migration: the 10s default is a starting heuristic — revisit on load data. No event schema, EventStore, or checkpoint change (CHE-0048:R1–R2 unaffected). Migration is internal to the projection runtime and the gh-report wiring; per-event consumers shift to barrier subscriptions or accept up-to-window staleness.
