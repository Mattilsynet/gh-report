# ADR-001: Serde-native serialization with GenomeSafe marker trait

**Status:** Accepted

### Context

Existing zero-copy formats (rkyv, FlatBuffers) require their own trait hierarchies
or external code generation, imposing a "mirror type" tax on every data structure.
pardosa-genome targets Rust services that already use `#[derive(Serialize, Deserialize)]`
everywhere. Adding a second derive ecosystem would double the maintenance surface.

### Decision

Use standard `serde::Serializer` / `serde::Deserializer<'de>` as the serialization
interface. Introduce a separate `GenomeSafe` marker trait with no methods — only two
associated constants (`SCHEMA_HASH: u64`, `SCHEMA_SOURCE: &'static str`). A
`#[derive(GenomeSafe)]` proc-macro generates both constants and performs compile-time
validation.

Types only need `#[derive(Serialize, Deserialize, GenomeSafe)]`. No mirror types,
no external schema files, no code generation beyond these three derives.

### Consequences

- **Positive:** Near-zero adoption friction for existing serde users. Any type that
  already derives `Serialize + Deserialize` only needs one additional derive.
- **Positive:** Full access to serde's ecosystem (custom serializers, derives, testing).
- **Negative:** Serde's data model constrains format design. Some serde attributes
  (`flatten`, `tag`, `untagged`, `skip_serializing_if`) are incompatible with
  fixed-layout binary serialization — must be rejected at compile time.
- **Negative:** Manual `Serialize` impls can bypass `GenomeSafe` validation. Runtime
  detection and `verify_roundtrip` CI checks are defense-in-depth.
- **Risk:** Serde major version changes could require format migration. Mitigated by
  serde's strong backward compatibility record.

### References

- genome.md §Motivation, §Design Principles (item 3), §Serde Integration
