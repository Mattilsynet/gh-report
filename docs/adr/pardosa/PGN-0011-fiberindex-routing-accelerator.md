# PGN-0011. `FiberIndex` Routing Accelerator

Date: 2026-06-08
Last-reviewed: 2026-07-10
Tier: B
Status: Accepted
Crates: pardosa

## Related

References: PGN-0001, PGN-0002, PGN-0008, PGN-0021

## Context

Source rescue ADR-0023 (`FiberIndex<K>` identity contract). PGN-0008
keeps domain identity out of the substrate: `FiberId` is dragline-local
routing identity and domain causality keys live in payload. This ADR
pins the semantic contract for an optional typed mapping from an
application-owned causality key `K` to one or more fibers on a single
journal. `FiberIndex<K>` is publicly admitted under the `pardosa::store`
façade (PGN-0008 R1), construction is opt-in via the reader side of the
façade, and a journal opened without an index pays no per-event
indexing cost. The substrate persists no part of `K`.

Amendment 2026-07-10 (Opaque Semantic Fence, PGN-0021): R5 narrowed to
carve out one opt-in exception unrelated to `FiberIndex<K>`'s own `K` — an
adopter-supplied `adopter_epoch` gate token, persisted in gate metadata
only, byte-compared at open, never interpreted or mixed into a schema
hash. `FiberIndex<K>`'s causality key `K` is untouched; see PGN-0021.

## Decision

`FiberIndex<K>` is per-journal, opt-in, log-authoritative, and rebuilt on demand from the log via an adopter-supplied closure-shaped extractor `Fn(&Event<T>) -> impl IntoIterator<Item = K>`. The index never leads the log and never lags it across a sync. Lookup returns a typed value with `Empty` / `Unique(fiber)` / `Diverged(fibers)` shapes; `Diverged` is not an error — it is the typed observation that the same `K` lives on two or more fibers. `K` is application-owned and opaque to Pardosa; the substrate never persists `K`, mixes it into a schema hash, or encodes it into `.pgno` bytes.

R1 [5]: `FiberIndex<K>` is per-journal scope, bounded by the lifetime of
  the owning `EventStore<T>`, drop-on-close, with no on-disk index file
  and no sidecar; the log is the sole durable artefact.
R2 [5]: Construction is an explicit adopter call against the reader side
  of the `EventStore<T>` façade; no `EventStore::open*` constructor
  implicitly builds an index, and a journal opened without requesting an
  index incurs zero per-event indexing cost.
R3 [5]: The index never leads the log: a lookup against an event whose
  `Lsn` has not been acknowledged durable claims no durability beyond
  what the substrate has minted. After `StoreWriter::sync` returns, the
  index observes every event up to that `Lsn` on the same handle.
R4 [5]: Lookup returns `Empty` / `Unique(fiber)` / `Diverged(fibers)`;
  `Diverged` is a typed value (multi-fiber observation), not a failure;
  silent collapse to `Unique` via first-write-wins or last-write-wins
  is forbidden.
R5 [5]: `K` is application-owned and opaque to the substrate; the
  substrate never persists `K`, transmits `K` over a wire, mixes `K`
  into any schema hash, or encodes `K` into `.pgno` bytes. Sole exception,
  carved out by PGN-0021: an `adopter_epoch` gate token — distinct from
  `K` — the substrate may persist in gate metadata only, byte-compared at
  open, never interpreted or mixed into a schema hash; PGN-0021 R1 states
  the boundary.
R6 [5]: No writer method accepts an `expected_version`-shaped parameter
  under this ADR; any future primitive requires a follow-up ADR keyed on
  this identity contract and naming the partial-failure shape against a
  `Diverged` `K`.

## Consequences

+ becomes easier: O(index) lookup from a domain key to fibers for
  application-defined causality keys; the index is recoverable from the
  log alone — losing it loses no domain state; coexistence with
  `LineCursor` (independent derived artefacts).
− becomes harder: substrate-side enforcement of "one fiber per `K`"
  (the substrate offers no opinion; the adopter enforces via payload-side
  domain logic); cross-journal `K`-correlation (still payload-owned).
risks/migration: any `FiberIndex<K>` implementation conforms to the
  rules above as its acceptance contract; this ADR introduces no
  `.pgno` change, no `Lsn` constructor widening, and no `FiberId`
  visibility change.
