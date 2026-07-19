# CHE-0057. Extension-Trait Composition Policy for EventStore Capabilities

Date: 2026-05-14
Last-reviewed: 2026-07-19
Tier: A
Status: Accepted

## Related

References: CHE-0005, CHE-0029, CHE-0030

## Context

Pardosa-as-second-`EventStore` (Track 2.1) surfaces two capabilities the
existing file-store cannot offer: physical purge / id reuse (PAR-0001
fiber state machine) and per-stream hash-chain integrity (surfaced as
`HashChainedEventStore`).
Forcing those methods onto the core `EventStore` trait either bloats
every implementation with `unimplemented!`-style stubs or fragments the
trait surface. Cherry-pit already binds infrastructure ports to a
single aggregate via associated types (CHE-0005:R1), which precludes
`dyn EventStore` — so a runtime-dispatch capability shim is also not
on the table. The remaining shape is supertrait-bounded extension
traits: each capability is its own trait that extends `EventStore`,
implementations opt in by implementing it, and capability-aware
downstream code bounds on the extension trait directly. Future
extensions (object-store backing per CHE-0044 when un-deferred, etc.)
follow the same idiom.

## Decision

Optional `EventStore` capabilities are surfaced as supertrait-bounded
extension traits living alongside `EventStore` in `cherry-pit-core`.
The core trait remains the minimum every implementation honours;
capability-aware code bounds on the extension trait. Per-extension
ADRs (e.g. `PurgeableEventStore`, `HashChainedEventStore`) cite this
ADR as parent.

R1 [4]: Optional capabilities not all EventStore implementations can
  offer MUST be surfaced as separate extension traits, not added to
  the core EventStore trait; the core EventStore trait remains the
  minimum every implementation honours.

R2 [4]: Extension traits MUST be named `<Capability>EventStore` (e.g.
  `PurgeableEventStore`, `HashChainedEventStore`), live in the same
  crate as `EventStore`, and extend `EventStore` as a supertrait
  bound; standalone capability traits without that bound are not
  permitted.

R3 [4]: Implementations that cannot satisfy an extension trait MUST
  NOT implement it; returning `Err(NotImplemented)` from required
  methods is forbidden. Always-failing stubs are permitted only for
  an in-progress rollout, MUST be documented in the impl block, and
  MUST be removed on completion.

R4 [4]: Downstream code requiring an extension capability MUST bound
  on the extension trait, not on `EventStore`. Trait objects
  (`dyn <Capability>EventStore`) are not permitted — preserves
  CHE-0005:R1 single-aggregate-per-port binding across extensions.

R5 [4]: New extension traits MUST be introduced by a dedicated ADR
  citing CHE-0057 as parent. Once published, an extension trait's
  method signatures are append-only; removal or signature change
  requires superseding the extension's ADR.

## Consequences

+ becomes easier: New `EventStore` implementations surface
  substrate-specific capabilities (purge, hash-chain, object-store
  backing) without forcing all impls to carry them. CHE-0005:R1
  single-aggregate-per-port is preserved across the extension axis,
  because extension traits inherit the same associated-type binding
  from `EventStore`.
− becomes harder: Downstream call sites that want a capability must
  thread the extension-trait bound through every generic context.
  Introducing a new capability now requires two ADRs (this policy ADR
  plus the per-extension ADR) rather than a single core-trait edit.
risks/migration: No migration — existing code uses `EventStore`
  directly and is unaffected. Track 2.1 introduces
  `PurgeableEventStore` and `HashChainedEventStore` as the first
  applications of this pattern; their ADRs cite CHE-0057. No
  supersedes edges; this ADR is purely additive.
