# CHE-0067. ListableEventStore Extension Trait

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Accepted

## Related

References: CHE-0057, CHE-0061, CHE-0065, CHE-0048

## Context

Boot-time projection replay in `adr-srv` (`AdrService::new_with_replay`)
and the gh-report saga's replay-as-rebuild path (CHE-0048 scope-carve)
need to walk every aggregate stream the store knows about. The core
`EventStore` trait deliberately omits enumeration: it binds to a single
aggregate (CHE-0005:R1) and remote substrates cannot enumerate cheaply.
File-backed substrates enumerate cheaply by directory listing; the
deleted `cherry-pit-pardosa::PardosaFileEventStore::list_aggregates`
established the pattern, and `cherry_pit_core::testing::InMemoryEventStore`
mirrors it for the in-process case. CHE-0057:R5 forbids introducing an
EventStore extension trait without a dedicated ADR; the trait already
lives in `cherry-pit-core/src/store.rs:428` and the present ADR ratifies
that surface and governs implementation conditions.

## Decision

`ListableEventStore` is a supertrait-bounded extension trait per
CHE-0057, exposing aggregate enumeration as an opt-in capability.
Implementations cover file-backed and in-process substrates; remote
substrates that cannot enumerate without `O(stream)` cost MUST NOT
implement it.

R1 [5]: `ListableEventStore` extends `EventStore` as a supertrait bound
  per CHE-0057:R2, lives in `cherry-pit-core` alongside `EventStore`,
  and is named per CHE-0057:R2's `<Capability>EventStore` convention.

R2 [5]: The trait surface is the single method `list_aggregates(&self)
  -> Result<Vec<AggregateId>, StoreError>` returning every known
  `AggregateId` in unspecified order. An empty store returns `Ok(vec![])`,
  never `Err`; errors reflect substrate I/O failure only and surface as
  `StoreError::Infrastructure`.

R3 [5]: File-backed `EventStore` implementations that enumerate cheaply
  by directory listing (e.g. `MsgpackFileStore` per CHE-0065) MUST
  implement `ListableEventStore`. In-process stores whose stream map is
  enumerable in `O(stream-count)` MUST implement it. Remote or otherwise
  non-enumerable substrates MUST NOT implement it per CHE-0057:R3.

R4 [5]: Downstream code requiring enumeration MUST bound on
  `ListableEventStore` per CHE-0057:R4, not on `EventStore`. Trait
  objects (`dyn ListableEventStore`) are forbidden, preserving
  CHE-0005:R1 single-aggregate-per-port binding across the extension.

R5 [5]: The `list_aggregates` signature is append-only per CHE-0057:R5;
  adding methods or changing the return shape requires a superseding
  ADR. Returning `Err(NotImplemented)` from `list_aggregates` is
  forbidden; substrates that cannot enumerate MUST omit the impl.

## Consequences

+ becomes easier: `adr-srv`'s `new_with_replay` and gh-report's
  replay-as-rebuild bound on `ListableEventStore` directly rather than
  re-deriving enumeration out-of-band; file-backed stores discharging
  CHE-0065 land their `list_aggregates` impl alongside the encoding
  swap in a single mission.

− becomes harder: introducing a future remote substrate that wants to
  participate in boot replay requires either an external index or a new
  capability trait (per CHE-0057), not silently widening this trait.

risks/migration: no migration — the trait already exists in
`cherry-pit-core/src/store.rs:428` and the `InMemoryEventStore` impl in
`crates/cherry-pit-core/src/testing.rs:307` already satisfies the
contract. The CHE-0065 swap mission adds `MsgpackFileStore` as the
second impl on the strength of R3; CHE-0057:R5 forecloses method
addition pressure.
