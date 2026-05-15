# GEN-0031. Rust-Only — Cross-Language Read Deferred

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0003

## Context

Schema hashing uses `stringify!` to produce canonical Rust type definitions at compile time (GEN-0003). This is inherently Rust-specific — the hash is computed from Rust syntax. Cross-language formats use language-neutral IDLs and code generators. pardosa-genome has no IDL — the Rust type definition _is_ the schema.

## Decision

pardosa-genome v1 is Rust-only. No cross-language interoperability guarantees in this
version. The schema hash, schema source, and wire format are stable, but readers in
other languages must reverse-engineer layout from the embedded schema source text
(GEN-0009) or a future schema export format.

Cross-language **read-only** support (non-Rust readers consuming Rust-written genome
files) is deferred to a future version. The path forward:

1. Define a language-neutral schema export format (JSON/binary description of types,
   field names, offsets, alignment).
2. Embed the export alongside the Rust schema source in file headers.
3. Build read-only libraries for target languages using the export.
4. Writing remains Rust-only (requires `GenomeSafe` derive + serde).

R1 [9]: pardosa-genome v1 is Rust-only with no cross-language
  interoperability guarantees
R2 [9]: The Rust type definition is the schema — no external IDL
R3 [9]: Cross-language read-only support is deferred to a future
  version with a language-neutral schema export format

## Consequences

- No IDL complexity. Rust types are the single source of truth.
- `stringify!`-based hashing is simple, deterministic, and zero-dependency.
- Non-Rust consumers cannot read genome files without custom parsers.
- Schema hash depends on Rust syntax — any future IDL must produce compatible hashes.
- Wire format is language-neutral by construction; only schema identification is Rust-specific.
