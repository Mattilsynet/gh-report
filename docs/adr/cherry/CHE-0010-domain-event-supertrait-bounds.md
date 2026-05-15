# CHE-0010. DomainEvent Supertrait Bounds

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: CHE-0004

## Context

`DomainEvent` is the marker trait for all events. Its supertrait bounds constrain every event type in every cherry-pit system. Events cross process boundaries (file storage, NATS transport, Pardosa logs), requiring `Serialize + DeserializeOwned`. Events fan out to multiple consumers, requiring `Clone`. Events cross thread boundaries in async runtimes, requiring `Send + Sync + 'static`. `Debug` and `PartialEq` were considered but excluded to keep the bound minimal — users add them per-type as needed.

## Decision

```rust
pub trait DomainEvent:
    Serialize + DeserializeOwned + Clone + Send + Sync + 'static
{
    fn event_type(&self) -> &'static str;
}
```

Every bound is load-bearing:

| Bound | Required by |
|-------|-------------|
| `Serialize` | `EventStore::create`, `EventStore::append`, `EventBus::publish`, Pardosa logs |
| `DeserializeOwned` | `EventStore::load`, Pardosa consumer, NATS subscriber |
| `Clone` | `EventBus::publish` fan-out, `EventEnvelope` derives `Clone` |
| `Send` | Async task spawning, cross-thread event delivery |
| `Sync` | Shared references to events across threads |
| `'static` | Storage in `Vec`, `Box`, and async futures |

`event_type() -> &'static str` is a stable string discriminator used
for routing, schema registry, and deserialization dispatch. It must
never change once events of this type exist in a log.

R1 [4]: DomainEvent requires Serialize + DeserializeOwned + Clone +
  Send + Sync + 'static as supertrait bounds
R2 [4]: event_type() returns a &'static str that must never change
  once events of that type exist in a log
R3 [4]: Every supertrait bound must be load-bearing with a concrete
  infrastructure consumer that requires it

## Consequences

- Every event type must derive `Serialize`, `Deserialize`, and `Clone` — the entry cost of using cherry-pit.
- `DeserializeOwned` means deserialization produces owned values. Zero-copy deserialization is not possible, simplifying the type system at the cost of one allocation per event.
- No `Debug` bound means the framework cannot log events by default.
- No `PartialEq` bound means test assertions require user-derived `PartialEq` or field-by-field comparison.
- The `event_type()` string must be stable forever — renaming breaks deserialization of historical data.
- Contrast with `Command` (CHE-0014): commands have minimal bounds because they stay in-process by default.
