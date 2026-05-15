# ADR-029: Reject `#[serde(default)]` at compile time

**Status:** Accepted

### Context

`#[serde(default)]` is a serde attribute that provides default values for
fields that are absent during deserialization. In self-describing formats
(JSON, TOML, YAML), fields can be legitimately absent from the serialized
data, and `default` fills them in.

In pardosa-genome's fixed-layout format, fields are **always present** at
their compile-time-determined offsets. There is no concept of a "missing
field" — every field occupies its inline slot regardless of value. The
`default` attribute compiles successfully but has no observable effect,
creating a silent behavioral difference from other serde formats.

ADR-004 establishes the pattern of rejecting incompatible serde attributes
at compile time (`flatten`, `tag`, `untagged`, `skip_serializing_if`,
`content`). The `default` attribute was initially excluded from rejection
because it does not cause data corruption — it simply does nothing.

However, silently accepting `default` creates a user experience problem:
developers migrating from JSON-based serde types apply `default` expecting
missing-field defaulting behavior, which never activates. This can lead to
incorrect assumptions about serialization behavior and debugging confusion.

### Decision

`#[derive(GenomeSafe)]` rejects `#[serde(default)]` at compile time with
a clear error message explaining that fixed-layout formats have no concept
of missing fields.

**Error message:**
```
GenomeSafe: #[serde(default = "...")] is not supported on field.
Fixed-layout format has no concept of missing fields. All fields are
always present at their compile-time offsets.
```

The `default` path attribute (type-level `#[serde(default)]`) is also
rejected with the same rationale.

**Implementation:** Add `"default"` to the rejected attribute lists in
`pardosa-derive/src/lib.rs`.

### Consequences

- **Positive:** Prevents silent behavioral divergence from JSON/TOML serde
  usage. Users get a clear compile-time error instead of mysterious no-op
  behavior.
- **Positive:** Aligns with the existing ADR-004 pattern — all incompatible
  serde attributes are rejected at compile time.
- **Positive:** Forces users to explicitly acknowledge that fixed-layout
  fields are always present, improving understanding of the format's
  semantics.
- **Negative:** Users migrating existing serde types must remove
  `#[serde(default)]` attributes before adding `GenomeSafe`. This is
  intentional friction — the attribute's behavior would be misleading.
- **Negative:** `#[serde(default)]` on the same type used with both JSON
  (where it works) and pardosa-genome (where it's rejected) requires
  conditional compilation or a separate type definition.
- **Residual risk:** Type-level `#[serde(default)]` (on the struct itself,
  not on individual fields) is also rejected. Both forms are caught by the
  same attribute scanning logic.

### References

- ADR-004 (compile-time rejection of incompatible serde attributes)
- genome.md §Serde Attribute Tests: `#[serde(default)]` documentation
- genome.md §Limitations: fixed-layout semantics
- `pardosa-derive/src/lib.rs` — `REJECTED_NAME_VALUE_ATTRS`,
  `REJECTED_PATH_ATTRS`
