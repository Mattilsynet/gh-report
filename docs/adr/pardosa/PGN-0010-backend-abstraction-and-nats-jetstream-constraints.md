# PGN-0010. Backend Abstraction and NATS/JetStream Constraints

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: A
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0001, PGN-0003, PGN-0008

## Context

Sources rescue ADR-0022 (app-configurable authoritative storage backends) and compatible Solon PAR-0004 (single-writer per stream). `EventStore<T>` admits authoritative storage backends through a sealed typed handle, not a `PathBuf`. Path constructors stay on the public surface as convenience wrappers that internally construct `PgnoBackend`. The append-shaped backend trait carries `AckPosition` as a backend-opaque ordering primitive; `EventId` is Pardosa-minted and never derived from `AckPosition`. Per the 2026-06-06 amendment, the substrate now occupies the `crates/pardosa-nats/` slot directly; the empty adapter skeleton was retired.

## Decision

`AuthoritativeBackend` and `BackendSink` are strong-sealed substrate traits owned by `pardosa` and implemented via in-`pardosa` adapter shims wrapping an opaque per-backend handle (`PgnoBackend`, `JetStreamBackend`). `EventStore::open_with_backend(handle, options)` is the typed admission seam. Backends own their async runtime internally; the façade stays synchronous; per-operation timeouts are typed via `BackendError::Timeout`. Single-writer per stream is the v0 stance; distributed writers are scoped to a future ADR. Canonical wire bytes cross the backend boundary verbatim — no transformation, no re-framing, no envelope-stripping.

R1 [4]: `AuthoritativeBackend` is strong-sealed via a private supertrait
  owned by `pardosa`; the only public impls are in-`pardosa` adapter shims
  wrapping per-backend opaque handles.
R2 [5]: `EventStore::open(path)` and friends remain on the public surface
  as convenience constructors that internally build `PgnoBackend`; the
  typed seam is `EventStore::open_with_backend(handle, options)`. No
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

## Consequences

+ becomes easier: adopters can configure JetStream as authoritative storage
  without forking the runtime; replica/fan-out adapters remain available;
  the `.pgno` backend and an in-memory test backend coexist behind one
  trait.
− becomes harder: out-of-tree backend impls (the sealed trait + in-`pardosa`
  adapter shim is the only path); cross-backend live migration (PGN-0009
  per-backend stance binding); transports masquerading as substrates.
risks/migration: PAR-0004's `Nats-Expected-Last-Subject-Sequence` fencing
  is one valid implementation of R6's single-writer enforcement when the
  backend is JetStream; PAR-0004 retirement is deferred to a follow-up
  mission. The `crates/pardosa-nats/` slot now hosts the substrate; any
  future outbound NATS transport adapter lands in a new crate.
