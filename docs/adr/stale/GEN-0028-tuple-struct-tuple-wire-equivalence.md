# GEN-0028. Tuple Struct / Tuple Wire Equivalence

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0003

## Context

Serde treats tuple structs (e.g., `struct Point(f64, f64)`) and plain tuples identically — both call `serialize_tuple`. The serialized bytes are identical regardless of wrapper type. pardosa-genome's schema hash (GEN-0003) includes the root type name, so hashes differ (`"struct:Point"` vs `"tuple2"`), but wire bytes after the header are the same.

## Decision

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

R1 [9]: Tuple structs and plain tuples produce identical wire bytes
  for their data payload
R2 [9]: They are distinguished only by schema hash — the format does
  not store type names in the binary payload
R3 [9]: Schema hash is the sole mechanism providing type identity for
  structurally equivalent types

## Consequences

- Consistent with serde's data model — no special-case logic.
- Schema hash catches accidental type substitution in normal operation.
- Structurally identical types produce identical payload; a future `decode_unchecked` bypassing the hash would allow silent type confusion.
- Users expecting structural typing must understand pardosa-genome uses nominal typing via schema hash.
