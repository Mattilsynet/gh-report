# COM-0007. Information Hiding — Minimize Leakage

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0002

## Context

Ousterhout (Ch. 5) refines COM-0002 from *how much* to hide to *what specifically* to hide. Information hiding means each module encapsulates its design decisions so other modules cannot depend on them. Leakage — when a decision is reflected in multiple modules — creates hidden coupling requiring coordinated changes. Leakage forms include interface leakage through signatures, temporal decomposition sharing intermediate state, and back-channel leakage through documentation.

Cherry-pit demonstrates this: `EventEnvelope` fields are `pub(crate)` with method-based access, MsgPack format is invisible to trait users, and file layout is hidden behind `EventStore`.

## Decision

Design decisions should be encapsulated within the module that owns
them. No other module should need to know — or be able to depend
on — the decision.

R1 [5]: Before implementing a module, list its design decisions —
  data representation, algorithm, protocol, resource strategy —
  as candidates for hiding
R2 [5]: Types exposed through interfaces describe the abstraction,
  not the implementation; return AggregateId not NonZeroU64, accept
  impl Iterator not Vec
R3 [6]: When multiple modules must change together for a format or
  protocol change, consolidate them into a single module that owns
  the full sequence
R4 [5]: Default to private visibility; promote to pub(crate) only
  when needed within the crate, and to pub only when needed by
  another crate

## Consequences

`EventEnvelope` accessors are the primary example: adding `correlation_id` and `causation_id` (CHE-0016, CHE-0039) did not change the consumer API because fields were hidden behind methods. Serialization format changes (CHE-0031, CHE-0045) are isolated to implementation modules. New infrastructure ports (CHE-0044) can change storage mechanisms without leaking concepts through `EventStore`. Temporal decomposition is prevented by consolidating envelope construction, sequencing, and persistence into atomic operations. Debugging difficulty is mitigated by exposing internal state through error messages and structured logging during failures (per COM-0003).
