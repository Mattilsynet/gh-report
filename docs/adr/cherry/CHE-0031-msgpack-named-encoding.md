# CHE-0031. MessagePack with Named Encoding for Persistence

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: CHE-0022

## Context

`MsgpackFileStore` persists aggregate event streams to disk. The format must balance compactness, performance, and forward compatibility. JSON is verbose and slow. Bincode and positional MessagePack break deserialization when fields are added or reordered. MessagePack with named/map encoding supports `#[serde(default)]` for new fields — existing data deserializes with defaults for absent keys, enabling forward-compatible evolution.

## Decision

`MsgpackFileStore` uses `rmp_serde::encode::to_vec_named` (map
encoding with string keys) for all writes. Deserialization uses
`rmp_serde::from_slice`.

R1 [9]: Use rmp_serde::encode::to_vec_named (map encoding with
  string keys) for all MsgpackFileStore writes
R2 [9]: New Option fields with #[serde(default)] can be added to
  EventEnvelope without migrating existing data files
R3 [9]: MsgpackFileStore keeps a committed golden fixture for a
  representative Vec<EventEnvelope<E>> stream encoded with named keys

This was validated when `correlation_id` and `causation_id` were added
to `EventEnvelope` with `#[serde(default)]` — existing data without
these fields deserializes with `None` values. A dedicated test
(`deserializes_old_format_without_correlation_fields`) proves this.

## Consequences

New `Option` fields with `#[serde(default)]` can be added without migrating existing data. Wire size grows versus positional msgpack, acceptable for the file store. Switching formats requires migration. Golden fixtures catch accidental serializer or envelope-layout changes.
