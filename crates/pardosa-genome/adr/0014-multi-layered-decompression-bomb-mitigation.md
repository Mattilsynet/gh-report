# ADR-014: Multi-layered decompression bomb mitigation

**Status:** Accepted

### Context

Zstd decompression is a denial-of-service vector. A few kilobytes of crafted
compressed data can expand to terabytes, exhausting memory. Real-world CVEs
demonstrate this attack class:

- **Movement Network** (2024): crafted zstd frame caused unbounded allocation.
- **urllib3** (CVE-2024-37891): decompression bomb via Content-Encoding.
- **OTel Collector** (2024): crafted zstd payload caused OOM in telemetry pipeline.

The zstd specification allows window sizes up to 3.75 TiB — a single decompressor
can consume terabytes of memory if window size is not capped.

A single-layer defense (e.g., only checking output size) is insufficient because
the attacker controls both the compressed payload and the size headers.

### Decision

Implement four independent defense layers, each checked **before** the allocation
it guards:

| Layer | What it guards | Check | Error |
|-------|---------------|-------|-------|
| 1 | Output buffer allocation | `msg_data_size` vs `max_uncompressed_size` | `DeError::UncompressedSizeTooLarge` |
| 2 | Output buffer allocation | Zstd `Frame_Content_Size` vs `max_uncompressed_size` | `DeError::UncompressedSizeTooLarge` |
| 3 | Decompressor memory | `ZSTD_d_maxWindowLog` capped at 22 (4 MiB) | `DeError::DecompressionFailed` |
| 4 | Post-decompression | Decompressed size matches `msg_data_size` | `DeError::PostDecompressionTrailingBytes` |

**Layer 1 — `msg_data_size` pre-check:** The uncompressed size field in the
message header is validated against `DecodeOptions::max_uncompressed_size` (default
256 MiB) before allocating the decompression buffer. Rejects crafted
`msg_data_size = u32::MAX` without allocation.

**Layer 2 — `Frame_Content_Size` validation:** The zstd frame header's
`Frame_Content_Size` field (when present) is validated against
`max_uncompressed_size` before allocation. Cross-checks layer 1 using the
decompressor's own metadata.

**Layer 3 — Window log cap:** `ZSTD_d_maxWindowLog` is set to 22 (4 MiB) to
bound decompressor working memory. Zstd frames requesting larger windows (up to
3.75 TiB per spec) are rejected before decompression begins.

**Layer 4 — Post-decompression verification:** After decompression, the actual
decompressed buffer size is verified against `msg_data_size`. Mismatches produce
`DeError::BufferTooSmall` or `DeError::PostDecompressionTrailingBytes`.

**Content size is always written** into the zstd frame header during compression
(`ZSTD_c_contentSizeFlag = 1`), ensuring layer 2 is always available for
pardosa-genome-produced messages.

Total decompressor memory per message: ~4 MiB (capped window) + output buffer
size.

### Consequences

- **Positive:** Four independent layers — compromising one layer does not bypass
  the others.
- **Positive:** All pre-allocation checks run before any memory-intensive operation.
- **Positive:** Configurable via `DecodeOptions` (`max_uncompressed_size`,
  `max_zstd_window_log`) for different deployment contexts.
- **Negative:** Legitimate zstd frames with large window sizes (>4 MiB) are
  rejected by default. Callers can raise `max_zstd_window_log` if needed.
- **Negative:** `Frame_Content_Size` is optional in the zstd spec. Third-party
  producers may omit it, disabling layer 2. Layers 1, 3, 4 still protect.

### References

- `config.rs` — `DecodeOptions::max_uncompressed_size`, `max_zstd_window_log`
- `error.rs` — `DeError::UncompressedSizeTooLarge`, `DecompressionFailed`,
  `PostDecompressionTrailingBytes`
- genome.md §Compression, §Deserialization Limits
- ADR-013 (page-class resource limits — complementary defense)
