# CHE-0010. DomainEvent Supertrait Bounds

Date: 2026-04-25
Last-reviewed: 2026-05-18
Tier: A
Status: Accepted

## Related

References: CHE-0004, CHE-0064, CHE-0065

## Context

`DomainEvent` is the marker trait for all events. Its supertrait bounds constrain every event type in every cherry-pit system. Events fan out to multiple consumers, requiring `Clone`. Events cross thread boundaries in async runtimes, requiring `Send + Sync + 'static`. Events must be canonically encodable for substrate-side hash chaining (CHE-0064), satisfied by `pardosa_encoding::Encode`. `Debug` and `PartialEq` were considered but excluded to keep the bound minimal — users add them per-type as needed.

Historically the trait also required `Serialize + DeserializeOwned`. Under CHE-0065:R1 the on-disk wire format moved to `pardosa_genome::to_vec` / `from_bytes` — concrete free functions, not serde traits — so the serde pair lost its load-bearing consumer set and was retired from the supertrait per the 2026-05-18 amendment below.

## Decision

```rust
pub trait DomainEvent:
    Clone + Send + Sync + 'static + pardosa_encoding::Encode
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
| `pardosa_encoding::Encode` | Substrate-side hash chaining per CHE-0064:R2 / R4 (added in CHE-0064) |

`event_type() -> &'static str` is a stable string discriminator used
for routing, schema registry, and dispatch. It must never change once
events of this type exist in a log.

R1 [4]: DomainEvent requires Clone + Send + Sync + 'static +
  pardosa_encoding::Encode as supertrait bounds (the
  pardosa_encoding::Encode contribution originates in CHE-0064:R2;
  the Serialize + DeserializeOwned pair previously required here
  was dropped 2026-05-18 — see Consequences §)
R2 [4]: event_type() returns a &'static str that must never change
  once events of that type exist in a log
R3 [4]: Every supertrait bound must be load-bearing with a concrete
  infrastructure consumer that requires it

## Consequences

- Every event type must derive (or hand-roll) `Clone` and `pardosa_encoding::Encode`, and implement `event_type()` — the entry cost of using cherry-pit. `Encode` is auto-emitted for `#[derive(GenomeSafe)]` payloads per GEN-0037:R4.
- The trait no longer requires `Serialize + DeserializeOwned`. Implementers MAY still derive serde traits voluntarily (gh-report does so for compatibility); the cherry-pit-core surface simply no longer demands them. This reconciles staleness vs CHE-0065:R1: under pardosa-genome end-to-end (`to_vec` / `from_bytes`) the load-bearing consumers for the serde pair (event-store reads/writes, Pardosa logs, NATS subscriber) have migrated to pardosa-genome's free-function entry points, so the bound failed CHE-0010:R3.
- No `Debug` bound means the framework cannot log events by default.
- No `PartialEq` bound means test assertions require user-derived `PartialEq` or field-by-field comparison.
- The `event_type()` string must be stable forever — renaming breaks dispatch of historical data.
- Contrast with `Command` (CHE-0014): commands have minimal bounds because they stay in-process by default.
