# Changelog

## [Unreleased]

- breaking? Y — pardosa removes `Box<T>` / `Arc<T>` event-eligibility impls from the GenomeSafe/EventSafe vocabulary while keeping their codec Encode/Decode infrastructure; the public payload-type surface narrows under the PGN-0013:R8/R1 closed bounded-field-type amendment, but gh-report's golden SCHEMA_HASH stayed byte-identical. Mission bead: adr-fmt-u9laf.
- breaking? N — pardosa-nats `JetStreamReplayRecord` adds optional `schema_tag` metadata and `JetStreamHandle::append_with_replay_tag(...)` so pardosa can carry per-message `ENVELOPE_HASH` as an opaque JetStream header; `payload` remains byte-verbatim per PGN-0010:R4, and the always-`#[non_exhaustive]` record field addition is non-breaking per COM-0021:R2 and PGN-0012:R3. Mission bead: adr-fmt-xdjus.
- breaking? N — pardosa `BackendError` adds `Connect { op, source }` and `Replay { op, source }` variants so JetStream connect/replay failures remain distinct from publish failures; adding variants to the always-`#[non_exhaustive]` enum is non-breaking per PGN-0012:R3. gh-report now carries `BackendOp` through `StoreError::BackendInfrastructure { op, source }` for operator-facing infrastructure errors. Mission bead: adr-fmt-ihppt.2.
