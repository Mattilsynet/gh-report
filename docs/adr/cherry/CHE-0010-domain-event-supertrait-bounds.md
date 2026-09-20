# CHE-0010. DomainEvent Supertrait Bounds

Date: 2026-04-25
Last-reviewed: 2026-09-20
Tier: A
Status: Accepted

## Related

References: CHE-0004, CHE-0045, CHE-0074, CHE-0100

## Context

`DomainEvent` is the marker trait for all events. Its supertrait bounds constrain every event type in every cherry-pit system. Events fan out to multiple consumers, requiring `Clone`. Events cross thread boundaries in async runtimes, requiring `Send + Sync + 'static`. CHE-0074 retired the opaque-byte pardosa adapter (formerly CHE-0071): gh-report now persists through a native pardosa store port that does not round-trip `DomainEvent` through serde. The last trait-level consumer of `Serialize + DeserializeOwned` was `cherry-pit-gateway`'s `rmp-serde` encoding; CHE-0100 retired that store, so under R3 the serde pair is no longer load-bearing and is dropped. Serialization is declared by the consumers that need it — `EventEnvelope`'s `#[serde(bound(...))]` impls and `cherry-pit-web`'s router bound on `Aggregate::Event` — the per-crate boundary CHE-0045 R1-R2 prescribes. `Debug` and `PartialEq` stay excluded; users add them per type as needed.

## Decision

```rust
pub trait DomainEvent: Clone + Send + Sync + 'static {
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

`event_type() -> &'static str` is a stable string discriminator used
for routing, schema registry, and dispatch. It must never change once
events of this type exist in a log.

R1 [4]: DomainEvent requires Clone + Send + Sync + 'static as
  supertrait bounds, and must not require serde: serialization is
  declared by each serializing consumer (CHE-0045 R1-R2)
R2 [4]: event_type() returns a &'static str that must never change
  once events of that type exist in a log
R3 [4]: Every supertrait bound must be load-bearing with a concrete
  infrastructure consumer that requires it; a bound whose last consumer
  is retired is removed, not retained for convenience

## Consequences

- Every event type must implement `event_type()` and be `Clone`. An event that is never serialized needs no serde impls; `tests/compile_pass/non_serde_domain_event.rs` pins this.
- An event reaching a serializing consumer must implement the required serde traits, by derive or hand-written impl; the requirement moves to the use site, and the error surfaces there.
- The trait does not require native pardosa `GenomeSafe` (CHE-0074). No persisted shape changes: `EventEnvelope`'s serde impls are unchanged.
- No `Debug` bound means the framework cannot log events by default.
- No `PartialEq` bound means test assertions require user-derived `PartialEq` or field-by-field comparison.
- The `event_type()` string must be stable forever — renaming breaks dispatch of historical data.
- Contrast with `Command` (CHE-0014): commands have minimal bounds because they stay in-process by default.
