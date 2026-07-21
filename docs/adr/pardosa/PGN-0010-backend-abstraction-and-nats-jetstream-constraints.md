# PGN-0010. Backend Abstraction and NATS/JetStream Constraints

Date: 2026-06-08
Last-reviewed: 2026-06-14
Tier: A
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0001, PGN-0003, PGN-0005, PGN-0008, PGN-0016, SEC-0011

## Context

Sources rescue ADR-0022 (app-configurable authoritative storage backends) and compatible Solon PAR-0004 (single-writer per stream). `EventStore<T>` admits authoritative storage backends through a sealed typed handle, not a `PathBuf`. Path constructors stay on the public surface as convenience wrappers that internally construct `PgnoBackend`. The append-shaped backend trait carries `AckPosition` as a backend-opaque ordering primitive; `EventId` is Pardosa-minted and never derived from `AckPosition`. Per the 2026-06-06 amendment, the substrate now occupies the `crates/pardosa-nats/` slot directly; the empty adapter skeleton was retired.

An on-read integrity survey found no ADR mandating the same on-read chain across `AuthoritativeBackend` adapters; the two production arms (`.pgno`, JetStream) had drifted to schema-hash + contiguity only, skipping three precursor checks that the `.pgno`-only `open_validated` (PGN-0008:R5) does run. R8 closes the gap: the stage exists universally above the sealed seam (R1). The authoritative check set — ratified here as the corpus's single enumeration — is five checks over raw canonical bytes each adapter delivers byte-identical per R4 (subjects/headers/`AckPosition` never feed the CRH):

- `CheckedReplayKind::EventIdPositionMismatch` — contiguity.
- `CheckedReplayKind::PrecursorOutOfBounds` — precursor index bounds.
- `CheckedReplayKind::PrecursorFiberMismatch` — precursor same-fiber linkage.
- `CheckedReplayKind::PrecursorHashMismatch` — precursor hash continuity.
- `Error::SchemaHashMismatch` — container schema hash (not a `CheckedReplayKind`; gates the header, not per-event replay).

Frontier-roll (raw-byte BLAKE3 fold) is bookkeeping, not a check — no error path, excluded. The precursor checks are additive over `Fiber::new`/`advance`'s intra-fiber ordering (`FiberInvariantKind`) — a disjoint class inspecting no `Precursor`; no double-enforcement.

pardosa's own read-path error is `persist::Error::CheckedReplay { kind }`, surfacing as `PardosaError` (`#[non_exhaustive]`, matchable, PGN-0016). `StoreError::CorruptData` is not a pardosa type; it is SEC-0011:R3's required-surface vocabulary, owned by the downstream consumer that maps `CheckedReplay` to it. R8 mandates stage location and enumeration only, not that mapping.

## Decision

`AuthoritativeBackend` and `BackendSink` are strong-sealed substrate traits owned by `pardosa` and implemented via in-`pardosa` adapter shims wrapping an opaque per-backend handle (`PgnoBackend`, `JetStreamBackend`). `EventStore::create_with_backend(handle, options)` and `EventStore::open_with_backend(handle, options)` are the typed admission seam: create authors the canonical-empty container in pardosa core, while open rehydrates existing authoritative bytes. Backends own their async runtime internally; the façade stays synchronous; per-operation timeouts are typed via `BackendError::Timeout`. Single-writer per stream is the v0 stance; distributed writers are scoped to a future ADR. Canonical wire bytes cross the backend boundary verbatim — no transformation, no re-framing, no envelope-stripping.

R1 [4]: `AuthoritativeBackend` is strong-sealed via a private supertrait
  owned by `pardosa`; the only public impls are in-`pardosa` adapter shims
  wrapping per-backend opaque handles.
R2 [5]: `EventStore::open(path)` and friends remain on the public surface
  as convenience constructors that internally build `PgnoBackend`; the
  typed seam has `create_with_backend(handle, options)` for
  pardosa-authored canonical-empty creation and
  `open_with_backend(handle, options)` for rehydrate-only admission. No
  generic backend parameter (`EventStore<T, B>`) is exposed.
