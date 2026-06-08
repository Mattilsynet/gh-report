# GEN-0001. Serde-Native Serialization with GenomeSafe Marker Trait

Date: 2026-04-25
Last-reviewed: 2026-04-27
Tier: S
Status: Accepted

## Related

Root: GEN-0001
Superseded-in-part-by: GEN-0035

## Context

> **Partial supersession (2026-05-15):** GEN-0035 widens R2's
> `SCHEMA_HASH: u64` to `SCHEMA_HASH: u128` (xxh3-128 — see also
> GEN-0003 partial supersession). R1, R3, and the marker-trait
> framing carry forward unchanged. R2's `SCHEMA_SOURCE: &'static str`
> constant carries forward unchanged.

Two forces are in tension: zero-copy read performance (the event replay hot path deserializes millions of events) and adoption friction (mirror-type ecosystems like rkyv and FlatBuffers double maintenance surface and sever serde compatibility). A serde-native binary format avoids mirror types while achieving partial zero-copy (str, bytes). rkyv requires `ArchivedFoo` per `Foo`; FlatBuffers requires external `.fbs` schemas and codegen. PAR-0006 contains the detailed per-library alternatives analysis.

## Decision

Use standard `serde::Serializer` / `serde::Deserializer<'de>` as the serialization
interface. Introduce a separate `GenomeSafe` marker trait with no methods — only two
associated constants (`SCHEMA_HASH: u64`, `SCHEMA_SOURCE: &'static str`). A
`#[derive(GenomeSafe)]` proc-macro generates both constants and performs compile-time
validation.

Types only need `#[derive(Serialize, Deserialize, GenomeSafe)]`. No mirror types,
no external schema files, no code generation beyond these three derives.

R1 [2]: All serializable types use serde Serialize and Deserialize as
  the sole serialization interface — no parallel trait hierarchies
R2 [2]: GenomeSafe is a marker trait with no methods, carrying only
  SCHEMA_HASH and SCHEMA_SOURCE associated constants
R3 [2]: No mirror types, no external schema files, no code generation
  beyond derive(Serialize, Deserialize, GenomeSafe)

## Consequences

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
