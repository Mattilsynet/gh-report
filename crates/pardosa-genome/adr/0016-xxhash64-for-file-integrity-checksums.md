# ADR-016: xxHash64 for file integrity checksums

**Status:** Accepted

### Context

pardosa-genome files contain per-message and footer checksums for corruption
detection. The choice of checksum algorithm affects error detection quality,
performance, and dependency count.

xxHash64 is already a dependency (`xxhash-rust` crate with `const_xxh64` feature)
for compile-time schema hashing (ADR-003). It is significantly faster than CRC32
on modern hardware and provides excellent distribution properties for error
detection.

Using the same hash family for both schema hashing and integrity checking
eliminates the need for a separate `crc` crate dependency.

### Decision

Use xxHash64 (via `xxhash-rust`) for all file-level integrity checksums:
per-message checksums in the message index and the footer checksum.

**Principle:** Never truncate a hash. All checksums use the full 64-bit xxHash64
output.

**Message index entry layout (24 bytes):**

```
Offset  Size  Field       Description
------  ----  ----------  ------------------------------------------
 0      8     offset      Absolute file offset to message (u64 LE)
 8      4     size        Total stored record size in bytes (u32 LE)
12      4     reserved    Must be zero (alignment padding)
16      8     checksum    xxHash64 of stored record bytes (u64 LE)
```

**File footer layout (32 bytes, unchanged total size):**

```
Offset  Size  Field           Description
------  ----  --------------  ------------------------------------------
 0      8     index_offset    Absolute file offset to message index (u64)
 8      8     message_count   Number of messages (u64)
16      4     reserved        Must be all zeros
20      4     footer_magic    ASCII "PGNO" (validation sentinel)
24      8     checksum        xxHash64 of footer bytes [0..24) (u64 LE)
```

Footer total size remains 32 bytes. Reserved space shrinks from 8 bytes to
4 bytes; checksum grows from 4 bytes to 8 bytes.

**Seed:** 0 (same as schema hashing — frozen, see ADR-003 stability contract).

**Coverage:** Per-message checksum covers the stored record bytes (from
`msg_data_size` through end of data/compressed_data). Footer checksum covers
footer bytes `[0..24)`. Both are mandatory and always verified before
deserialization.

**Threat model:** xxHash64 is a non-cryptographic hash. Like CRC32, it detects
accidental corruption (disk errors, truncation, bit rot) but provides zero
protection against intentional tampering. An adversary who controls the file can
recompute checksums for arbitrary modifications. For tamper detection, use
transport-level integrity (TLS/QUIC) or a future AEAD extension (v2).

### Consequences

- **Positive:** Eliminates `crc` crate dependency — xxHash64 already present
  for schema hashing.
- **Positive:** xxHash64 is faster than CRC32 on modern CPUs (~30 GB/s vs
  ~5 GB/s without hardware CRC acceleration).
- **Positive:** Better error detection distribution than CRC32 for the same
  bit width. Full 64-bit output: birthday bound ~4 billion messages before
  accidental collision.
- **Positive:** Single hash algorithm for the entire crate (schema + integrity).
- **Negative:** Index entry size increases from 16 to 24 bytes (~50% increase).
  Negligible in practice: 1,000 messages add 24 KiB of index vs 16 KiB.
- **Negative:** xxHash64 is not hardware-accelerated on most platforms (unlike
  CRC32 on x86 with SSE4.2). But xxHash64's software speed exceeds CRC32's
  hardware speed on modern out-of-order CPUs.
- **Negative:** Same tamper-detection limitation as CRC32 — non-cryptographic.

### References

- ADR-003 (xxHash64 schema hashing, stability contract, seed = 0)
- `format.rs` — `INDEX_ENTRY_SIZE`, footer constants
- `error.rs` — `DeError::ChecksumMismatch`, `FileError::InvalidChecksum`
- genome.md §Message Index, §File Footer, §Integrity boundaries
