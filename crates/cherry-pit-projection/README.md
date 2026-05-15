# cherry-pit-projection

Projection drivers and storage backends for cherry-pit read models.

This crate realises CHE-0048: it drives `cherry_pit_core::Projection` from a
typed `EventStore`, without redefining the core projection trait and without
dynamic projection dispatch.

## Role

`cherry-pit-projection` is the read-side adapter layer. It folds durable event
streams into query-optimized projection state and persists checkpointed snapshots
for restart/rebuild workflows. It is a sibling adapter crate to
`cherry-pit-gateway`; it depends on `cherry-pit-core` and does not stack on the
gateway implementation.

## Public API summary

- `ProjectionDriver<P, S>` — generic driver for one `P: Projection` and one
  typed `S: EventStore<Event = P::Event>`.
- `InMemoryProjection<P>` — ephemeral/test backend with no durable state and no
  dynamic projection dispatch.
- `FileProjectionStore<P>` — MessagePack snapshot + checkpoint backend. It writes
  a snapshot file first, then writes the sibling checkpoint file. A crash in
  the window between the two leaves the snapshot present but the checkpoint
  absent — restart code must treat that case as "rebuild" rather than
  "trust snapshot" (CHE-0048:R2). Mutating operations are fenced by an
  advisory `.lock` file in the store directory (CHE-0043:R1–R3) and sweep
  orphaned `*.tmp` files from prior crashed writes before each persist
  (CHE-0047:R1).
- `ProjectionCheckpoint` — persisted `(aggregate_id, projection_name,
  last_sequence)` record.
- `ProjectionError` / `ProjectionResult<T>` — typed corruption, infrastructure,
  and advisory-lock-contention (`StoreLocked`, retryable) failures with
  `ErrorCategory` classification. `#[non_exhaustive]` for forward compatibility.

## Minimal usage

```rust,no_run
use cherry_pit_core::{AggregateId, DomainEvent, EventEnvelope, EventStore, Projection};
use cherry_pit_projection::{FileProjectionStore, ProjectionDriver};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent { Incremented }
impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str { "counter.incremented" }
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct CounterView { total: u64 }
impl Projection for CounterView {
    type Event = CounterEvent;
    fn apply(&mut self, _event: &EventEnvelope<Self::Event>) { self.total += 1; }
}

async fn rebuild<S>(store: S, id: AggregateId) -> Result<CounterView, Box<dyn std::error::Error>>
where
    S: EventStore<Event = CounterEvent>,
{
    let driver = ProjectionDriver::<CounterView, _>::new(store);
    let files = FileProjectionStore::<CounterView>::new("projection-store", "counter-view");
    let correlation = cherry_pit_core::CorrelationContext::none();
    Ok(driver.rebuild_file(id, &correlation, &files).await?)
}
```
