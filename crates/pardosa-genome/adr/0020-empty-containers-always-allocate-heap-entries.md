# ADR-020: Empty containers always allocate heap entries — offset 0 is never a sentinel

**Status:** Accepted

### Context

Binary formats that use offsets to reference variable-length data must define the
semantics of offset 0. Two approaches exist:

1. **Offset 0 as sentinel:** Offset 0 means "empty" or "null." Eliminates heap
   allocation for empty containers but creates ambiguity: does offset 0 mean
   "empty string" or "data at position 0"?

2. **Offset 0 as data:** Offset 0 means "data at position 0 in the heap." Empty
   containers are allocated as normal heap entries with `len = 0` or `count = 0`.
   No ambiguity.

pardosa-genome already uses `0xFFFFFFFF` as the `Option::None` sentinel
(ADR-017). Adding a second sentinel (0 for "empty") doubles the special-case
logic in the deserializer.

### Decision

**Offset 0 always means "data at position 0."** It is never overloaded as a
sentinel. The only sentinel value in the entire format is `0xFFFFFFFF`
(`Option::None`).

Empty containers always allocate a heap entry:

- **Empty string (`""`):** Heap entry `[len:u32 = 0x00000000]`. Offset points
  to the 4-byte length prefix. Zero bytes of string data follow.
- **Empty byte slice:** Same as empty string.
- **Empty `Vec<T>`:** Heap entry `[count:u32 = 0x00000000]`. Offset points to
  the 4-byte count prefix. Zero elements follow.
- **Empty `BTreeMap<K, V>`:** Same as empty Vec — `[count:u32 = 0x00000000]`.

This means every string, byte slice, vec, and map — regardless of length —
produces a valid heap entry with a forward-pointing offset. The deserializer has
no special case for empty containers; it reads the length/count prefix and
proceeds with zero iterations.

**Edge case — `Option<()>` with `Some(())`:** The offset points to a valid
position in the buffer, but zero bytes are read (unit type has 0 inline size).
The offset may point to `buf.len()` — a 0-byte read at the end of the buffer
is valid.

### Consequences

- **Positive:** Eliminates offset-0 ambiguity. Hex dumps and debugging tools
  can treat every non-`0xFFFFFFFF` offset as a real data pointer.
- **Positive:** Simplifies the deserializer — no special case for empty
  containers. All containers follow the same `read-offset → read-count →
  iterate` pattern.
- **Positive:** Single sentinel value (`0xFFFFFFFF`) for the entire format.
  Easy to document, easy to audit.
- **Negative:** Empty containers cost 4 bytes of heap space (the length/count
  prefix). Negligible: 4 bytes per empty container in a format that already
  uses 4-byte offset stubs.
- **Negative:** Slightly larger output for messages with many empty containers.
  Compression (ADR-014) eliminates this overhead in practice.

### References

- `format.rs` — `NONE_SENTINEL: u32 = 0xFFFF_FFFF`
- ADR-017 (4 GiB limit, None sentinel correctness)
- genome.md §Binary Format (offset semantics, empty container rules)
