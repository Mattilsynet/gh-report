# CHE-0068. Event-Driven Projection Refresh

Date: 2026-06-01
Last-reviewed: 2026-09-03
Tier: C
Status: Accepted
Crates: gh-report, cherry-pit-projection

## Related

References: CHE-0073, CHE-0048, CHE-0024

## Context

CHE-0048 fixes projection persistence and rebuild but is silent on *when* a running projection re-renders. CHE-0073 collapses gh-report's persisted model to `RepositoryStateCaptured` and demotes sweep lifecycle events to in-memory state, so the barrier can no longer be a durable `SweepCompleted` event. We need a freshness model driven by repository snapshots, an in-memory collect-cycle-complete barrier for terminal render and broadcast, bounded render frequency, and unchanged correctness floor — replay still rebuilds truth per CHE-0024:R3 / CHE-0048.

R3 amended 2026-09-03, one second to ten, trailing-edge debounce to leading-edge plus hold-down; R1, R2, R4, R5 reaffirmed unchanged (COM-0034:R4). The old window was anchored to the first signal of a burst, so it both delayed an idle system and re-rendered once per second under sustained inflow.

Cost curve (FLO-0012:R1). Raising the hold-down lowers render and broadcast cost — steady state is one render per window plus one render duration — and raises intermediate visible staleness, since a signal arriving just after a render waits the full window. Lowering it inverts both: fresher intermediate views, render cost approaching one per arrival, and at the limit a continuous loop rendering output that is already superseded. Ten seconds keeps intermediate staleness far below the one-hour collect cadence governing the terminal view while bounding render cost to a small constant per cycle. Presumed wrong until re-measured under load (FLO-0012:R3); single-sourced as `gh_report::config::PARTIAL_RENDER_HOLD_DOWN` (COM-0027:R1).

Variance absorber (FLO-0002:R1). Leading-edge firing is aperiodic, so cadence cannot absorb arrival variance. The absorber is the hold-down timer plus the dirty flag: coalescing coerces any number of in-window arrivals to one bit, and the timer converts an unbounded arrival rate into a bounded render rate. The flag never sheds a signal, so absorption costs latency, never information — hence a hold-down rather than a sampling window. Harmonic with the base cadence (FLO-0002:R2): 3600 / 10 = 360.

FLO-0009:R1 reconciliation (CHE-0102 precedent). It does not literally bind: this is render-side, has no queue-fill input, and admits or rejects nothing. Its spirit is satisfied rather than ignored — the regulator has no cliff, because the leading edge never delays the first signal after idle and the output rate degrades smoothly to the hold-down asymptote. A gradient response would need a queue-fill signal this path lacks.

Barrier interaction (R5) is load-bearing at ten seconds where it was incidental at one: the pre-amendment implementation dropped a pending render at the barrier outright, a fault the wider window would have made ten times more likely. The hold-down is pre-empted by the barrier, never waited out at it.

## Decision

Projection freshness is driven by `RepositoryStateCaptured`; the in-memory collect-cycle-complete barrier is the terminal render and broadcast barrier; intermediate render/broadcast may coalesce within a bounded staleness window; replay remains the correctness mechanism.

R1 [7]: Within gh-report's evidence projection, each persisted-and-published `RepositoryStateCaptured` drives projection freshness directly; the projection handler applies it on receipt without waiting for a collect-cycle boundary, preserving CHE-0024 persist-then-publish semantics for repository snapshots.

R2 [8]: The in-memory collect-cycle-complete barrier acts as the render and finalization barrier; terminal render plus client broadcast occur only after all repository snapshot writes for the cycle have completed and applied, never mid-cycle.

R3 [9]: Render and broadcast MAY coalesce intermediate `RepositoryStateCaptured` arrivals within a configurable window; the intermediate partial publisher renders on the leading edge and then holds down for a default of ten seconds measured from render completion, coalescing every signal arriving during the hold-down into exactly one follow-up render, balancing user-perceived freshness against render and broadcast cost.

R4 [7]: Replay from `EventStore` using durable checkpoints per CHE-0024:R3 and CHE-0048 remains the projection correctness mechanism; coalescing is a render-side optimisation and never modifies persisted state or the canonical event sequence.

R5 [8]: Coalescing must never delay the collect-cycle-complete barrier; finalization flushes any pending coalesced render before completion is exposed to the server or WebSocket clients, so barriers always observe the latest applied state.

## Consequences

+ becomes easier: render cost scales with collect-cycle cardinality rather than per-repo arrivals; the terminal view is fresh without polling while the durable log stays repository-only.

− becomes harder: the barrier is no longer a replayable event; observability must distinguish in-window staleness, lagging-handler staleness, and in-memory finalization lag.

risks/migration: no checkpoint change (CHE-0048:R1–R2 unaffected). Migration is internal to gh-report wiring; persisted sweep-barrier consumers must move to the in-memory barrier or to a future analytics service.
