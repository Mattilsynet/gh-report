# GEN-0014. Multi-Layered Decompression Bomb Mitigation

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Accepted

## Related

References: GEN-0013, GND-0001, GND-0005

## Context

Zstd decompression is a denial-of-service vector — crafted compressed data can expand to terabytes, exhausting memory (CVEs in Movement Network, urllib3, OTel Collector). The zstd spec allows window sizes up to 3.75 TiB. A single-layer defense is insufficient because the attacker controls both the compressed payload and the size headers.

## Decision

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

R1 [5]: Implement four independent defense layers each checked before
  the allocation it guards
R2 [5]: ZSTD_d_maxWindowLog is capped at 22 (4 MiB) to bound
  decompressor working memory
R3 [6]: Content size is always written into the zstd frame header
  during compression ensuring layer 2 is always available

## Consequences

- **Positive:** Four independent layers — compromising one layer does not bypass
  the others.
- **Positive:** All pre-allocation checks run before any memory-intensive operation.
- **Positive:** Configurable via `DecodeOptions` (`max_uncompressed_size`,
  `max_zstd_window_log`) for different deployment contexts.
- **Negative:** Legitimate zstd frames with large window sizes (>4 MiB) are
  rejected by default. Callers can raise `max_zstd_window_log` if needed.
- **Negative:** `Frame_Content_Size` is optional in the zstd spec. Third-party
  producers may omit it, disabling layer 2. Layers 1, 3, 4 still protect.
