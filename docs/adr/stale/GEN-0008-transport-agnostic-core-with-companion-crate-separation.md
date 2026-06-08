# GEN-0008. Transport-Agnostic Core with Companion Crate Separation

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0001

## Context

pardosa-genome is designed for use over NATS/JetStream, local file storage, and
potentially other transports. Coupling transport logic into the serialization crate
would add dependencies, complicate feature flags, and limit reuse.

## Decision

`pardosa-genome` is strictly `bytes ↔ types`. Two wire formats are defined:

- **Bare message**: `encode`/`decode` API. Size-prefixed, schema-hash-verified.
  For IPC, network embedding, single messages.
- **File format**: `Writer`/`Reader` API. Multi-message with file header, schema block,
  message index, footer, xxHash64 checksums.

Transport integration (NATS headers, JetStream stream lifecycle, KV-based discovery,
metadata messages) is provided by a separate **`pardosa-genome-nats`** companion crate
that depends on `pardosa-genome` for encode/decode.

R1 [5]: pardosa-genome is strictly bytes-to-types with no transport
  dependencies
R2 [5]: Two wire formats are defined — bare message for IPC/network and
  file format for multi-message persistence
R3 [6]: Transport integration is provided by a separate companion crate
  that depends on pardosa-genome for encode/decode

## Consequences

- **Positive:** Serialization crate has minimal dependencies — no async runtime, no
  network libraries.
- **Positive:** Can be used in contexts beyond NATS (embedded, testing, CLI tools)
  without pulling transport dependencies.
- **Positive:** Transport companion crate can evolve independently (NATS client version
  upgrades, protocol changes).
- **Negative:** Two crates to maintain and version. Transport crate must stay compatible
  with serialization crate.
