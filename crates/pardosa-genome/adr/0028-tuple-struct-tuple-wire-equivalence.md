# ADR-028: Tuple struct / tuple wire equivalence

**Status:** Accepted

### Context

Serde treats tuple structs (e.g., `struct Point(f64, f64)`) and plain tuples
(e.g., `(f64, f64)`) identically at the data model level — both call
`serialize_tuple` with the same element count. This means the serialized
bytes for the inner data are identical regardless of whether the outer type
is a named tuple struct or an anonymous tuple.

pardosa-genome's schema hash (ADR-003) includes the root type name in the
hash input (`"struct:Point"` vs `"tuple2"`), so the schema hashes differ.
But the wire bytes after the header (where the schema hash lives) are the
same.

### Decision

Tuple structs and plain tuples produce **identical wire bytes** for their
data payload. They are distinguished **only by schema hash**.

- `struct Point(f64, f64)` and `(f64, f64)` serialize to the same data bytes.
- Their schema hashes differ: `hash("struct:Point") ⊕ ...` vs
  `hash("tuple2") ⊕ ...`.
- In bare messages, the schema hash at bytes 2–9 catches substitution.
- In file format, the file header schema hash catches substitution.
- If schema verification is bypassed (future `decode_unchecked` — currently
  does not exist), deserializing one from the other's bytes succeeds silently.

This is **intentional**. The format does not store type names in the binary
payload — only the schema hash provides type identity. This matches serde's
own data model equivalence between tuple structs and tuples.

### Consequences

- **Positive:** Consistent with serde's data model. No special-case logic
  needed in the serializer or deserializer.
- **Positive:** Schema hash catches accidental type substitution in normal
  operation (always-verify, ADR-011).
- **Positive:** Wire format simplicity — no additional type discriminator
  beyond the schema hash in the header.
- **Negative:** Types with identical serde structure but different names
  (e.g., `Meters(f64)` vs `Seconds(f64)`) produce identical payload bytes.
  Schema hash is the only defense. If a future `decode_unchecked` bypasses
  the hash, silent type confusion is possible.
- **Negative:** Users expecting structural typing (same layout = same type)
  must understand that pardosa-genome uses nominal typing via schema hash.

### References

- ADR-003 (schema hashing — root type name included in hash input)
- ADR-011 (always-verify — schema hash checked on every decode)
- genome.md §Limitations: "Tuple structs and plain tuples produce identical
  wire representations. Deserializing one as the other succeeds silently.
  This is intentional."
- `genome_safe.rs` — tuple blanket impls use `"tuple{N}"` prefix,
  derive macro uses `"struct:{Name}"` prefix
