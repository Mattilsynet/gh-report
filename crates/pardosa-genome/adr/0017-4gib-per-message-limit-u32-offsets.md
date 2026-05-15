# ADR-017: 4 GiB per-message limit with u32 offsets

**Status:** Accepted

### Context

pardosa-genome uses u32 offsets to reference variable-length data (strings, vecs,
maps, option payloads, enum variant data) from the inline region to the heap
region. The offset width determines the maximum addressable message size.

Two options were evaluated:

- **u32 offsets (4 bytes):** Maximum message size 4 GiB. Compact inline stubs.
- **u64 offsets (8 bytes):** Unlimited message size. Doubles stub size for every
  variable-length field.

### Decision

Use u32 offsets for all intra-message references. Maximum message size per
message: 4 GiB (`u32::MAX` bytes). Enforce with `SerError::MessageTooLarge`
after serialization.

The file-level message index uses u64 offsets (`offset: u64` per index entry),
enabling arbitrarily large files containing many messages. The per-message limit
does not constrain file size.

**`Option::None` sentinel:** The value `0xFFFFFFFF` (`u32::MAX`) serves as the
`None` sentinel for `Option<T>`. This works because no single message can reach
4 GiB — `0xFFFFFFFF` is always an invalid offset. If messages could exceed
`u32::MAX` bytes, this sentinel would be ambiguous.

**Interaction with `SerError::MessageTooLarge`:** After the `WritingSerializer`
completes, the total message size is checked against `u32::MAX`. Overflow
produces `SerError::MessageTooLarge`. The check runs in all build profiles
(debug and release).

For payloads that approach 4 GiB, split data across multiple messages using
`Writer`. Each message's buffer is independent.

### Consequences

- **Positive:** Compact 4-byte inline stubs for all variable-length types.
  Reduces message size and improves cache locality compared to 8-byte stubs.
- **Positive:** Enables the `0xFFFFFFFF` None sentinel — a simple, unambiguous
  encoding for `Option::None` that requires no additional tag byte.
- **Positive:** Files can be arbitrarily large via u64 index offsets.
- **Negative:** Individual messages cannot exceed 4 GiB. Acceptable for
  pardosa-genome's use case (event-sized messages, not bulk data blobs).
- **Negative:** Near-4-GiB messages waste memory on the serialization buffer.
  Guidance: split into multi-message files.

### References

- `format.rs` — `NONE_SENTINEL: u32 = 0xFFFF_FFFF`
- `error.rs` — `SerError::MessageTooLarge`
- genome.md §Limitations (4 GiB per-message limit), §Binary Format (offset semantics)
