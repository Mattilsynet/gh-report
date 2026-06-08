# PGN-0008. EventStore Facade and Operation-Specific Bounds

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: A
Status: Accepted
Crates: pardosa

## Related

References: PGN-0001, PGN-0003, PGN-0007

## Context

Sources rescue ADR-0018 (public `EventStore<T>` façade) and rescue ADR-0020 (`EventSafe` decoupled from codec traits — read against PGN-0003). `pardosa::store` is the sole adopter-facing module on the runtime crate; ring-internal primitives (`authoritative`, `backend`, `cursor`, `dragline`, `event`, `fiber`, `frontier`, etc.) are all `pub(crate)` and unreachable from downstream crates. The historical root-level kit-of-parts (`pardosa::reader`, `pardosa::writer`, `pardosa::event_log`) does not exist as a current public surface — those root modules are not shipped as source files in the runtime crate. The façade composes existing runtime primitives into one entry: open / append / per-fiber read / line tail / same-fiber causal walk. `FiberId` is dragline-local — domain identity and causality live in payload (PGN-0011 names the routing accelerator).

## Decision

`pardosa::store` exports `EventStore<T>` (path-backed, file-sink specialised) plus `StoreReader<'_, T>` and `StoreWriter<'_, T>` capability handles. Adopter constructors: `create(path)`, `open_validated(path)`, `open_with_publisher(...)`; `open(path)` is `pub(crate)` by default (the unchecked rehydrate path, `pub` only under `feature = "test-support"`). Writer verbs are payload-only — `begin` / `append` / `detach` / `resume` — minting `EventId`, `FiberId`, precursor, and detached state substrate-side. Typestate `LiveFiber` / `DetachedFiber` tokens make illegal transitions unrepresentable. Three reader views: `fiber(id) → FiberHistory`, `causal_chain(head) → CausalChain` (same-fiber, this dragline), `cursor(sidecar) → LineCursor` (global ACK/resume).

R1 [5]: `pardosa::store` is the sole adopter-facing module on the runtime
  crate, paired with the ergonomic single-glob `pardosa::prelude` that
  re-exports the same items (broadening nothing); every ring-internal
  module (`authoritative`, `backend`, `cursor`, `dragline`, `event`,
  `fiber`, `frontier`, etc.) is `pub(crate)` and unreachable from
  downstream crates. No root-level `pardosa::reader`, `pardosa::writer`,
  or `pardosa::event_log` module is shipped as source.
R2 [5]: Public bounds are operation-specific, not façade-wide: writer paths
  require `T: Encode + GenomeSafe`; reader and cursor paths require
  `T: Decode + GenomeSafe`. `EventSafe` is inherited through `GenomeSafe`
  and is not a façade-level bound.
R3 [5]: `StoreReader<'_, T>` cannot name writer authority; the boundary is
  type-level, not feature-gated, mirroring PGN-0007 R6. Compile-fail tests
  (`reader_has_no_append_to`, `reader_has_no_sync`,
  `reader_does_not_coerce_to_writer`) pin the property.
R4 [5]: Writer verbs (`begin` / `append` / `detach` / `resume`) take payload
  `T` only; the substrate mints `EventId`, `FiberId`, precursor pointer,
  and detached/live state. A forged identity cannot enter the substrate
  through the public surface.
R5 [5]: `open_validated` folds the rolling frontier on raw persisted bytes
  and runs `Validate::validate` per event; `open(path)` (test-support
  `pub` only) provides framing-, schema-hash-, and contiguity-level checks
  but not per-event precursor-hash validation.
R6 [5]: `cursor.tail()` is global consumer ACK/resume over the journal's
  event line, not fiber history and not a causal-history cursor;
  `commit_offset(EventId)` is `pub(crate)` and `commit_consumed(&Event<T>)`
  is the sole adopter-facing acknowledgement verb.
R7 [5]: No public `open_with_migration` or `MigrationPolicy` symbol ships
  until the PGN-0009 migration implementation mission lands; out-of-band
  `pardosa::store::migrate::migrate_keep` remains the only public migration
  path.

## Consequences

+ becomes easier: one adopter entry for the five core capabilities; the
  most common adopter mistake (line tail vs fiber history) is rejected by
  type signature; payload-only writer verbs make illegal envelope shape
  unrepresentable; `Lsn::new` / `Event::try_new` opacity is type-system
  invariant.
− becomes harder: writing a generic-backend or path-string-dispatched
  store (the sealed typed-backend admission of PGN-0010 is required);
  bypassing capability split via root-level kit-of-parts (no longer
  available).
risks/migration: cross-dragline identity is out of scope; expected-version
  optimistic-concurrency primitives are deferred to a follow-up identity-
  model ADR (PGN-0011's contract is the gate). `pardosa-cli` carries no
  stability guarantee under this ADR.
