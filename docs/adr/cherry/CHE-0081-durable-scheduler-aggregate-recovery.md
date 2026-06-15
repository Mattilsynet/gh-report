# CHE-0081. Durable Scheduler Aggregate Recovery

Date: 2026-06-15
Last-reviewed: 2026-06-15
Tier: B
Status: Accepted
Crates: cherry-pit-core, cherry-pit-app, gh-report

## Related

References: CHE-0077, CHE-0040, CHE-0024, CHE-0018, CHE-0048, CHE-0006, CHE-0051, CHE-0041, CHE-0005, CHE-0025, CHE-0029, CHE-0057, CHE-0022, CHE-0070, PGN-0016

## Context

CHE-0077 ratifies explicit scheduled domain events but leaves implementation choices open: pending-state storage, crash recovery after a fire decision, stream identity, retention, and embedded payload semantics. Oracle ruled the scheduler aggregate admissible and Case-B no-loss mandatory.

## Decision

Persist schedules as a singleton scheduler aggregate in the existing EventStore. Firing records `ScheduleFired` first, carrying the caller event transport, then drives the caller event through the normal persist-then-publish path. Recovery replays, reconstructs pending, and completes half-done fires idempotently.

R1 [5]: Represent schedules as a dedicated scheduler aggregate; persist `ScheduleArmed`, `ScheduleFired`, and `ScheduleCancelled` as a separate scheduler event type in the existing EventStore.

R2 [5]: Recover pending schedules by replaying the scheduler stream; `ScheduleArmed` unmatched by `ScheduleFired` or `ScheduleCancelled` remains pending, and due-already pending schedules fire during recovery.

R3 [5]: Allocate one singleton scheduler aggregate stream per application writer; PGN-0016 per-subject fencing then protects one decisive scheduler subject instead of many caller-key schedule streams.

R4 [5]: On due fire, append `ScheduleFired` to the scheduler stream before appending the caller event through the normal CHE-0024 persist-then-publish path.

R5 [5]: Require `ScheduleFired` to carry caller event identity, opaque payload, target aggregate, and explicit `CorrelationContext` so recovery can complete a half-done fire.

R6 [5]: Treat Case-A silent loss as forbidden; CHE-0024's non-fatal publish rule protects stored events only, so unstored caller events must be recovered or dead-lettered.

R7 [5]: Complete recovery idempotently: if the caller event identity is absent in the target stream, append and publish it; if present, rely on replay and dedup.

R8 [5]: Treat embedded caller payloads as opaque transport, not scheduler-owned computed aggregates or projections, preserving CHE-0022's own-scope event-payload rule.

R9 [5]: Retain terminal scheduler events in v0.1; pruning, compaction, retention windows, or terminal-schedule garbage collection require a later ADR.

R10 [5]: Place scheduler events, trait, and fold in `cherry-pit-core`; place the async driver and dead-letter wiring in `cherry-pit-app`, statically wired and never pardosa-sited.

## Consequences

+ becomes easier: recovery reuses event-sourced replay, and crash-mid-fire completion is derived from stored scheduler facts rather than process timers.

− becomes harder: one scheduler stream serializes all fire decisions and retains terminal history until a future compaction ADR exists.

risks/migration: no pluggable coordinator is introduced. The next Rust mission must keep scheduler events as a separate event type, not a `gh-report` domain-event enum variant, so existing schema hashes stay stable.
