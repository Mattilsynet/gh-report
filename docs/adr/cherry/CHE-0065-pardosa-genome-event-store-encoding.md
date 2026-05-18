# CHE-0065. pardosa-genome Event Store Encoding

Date: 2026-05-18
Last-reviewed: 2026-05-18
Tier: D
Status: Accepted

## Related

References: CHE-0022, GEN-0015, GEN-0037, CHE-0044, CHE-0064 | Supersedes: CHE-0031

## Context

CHE-0031 mandated `rmp_serde::encode::to_vec_named` for
`MsgpackFileStore` writes, paired with `#[serde(default)]`-based
additive field evolution (CHE-0022:R3). The 2026-05-18 prime
directive ratifies `pardosa-genome` as the workspace event serde;
GEN-0029 rejects `#[serde(default)]` on `GenomeSafe`-deriving types
and GEN-0015 provides only format-level version negotiation. The
named-MessagePack mechanism is therefore replaced wholesale. User
operating constraints accept hard-cut + re-scrape on schema change
(no production deployments, no data migration).

## Decision

The event store wire format is pardosa-genome. Schema evolution is
realised via GEN-0015's file-header version field — readers reject
unknown versions and the caller's recovery branch discards and
re-scrapes.

R1 [9]: Use `pardosa_genome::to_vec` (file format) for all
  `MsgpackFileStore` writes; use `pardosa_genome::from_bytes` for
  all reads. The crate-internal type name `MsgpackFileStore` is
  retained until a follow-up structural rename mission.

R2 [9]: On read, propagate `pardosa_genome::DeError::VersionMismatch`
  and `FileError::UnsupportedVersion` to the caller as
  `StoreError::SchemaVersionMismatch`; gh-report's restart path maps
  this to discard-and-rebuild via re-scrape.

R3 [9]: gh-report bumps its domain schema generation (the value
  written into GEN-0015's file-header `format_version`) when any
  type in the `DomainEvent` reachable closure changes shape.
  Bumping is a deliberate, reviewed act paired with each PR that
  changes the closure.

R4 [9]: The committed golden fixture for a representative
  `Vec<EventEnvelope<E>>` stream is re-encoded in pardosa-genome
  bytes; the fixture format and the round-trip regression test
  follow CHE-0038's pattern unchanged in intent.

## Consequences

+ becomes easier: derived canonical Encode/Decode via
  `#[derive(GenomeSafe)]` (GEN-0037:R4); single event substrate.

− becomes harder: schema evolution — additive Option-field
  evolution (CHE-0022:R3, amended) is replaced by hard-cut version
  bumping; data files written by older schemas are unreadable.

risks/migration: pre-swap CHE-0031 on-disk batches are unreadable;
the swap verify path includes a re-scrape demonstration. CHE-0044
inherits this encoding.
