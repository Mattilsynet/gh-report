# CHE-0037. No Snapshot Support (Deliberate Deferral)

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0009, CHE-0012

## Context

Event-sourced systems reconstruct aggregate state by replaying all events. Snapshots mitigate growing replay time by persisting materialized state. Adding snapshot support now would require aggregates to be snapshot-serializable, stores to manage snapshots alongside events, and the bus to load snapshot + remaining events. All deferral conditions hold: aggregates are short-lived, event streams small, system is single-writer with local storage.

## Decision

No snapshot support. Full event replay for every aggregate
reconstruction. Deliberate deferral, not oversight.

R1 [11]: Full event replay for every aggregate reconstruction with no
  snapshot support
R2 [11]: Aggregates do not need Serialize or Deserialize bounds on
  their state type

Snapshots are not needed when:

- Aggregates are short-lived (bounded event count)
- Event streams are small (< 1,000 events per aggregate)
- The system is single-writer with local storage (low replay latency)

All three conditions hold for cherry-pit's current deployment model
(single-writer, file-based storage, development/small-scale).

## Consequences

- **Performance ceiling** — reconstruction is O(n) where n = total events.
- **No snapshot serialization** — aggregates need only `Default + Send + Sync`, not `Serialize`/`Deserialize`.
- **Simpler traits** — no `load_from_sequence`, `save_snapshot`, or staleness handling.
- **Full history always available** — debugging replays exact event sequence.
- Revisit when aggregates exceed 10,000 events or dispatch latency exceeds SLA.
