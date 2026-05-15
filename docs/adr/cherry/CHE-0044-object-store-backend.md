# CHE-0044. Object Store Backend (Planned)

Date: 2026-04-25
Last-reviewed: 2026-04-29
Tier: D
Status: Accepted

## Related

References: CHE-0006, CHE-0031, CHE-0032, CHE-0043, COM-0025

## Context

`MsgpackFileStore` is the sole `EventStore` implementation, limited to single-machine deployment. The `object_store` crate (Apache Arrow) provides a unified API for local filesystem, S3, GCS, and Azure Blob. Its conditional put (`PutMode::Update` with ETags) maps naturally to optimistic concurrency, replacing advisory locking and temp-file-rename.

## Decision

Adopt the `object_store` crate as the storage abstraction for a new
`EventStore` implementation alongside `MsgpackFileStore`. The new
implementation will:

R1 [10]: Use conditional put (PutMode::Update) for optimistic
  concurrency instead of file locks and rename
R2 [10]: Preserve the MessagePack named-fields wire format across
  all storage backends
R3 [10]: Keep MsgpackFileStore available for single-machine
  zero-dependency deployments alongside the object store backend
R4 [10]: Verify each object store provider's ETag, conditional put,
  read-after-write, overwrite, and list consistency before support
R5 [10]: Treat object listing as advisory; authoritative stream reads
  use exact aggregate object keys and compare-and-swap metadata

1. Implement `EventStore` with the same `MessagePack + named fields`
   wire format.
2. Use conditional put (`PutMode::Update`) for optimistic concurrency
   instead of file locks and rename.
3. Support local filesystem via `object_store::local::LocalFileSystem`
   as a drop-in replacement for `MsgpackFileStore` in tests.
4. Support S3-compatible backends only after provider consistency tests
   prove ETag and conditional write semantics.
5. Replace process-level fencing (CHE-0043) with backend CAS semantics
   for each aggregate object key.

The `MsgpackFileStore` remains available for single-machine,
zero-dependency deployments. The two implementations coexist — users
choose at wiring time.

## Consequences

The object backend adds a feature-gated dependency and higher request latency. Conditional put replaces local flock only after provider-specific CAS tests pass. The wire format remains identical, so migration copies `.msgpack` streams to object keys.
