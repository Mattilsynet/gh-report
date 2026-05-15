# GEN-0009. One Schema Per File with Embedded Schema Source

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0003

## Context

Multi-schema files (different types in the same file) require per-message schema
identification, complicating the reader and index structure. pardosa's storage model
is one-type-per-file (append-only event log per aggregate type).

Self-describing formats (Avro, JSON) embed schema or field names in every message.
Non-self-describing formats (FlatBuffers, bincode) require external schema knowledge.
pardosa-genome wants a middle ground: compact binary messages with human-inspectable
schema metadata in the file header.

## Decision

All messages in a file share the same schema. The file header contains the 8-byte
schema hash as the authoritative compatibility check. Optionally, the `GenomeSafe`
derive macro's `SCHEMA_SOURCE` — a cleaned Rust type definition as plain UTF-8 — is
embedded in a schema block between the file header and the first message.

The embedded source is informational. A developer can read the type structure from the
file without the original source code. Bare messages do not carry embedded source (they
are compact; the hash suffices).

Schema changes require a new file (pardosa migration model).

R1 [5]: All messages in a file share the same schema with the 8-byte
  hash as the authoritative compatibility check
R2 [6]: The GenomeSafe derive macro's SCHEMA_SOURCE is optionally
  embedded in a schema block between the file header and first message
R3 [5]: Schema changes require a new file — no in-place schema updates
R4 [6]: Header and footer byte layouts are pinned by committed golden
  fixtures for conformance testing

## Consequences

Readers stay simple because no per-message type dispatch is required. Embedded `SCHEMA_SOURCE` enables inspection without source access but adds header bytes. Heterogeneous messages require separate files or an outer enum. Golden header/footer fixtures catch accidental byte-layout changes.