R3 [5]: `AckPosition` is backend-opaque, monotonic within one backend
  instance, and carries no cross-backend meaning; `EventId` is
  Pardosa-minted at append time and never derived from `AckPosition`.
R4 [5]: Canonical wire bytes cross the backend boundary byte-identical —
  no transformation, no re-framing, no envelope-stripping. Subjects,
  headers, and `AckPosition` are metadata and never input to the
  frontier CRH or to canonical encoding.
R5 [5]: Backends own their async runtime internally; the public façade is
  synchronous. Per-operation timeouts surface as
  `BackendError::Timeout { op, elapsed, configured }`
  (`#[non_exhaustive]` per PGN-0006).
R6 [5]: Single-writer per authoritative-storage instance is the v0 stance;
  enforcement is either constructor-time exclusion or documented adopter
  constraint with a Phase 3 conformance test. Distributed writers require
  a follow-up ADR defining `FiberId` partition and `EventId` monotonicity.
R7 [4]: Backend traits MUST NOT fuse codec and sealing axes; per-operation
  codec bounds remain `T: Encode + GenomeSafe` on write paths and
  `T: Decode + GenomeSafe` on read paths (PGN-0003).
R8 [4]: An on-read verify stage exists universally above the sealed seam
  (R1), running the ratified check set (Context) over raw canonical bytes
  each adapter delivers byte-identical (R4); it hashes raw bytes, never
  re-encodes, keeping the read-path bound `Decode + GenomeSafe` (R7,
  PGN-0008:R2/R5). Each adapter declares whether it surfaces hash-chain
  verification: true runs the full precursor chain; false skips the
  enforcing hash check (SEC-0011:R3 opt-in preserved). Default true;
  opt-out requires an ADR-cited reason. This mandates stage location and
  enumeration only — not SEC-0011:R3's surfacing obligation (deferred to a
  future ADR) — and is mutation detection, not authentication (PGN-0005:R4).

## Consequences

+ becomes easier: adopters can configure JetStream as authoritative storage
  without forking the runtime; replica/fan-out adapters remain available;
  the `.pgno` backend and an in-memory test backend coexist behind one
  trait; cross-adapter on-read parity is now legislated once, above the
  seam, not re-derived per adapter.
− becomes harder: out-of-tree backend impls (the sealed trait + in-`pardosa`
  adapter shim is the only path); cross-backend live migration (PGN-0009
  per-backend stance binding); transports masquerading as substrates; an
  adapter opting out of hash-chain surfacing needs an ADR-cited reason.
 risks/migration: PGN-0016 supplies `Nats-Expected-Last-Subject-Sequence`
   fencing as the JetStream implementation of R6's single-writer enforcement.
   PAR-0004 remains retired; the enforcement gap is closed for JetStream. The
   `crates/pardosa-nats/` slot now hosts the substrate; any future outbound
   NATS transport adapter lands in a new crate. R8 legislates stage location
   only; closing the per-adapter precursor-check gaps is follow-on
   implementation work, not this ADR change.

## Amendment 2026-07-21 (P2b enforce-capability, default stays ObserveOnly)

R8's ratified check set now runs uniformly across both backend arms via
the shared stage's `PrecursorCheckMode` switch (`Enforce` /
`ObserveOnly`), closing the per-adapter gap noted above as follow-on
work. The stage can reject on any of the three precursor checks
(bounds/same-fiber/hash) when `Enforce`, surfacing
`persist::Error::CheckedReplay { kind }` per adapter, matchable and
`#[non_exhaustive]` (PGN-0016:R9) — mutation detection, not
authentication (PGN-0005:R4). This is capability-now-exists, not a
default flip: the shipped default is `ObserveOnly` (checks run,
violations logged, no rejection), pending a NATS-baseline replay soak
that has not yet run against a live JetStream stream (the `.pgno`
baseline — a 771-repo captured sweep — already opens clean under
`Enforce` with zero rejections). R8's "default true" language describes
the per-adapter hash-chain-surfacing opt-out (SEC-0011:R3 axis,
unchanged, still deferred); it does not commit to an `Enforce`-by-default
runtime mode, which is a separate future decision this amendment does
not make.
