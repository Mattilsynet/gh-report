# GEN-0016. xxHash64 for File Integrity Checksums

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0003

## Context

pardosa-genome files contain per-message and footer checksums for corruption detection. xxHash64 is already a dependency for compile-time schema hashing (GEN-0003). It is faster than CRC32 on modern hardware with excellent distribution. Using the same hash family eliminates a separate `crc` dependency.

## Decision

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

**File footer layout (32 bytes):**

```
Offset  Size  Field           Description
------  ----  --------------  ------------------------------------------
 0      8     index_offset    Absolute file offset to message index (u64)
 8      8     message_count   Number of messages (u64)
16      4     reserved        Must be all zeros
20      4     footer_magic    ASCII "PGNO" (validation sentinel)
24      8     checksum        xxHash64 of footer bytes [0..24) (u64 LE)
```

**Seed:** 0 (same as schema hashing — frozen, see GEN-0003 stability contract).

**Coverage:** Per-message checksum covers the stored record bytes (from
`msg_data_size` through end of data/compressed_data). Footer checksum covers
footer bytes `[0..24)`. Both are mandatory and always verified before
deserialization.

**Threat model:** xxHash64 is a non-cryptographic hash. Like CRC32, it detects
accidental corruption (disk errors, truncation, bit rot) but provides zero
protection against intentional tampering. For tamper detection, use
transport-level integrity (TLS/QUIC) or a future AEAD extension (v2).

R1 [9]: Use xxHash64 for all file-level integrity checksums including
  per-message and footer checksums
R2 [9]: Never truncate a hash — all checksums use the full 64-bit
  xxHash64 output
R3 [9]: Both per-message and footer checksums are mandatory and always
  verified before deserialization

## Consequences

- Eliminates `crc` dependency — single hash algorithm for schema and integrity.
- Faster than CRC32 on modern CPUs (~30 vs ~5 GB/s). Better distribution at 64 bits.
- Index entry grows from 16 to 24 bytes (~50%). Negligible: 1,000 messages add 8 KiB.
- Same non-cryptographic tamper-detection limitation as CRC32.
