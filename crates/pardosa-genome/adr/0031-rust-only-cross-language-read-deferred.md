# ADR-031: Rust-only in v1 — cross-language read-only deferred

**Status:** Accepted

### Context

Schema hashing in pardosa-genome uses `stringify!` to produce canonical Rust type
definitions at compile time (ADR-003). This is inherently Rust-specific — the schema
source text contains Rust syntax (`struct Player { name: String, hp: u32 }`), and the
hash is computed from this Rust-specific representation.

Cross-language serialization formats (FlatBuffers, Cap'n Proto, Protocol Buffers) use
language-neutral IDL files and code generators. pardosa-genome does not have an IDL —
the Rust type definition _is_ the schema.

genome.md §Future Scope already identifies an "Exportable Schema Definition Format"
as a future direction.

### Decision

pardosa-genome v1 is Rust-only. No cross-language interoperability guarantees in this
version. The schema hash, schema source, and wire format are stable, but readers in
other languages must reverse-engineer layout from the embedded schema source text or
a future schema export format.

Cross-language **read-only** support (non-Rust readers consuming Rust-written genome
files) is deferred to a future version. The path forward:

1. Define a language-neutral schema export format (JSON/binary description of types,
   field names, offsets, alignment).
2. Embed the export alongside the Rust schema source in file headers.
3. Build read-only libraries for target languages using the export.
4. Writing remains Rust-only (requires `GenomeSafe` derive + serde).

### Consequences

- **Positive:** No IDL complexity. Rust types are the single source of truth.
- **Positive:** `stringify!`-based hashing is simple, deterministic, and zero-dependency.
- **Negative:** Non-Rust consumers cannot read genome files without custom parsers.
- **Negative:** Schema hash depends on Rust syntax — any future IDL must produce
  compatible hashes or define a separate hash namespace.
- **Mitigated:** The wire format (ADR-007, ADR-012) is language-neutral by construction
  (LE integers, offsets, heap regions). Only schema identification is Rust-specific.

### References

- ADR-003 (xxHash64 schema hashing, `stringify!`-based)
- genome.md §Future Scope ("Exportable Schema Definition Format")
- ADR-009 (embedded schema source in file headers)
