# ADR-002: No schema evolution — fixed layout

**Status:** Accepted

### Context

Schema evolution (adding/removing fields, reordering) requires vtables (FlatBuffers),
offset tables (Cap'n Proto), or self-describing framing (Avro). All add read-path
complexity and branching. pardosa-genome prioritizes read speed for append-only event
storage where schema changes are handled by pardosa's migration system (new file,
migrate forward).

### Decision

Struct layout is determined entirely by the Rust type's serde representation at compile
time. No vtables, no field presence bits, no default values for missing fields. A
compile-time xxHash64 schema fingerprint detects type mismatches at deserialization time
with `DeError::SchemaMismatch`.

Schema changes require a new file (pardosa migration model). Enum variant addition is
the only in-schema flexibility — the discriminant-based layout handles new variants
without breaking existing readers (readers reject unknown discriminants with
`DeError::UnknownVariant`).

### Consequences

- **Positive:** Minimal read-path overhead. No vtable lookups, no conditional field
  reads. Struct fields are at compile-time-predictable offsets.
- **Positive:** Simple implementation — no schema registry, no compatibility matrices.
- **Negative:** Any struct field change (add, remove, rename, reorder, retype) breaks
  binary compatibility. Requires pardosa file migration.
- **Negative:** Cannot read old files with new types or new files with old types.
  The schema hash catches this, but operational migration is required.
- **Mitigated by:** Pardosa's append-only file model already handles migrations by
  creating new files. The format's rigidity aligns with this model.
- **Cross-crate sentinel reservation:** Pardosa reserves `u64::MAX` as the
  `Index::NONE` sentinel in `Index(u64)`, a genome-encoded newtype. This value
  must never be assigned structural meaning by the wire format for `u64` fields
  of `Index` type. See
  [pardosa ADR-002](../../pardosa/adr/0002-index-none-sentinel-replacing-option.md).

### References

- genome.md §Design Principles (item 5), §Single Schema Per File, §File Header
  (`schema_hash` field)
