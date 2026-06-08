# GEN-0030. Zstd-Only Compression in v1

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: D
Status: Accepted

## Related

References: GEN-0001

## Context

pardosa-genome supports optional compression via `compression_algo`. Zstd dominates brotli for real-time workloads: ~4× faster decompression, ~25× faster compression at comparable ratios. The file header reserves 3 bits (7 values) for the algorithm, ensuring brotli can be added later without a format version bump.

## Decision

v1 supports **zstd only**. Brotli is deferred to a future version.

**Compression algorithm codes:**
- `0x00` — no compression
- `0x01` — zstd
- `0x02`–`0x07` — reserved (reader must reject with
  `FileError::UnsupportedCompression`)

**Default zstd level:** 3 (best speed/ratio tradeoff for real-time workloads).
Levels 1–22 are supported.

**Feature gating:** Zstd is behind the `zstd` Cargo feature flag (`default =
["std", "derive"]` — zstd is opt-in). When the `zstd` feature is not enabled,
files with `compression_algo = 0x01` produce
`FileError::CompressionNotAvailable`.

### Rationale

The primary use case is real-time network and MLOps pipelines where
decompression speed dominates. Zstd's ≈4× faster decompression and ≈25×
faster compression than brotli at comparable ratios makes it the clear choice
for this workload. Brotli's marginally better compression ratio does not
justify the decompression speed penalty for real-time consumers.

The file header reserves 3 bits for the compression algorithm (7 possible
values), ensuring brotli can be added in a future version without a format
version bump. v1 readers reject unknown algorithm codes, ensuring clean
forward compatibility.

R1 [9]: v1 supports zstd only — Brotli is deferred to a future version
R2 [9]: Zstd is behind the zstd Cargo feature flag and is opt-in —
  not included in default features
R3 [9]: Reserved algorithm codes 0x02 through 0x07 must be rejected
  with FileError::UnsupportedCompression

## Consequences

- Single compression dependency reduces testing surface. One decompression path to audit.
- Zstd dictionary support enables future 2–5× improvement on small messages.
- Reserved algorithm codes ensure brotli can be added without breaking the wire format.
- Cold storage workloads cannot use brotli's better ratios until a future version.
- `no_std` consumers needing decompression must use `ruzstd` (experimental).
