# CHE-0099. gh-report Ephemeral Scheduler

Date: 2026-07-23
Last-reviewed: 2026-07-23
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0077, CHE-0081, CHE-0040, CHE-0024, CHE-0048 | Supersedes: none

## Context

`gh-report`'s scheduler and sweep-timeout streams were wired through
`cherry-pit-gateway`'s `MsgpackFileStore`, giving the appearance of durable
schedule persistence. That appearance was never load-bearing: enforcement
of the sweep timeout is an in-process `tokio::select!` race
(`crates/gh-report/src/app/collect.rs:901-943`); the persisted store was
only an audit trail recovery never consulted.

This ADR corrects three false claims assuming durable, production-consumed
persistence here, and records the CHE-0081:R11 / CHE-0077:R9 opt-out
rationale:

- CHE-0098:R10 called `MsgpackFileStore` test-only once adr-srv migrated
  off it; incomplete — gh-report was also a production consumer, now
  removed here.
- `AGENTS.md:167` called gh-report msgpack-free "as of the CHE-0074 purge";
  the scheduler/sweep-timeout usage postdated that purge and is only
  removed here.
- `adr-fmt-pk4xa`'s stated scope omitted the gh-report scheduler wiring;
  this ADR corrects it.

## Decision

gh-report wires an ephemeral in-memory `EventStore` into the
`CHE-0081` scheduler driver for its sweep-timeout stream; timers are
re-armed per collection run, with no durable schedule persistence.

R1 [5]: gh-report's scheduler and sweep-timeout streams are ephemeral, in-memory, best-effort; timers re-arm each collection run, with no durable persistence, exercising the CHE-0081:R11 / CHE-0077:R9 opt-out.
R2 [5]: The opt-out is justified because sweep-timeout enforcement is an in-process `tokio::select!` race (`collect.rs:901-943`); the persisted store was only a durable audit trail, never the enforcement mechanism. A recovered stale timeout would publish to a subscriber-less `InProcessEventBus` — a no-op.
R3 [5]: A Cloud Run restart aborts the in-flight sweep, so a recovered timer would fire against an already-aborted operation; durable recovery is moot for this deployment. Pending-timer loss on restart is accepted by design.

## Consequences

+ becomes easier: gh-report's scheduler wiring matches what it actually does — no durable schedule persistence is claimed or implied, and the msgpack dependency this stream carried is removed.
− becomes harder: a stale sweep-timeout schedule does not survive a process restart; operators cannot rely on recovery to complete an in-flight sweep timeout across a Cloud Run restart.
risks/migration: none — this is a documentation-and-wiring correction. No cherry-pit-core or cherry-pit-app code changes; the DurableScheduler pattern (CHE-0081:R1-R10) remains fully available to any consumer that needs durable recovery.
