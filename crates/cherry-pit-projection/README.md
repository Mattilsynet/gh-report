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
- `InMemoryProjection<P>` — the **EPHEMERAL** backend (CHE-0048:R5, R10): no
  durable state, rebuilds from the `EventStore` on every process start. This
  is what `gh-report` and `adr-srv` ship in production today, on ephemeral
  Cloud Run filesystems where a local snapshot file would not survive a
  restart anyway.
- `PardosaProjectionStore<P>` — the **PERSISTENT** backend (CHE-0048:R1, R10):
  stores each `(aggregate_id, projection_name)` snapshot and checkpoint
  through the pardosa store facade (`.pgno` file by default). Writes the
  snapshot strictly before the checkpoint (CHE-0048:R2); a crash between the
  two leaves the snapshot present but the checkpoint absent, and restart
  code must treat that as "rebuild" rather than "trust snapshot". Durability
  (fsync/atomicity) is delegated to the pardosa backend, not to file
  temp-rename choreography.
- `FileProjectionStore<P>` — the legacy `MessagePack` file backend. Same
  snapshot-then-checkpoint semantics as `PardosaProjectionStore`, but
  superseded by it per CHE-0048:R1/R10 ("MessagePack/rmp-serde file
  snapshots are removed"); retained for the current release only and slated
  for removal alongside the `rmp-serde` dependency in a follow-up mission.
  Per CHE-0048:R10, `cherry-pit-projection` sanctions **exactly two**
  backends going forward — EPHEMERAL and PERSISTENT above — no third
  backend without a new ADR.
- `ProjectionCheckpoint` — persisted `(aggregate_id, projection_name,
  last_sequence)` record.
- `ProjectionError` / `ProjectionResult<T>` — typed corruption, infrastructure,
  and advisory-lock-contention (`StoreLocked`, retryable) failures with
  `ErrorCategory` classification. `#[non_exhaustive]` for forward compatibility.

## Ephemeral vs persistent duality (CHE-0048:R10)

Every consumer selects one of exactly two sanctioned backends per
deployment:

| | EPHEMERAL (`InMemoryProjection`) | PERSISTENT (`PardosaProjectionStore`) |
|---|---|---|
| Storage | in-process map, no durable state | pardosa store facade (`.pgno` file, or NATS) |
| Restart behaviour | rebuilds from the `EventStore` every start (O(N) replay, no snapshot shortcut) | loads the latest persisted snapshot + checkpoint, resumes from there |
| When to reach for it | ephemeral runtimes (Cloud Run) where a local snapshot would not survive restart anyway; tests | long-lived processes where O(N) replay-on-restart is too costly |
| Write ordering guarantee | not applicable (no durable state) | snapshot written strictly before checkpoint (CHE-0048:R2); checkpoint absence after a crash means "rebuild" |

## Minimal usage — ephemeral backend

```rust
use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
use cherry_pit_projection::InMemoryProjection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CounterEvent { Incremented }
impl DomainEvent for CounterEvent {
    fn event_type(&self) -> &'static str { "counter.incremented" }
}

#[derive(Default)]
struct CounterView { total: u64 }
impl Projection for CounterView {
    type Event = CounterEvent;
    fn apply(&mut self, _event: &EventEnvelope<Self::Event>) { self.total += 1; }
}

let projection = InMemoryProjection::<CounterView>::new();
assert_eq!(projection.get().total, 0);
```

## Minimal usage — persistent backend

```rust,no_run
use std::num::NonZeroU64;
use cherry_pit_core::AggregateId;
use cherry_pit_projection::PardosaProjectionStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CounterView { total: u64 }

async fn persist_and_load(path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let store = PardosaProjectionStore::<CounterView>::create_pgno(path, "counter_view")?;
    let id = AggregateId::new(NonZeroU64::new(1).unwrap());
    let four = NonZeroU64::new(4).unwrap();

    store.persist(id, &CounterView { total: 4 }, four).await?;

    let snapshot = store.load_snapshot(id).await?;
    assert_eq!(snapshot, Some(CounterView { total: 4 }));
    Ok(())
}
```


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
