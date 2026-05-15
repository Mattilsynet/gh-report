# ADR-005: Two-pass serialization architecture

**Status:** Accepted (designed, not yet implemented — Phase 2)

### Context

Fixed-layout formats need to write inline scalar data and variable-length heap data
(strings, vecs, maps, option payloads) into a single contiguous buffer. FlatBuffers
uses a builder with back-patching. An intermediate AST approach allocates proportionally
to the data. A two-pass approach avoids both: first pass computes sizes, second pass
writes with exact pre-allocation.

### Decision

Serialization uses two `serde::Serializer` implementations that share the same
structural logic:

1. **Pass 1 — `SizingSerializer`**: Walks the type tree computing total buffer size
   (inline + heap). Cost: one serde traversal, `O(depth)` stack frames, zero heap
   allocation.
2. **Pass 2 — `WritingSerializer`**: Pre-allocates the exact buffer. Writes inline data
   from position 0, appends heap items in breadth-first order.

A sizing mismatch between passes produces `SerError::InternalSizingMismatch` in all
build profiles, with an additional `debug_assert!` in debug builds.

### Consequences

- **Positive:** Peak memory ≈ 1× final message size. No intermediate tree, no
  back-patching, no reallocation.
- **Positive:** Breadth-first heap ordering ensures all offsets are forward-pointing.
- **Negative:** `value.serialize()` is called twice. Safe for any correct `Serialize`
  impl (immutable observation), but types with interior mutability during serialization
  (extremely rare) could produce inconsistent sizing. The sizing mismatch check catches
  this.
- **Negative:** Write throughput is lower than a single-pass approach due to two
  traversals; not yet benchmarked. Acceptable tradeoff for read-optimized format.

### References

- genome.md §Serialization Algorithm, §Two-Pass Architecture, §Memory Usage
