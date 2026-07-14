# CHE-0010. DomainEvent Supertrait Bounds

Date: 2026-04-25
Last-reviewed: 2026-06-10
Tier: A
Status: Accepted

## Related

References: CHE-0004, CHE-0074

## Context

`DomainEvent` is the marker trait for all events. Its supertrait bounds constrain every event type in every cherry-pit system. Events fan out to multiple consumers, requiring `Clone`. Events cross thread boundaries in async runtimes, requiring `Send + Sync + 'static`. The active pardosa adapter (CHE-0071) stores serde-encoded `EventEnvelope<E>` bytes inside a `GenomeSafe` wrapper, so `Serialize + DeserializeOwned` remain load-bearing on the public trait. `Debug` and `PartialEq` stay excluded; users add them per type as needed.

## Decision

```rust
pub trait DomainEvent:
    Clone + Send + Sync + 'static + serde::Serialize + serde::de::DeserializeOwned
{
    fn event_type(&self) -> &'static str;
}
```

Every bound is load-bearing:

| Bound | Required by |
|-------|-------------|
| `Clone` | `EventBus::publish` fan-out, `EventEnvelope` derives `Clone` |
| `Send` | Async task spawning, cross-thread event delivery |
| `Sync` | Shared references to events across threads |
| `'static` | Storage in `Vec`, `Box`, and async futures |
| `Serialize` | Adapter-side envelope encoding through rmp-serde (CHE-0071) |
| `DeserializeOwned` | Adapter-side envelope replay through rmp-serde (CHE-0071) |

`event_type() -> &'static str` is a stable string discriminator used
for routing, schema registry, and dispatch. It must never change once
events of this type exist in a log.

R1 [4]: DomainEvent requires Clone + Send + Sync + 'static +
  serde::Serialize + serde::de::DeserializeOwned as supertrait bounds;
  the serde pair is load-bearing for CHE-0071's opaque envelope encoding
R2 [4]: event_type() returns a &'static str that must never change
  once events of that type exist in a log
R3 [4]: Every supertrait bound must be load-bearing with a concrete
  infrastructure consumer that requires it

## Consequences

- Every event type must derive or implement serde Serialize and DeserializeOwned and implement `event_type()` — the entry cost of using cherry-pit.
- The trait does not require native pardosa `GenomeSafe`; CHE-0071 keeps genome-safety at the adapter wrapper boundary and leaves domain events on the serde model.
- No `Debug` bound means the framework cannot log events by default.
- No `PartialEq` bound means test assertions require user-derived `PartialEq` or field-by-field comparison.
- The `event_type()` string must be stable forever — renaming breaks dispatch of historical data.
- Contrast with `Command` (CHE-0014): commands have minimal bounds because they stay in-process by default.
