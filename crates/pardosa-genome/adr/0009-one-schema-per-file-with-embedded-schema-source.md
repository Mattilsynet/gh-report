# ADR-009: One schema per file with embedded schema source

**Status:** Accepted

### Context

Multi-schema files (different types in the same file) require per-message schema
identification, complicating the reader and index structure. pardosa's storage model
is one-type-per-file (append-only event log per aggregate type).

Self-describing formats (Avro, JSON) embed schema or field names in every message.
Non-self-describing formats (FlatBuffers, bincode) require external schema knowledge.
pardosa-genome wants a middle ground: compact binary messages with human-inspectable
schema metadata in the file header.

### Decision

All messages in a file share the same schema. The file header contains the 8-byte
schema hash as the authoritative compatibility check. Optionally, the `GenomeSafe`
derive macro's `SCHEMA_SOURCE` — a cleaned Rust type definition as plain UTF-8 — is
embedded in a schema block between the file header and the first message.

The embedded source is informational. A developer can read the type structure from the
file without the original source code. Bare messages do not carry embedded source (they
are compact; the hash suffices).

Schema changes require a new file (pardosa migration model).

### Consequences

- **Positive:** Simple reader and index — no per-message type dispatch.
- **Positive:** Human-readable schema in the file header enables inspection without
  source access (`genome-dump` CLI displays it).
- **Positive:** `SCHEMA_SOURCE` is auto-generated from the derive input — always in
  sync with the actual type, no manual maintenance.
- **Negative:** Cannot store heterogeneous message types in one file. Must use separate
  files or an outer enum wrapper.
- **Negative:** Embedded source adds bytes to the file header. Mitigated: `schema_size=0`
  is valid for backward compatibility.

### References

- genome.md §Single Schema Per File, §Embedded Schema Source, §File Header
  (`schema_size` field), §Crate Structure (`genome-dump` CLI)
