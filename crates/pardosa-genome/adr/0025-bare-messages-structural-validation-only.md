# ADR-025: Bare messages — structural validation only, no data integrity checksums

**Status:** Accepted

### Context

pardosa-genome defines two wire formats: bare messages (`encode`/`decode`) and
file format (`Writer`/`Reader`). The file format includes per-message xxHash64
checksums in the message index (ADR-016) and a footer checksum, providing
corruption detection for all stored data.

Bare messages have no equivalent integrity mechanism. This is a deliberate
security boundary decision that trades compactness for resilience.

### Decision

Bare messages provide **structural validation** but no **data integrity
checksums**.

**Structural validation present (ADR-011):**
- Schema hash verification (type mismatch detection)
- Format version check
- All u32 offset bounds checks with overflow-safe arithmetic
- UTF-8 validation on string data
- Char Unicode scalar validation
- Bool value validation (0x00 or 0x01 only)
- Padding zero enforcement (non-zero padding = hard error)
- Backward offset rejection
- Trailing bytes rejection (default on)

**Data integrity absent:**
- No checksum covering the message bytes. A bit flip in a scalar field (e.g.,
  `u32` value changing from 42 to 43) that does not violate any structural
  invariant produces silently wrong data.
- No HMAC or signature. Bare messages provide zero tamper detection.

**Design rationale:** Bare messages are designed for transport-protected
channels (TLS, QUIC, JetStream, Unix domain sockets). These transports
already provide data integrity. Adding a checksum to bare messages would
increase message size by 8 bytes and add computation overhead for redundant
protection.

### Consequences

- **Positive:** Minimal bare message overhead. No checksum computation on
  encode or verification on decode.
- **Positive:** Transport-agnostic — the format does not duplicate integrity
  mechanisms provided by the transport layer.
- **Negative:** Applications using bare messages on transports without
  integrity (raw TCP without TLS, UDP, shared memory IPC) get zero bit-flip
  detection for scalar values. A single bit flip in a numeric field is
  undetectable.
- **Negative:** Bare messages over untrusted channels without TLS/QUIC are
  vulnerable to silent data corruption from network errors.
- **Mitigation:** Use transport-level integrity (TLS, QUIC) for bare messages.
  Use the file format (with xxHash64 checksums) for persistent storage.
- **Future:** genome.md §Future Scope defines an optional bare message
  checksum trailer (`encode_checksummed` / `decode_checksummed`) behind a
  `checksum` feature flag for v2.

### References

- ADR-011 (inline verification check catalog — structural checks)
- ADR-016 (xxHash64 for file integrity checksums)
- genome.md §Safety Contract: "Bare messages have no integrity mechanism"
- genome.md §Future Scope: bare message checksum trailer
- `format.rs` — bare message header constants (no checksum field)
