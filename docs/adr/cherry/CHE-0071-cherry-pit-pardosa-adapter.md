# CHE-0071. cherry-pit-pardosa Adapter

Date: 2026-06-10
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted
Crates: cherry-pit-pardosa
Parent-cross-domain: PGN-0008 — adapter implements the pardosa facade for cherry-pit

## Related

References: PGN-0008, PGN-0002, PGN-0005, PGN-0011, CHE-0029, CHE-0061, CHE-0070 | Supersedes: CHE-0064, CHE-0065

## Context

gh-report needs to persist cherry-pit `EventEnvelope<DomainEvent>` values through pardosa without making every domain event derive pardosa genome traits. Pardosa's public typestate cannot resume a prior fiber after process restart, so a one-aggregate-one-fiber mapping is not a valid adapter contract. The adapter must preserve cherry-pit's logical stream semantics while using only pardosa's public facade.

## Decision

`cherry-pit-pardosa` stores each `EventEnvelope<E>` as opaque serde-encoded bytes inside a `GenomeSafe` payload with scalar `aggregate_id` and `domain_key`. Logical cherry-pit streams are reconstructed by folding pardosa's public `fiber_index` read seam on open, grouping by `aggregate_id`, sorting decoded envelopes by sequence, and validating gap-free `1..=N` streams.

R1 [5]: `cherry-pit-pardosa` is an adapter crate depending on `cherry-pit-core` and `pardosa`; domain crates depend on the adapter only at wiring boundaries, preserving the CHE-0029 acyclic crate DAG.
R2 [5]: `EnvelopePayload` carries serde-encoded `EventEnvelope<E>` bytes as opaque payload plus scalar `aggregate_id: u64` and `domain_key: String`; adding sequence as a scalar is forbidden because sequence is authoritative inside the envelope bytes.
R3 [5]: The adapter models identity as a logical `AggregateId` stream and a physical pardosa fiber per create or append event; it MUST NOT maintain or require a one-to-one `AggregateId` to `FiberId` mapping.
R4 [5]: On open, the adapter MUST build its own index by calling the public `StoreReader::fiber_index` fold with a capturing extractor, and MUST discard the returned `FiberIndex` after the fold.
R5 [5]: `load(id)` MUST gather captured events for `id`, decode envelopes, sort by embedded sequence, and validate a gap-free `1..=N` stream before returning.
R6 [5]: `list_aggregates()` MUST return the captured index keys and stay async through `ListableEventStore` per CHE-0070.
R7 [5]: `create` and `append` MUST write a fresh pardosa fiber per envelope, and `append` MUST enforce cherry-pit optimistic concurrency from the reconstructed maximum sequence.
R8 [5]: `PardosaEventStore<E>` MUST implement `SingleWriterEventStore` on the adopter-side single logical writer constraint and PGN-0011:R4's typed `Diverged` non-error semantics.

## Consequences

+ becomes easier: gh-report can move onto pardosa without changing the CHE-0054 domain event enum or deriving native pardosa schemas for every domain payload.
− becomes harder: append locality is no longer a single physical fiber per aggregate; the adapter must rebuild a logical index on open and validate streams at the boundary.
risks/migration: CHE-0064 and CHE-0065 retire because native substrate-side encoding is not the M1 path. Live JetStream parity remains source-proven until the provisioned-NATS follow-up runs the ignored test.
