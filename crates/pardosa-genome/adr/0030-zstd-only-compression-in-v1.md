# ADR-030: Zstd-only compression in v1 — brotli deferred

**Status:** Accepted

### Context

pardosa-genome's file format and bare messages support optional compression
via the `compression_algo` field (3 bits in the file header flags, 1-byte
`algo` field in bare messages). ADR-014 documents the decompression bomb
mitigation strategy. This ADR documents the algorithm selection decision.

Two compression algorithms were evaluated for v1:

| Property | Zstd | Brotli |
|----------|:----:|:------:|
| Decompression speed | ~1500 MB/s | ~400 MB/s |
| Compression speed (level 3) | ~400 MB/s | ~15 MB/s |
| Compression ratio | 50–60% | 55–65% |
| Dictionary support | Yes (built-in) | Yes (less common) |
| Rust crate maturity | `zstd` 0.13 (stable, well-maintained) | `brotli` 6.0 (stable) |
| `no_std` decompression | `ruzstd` (pure Rust, experimental) | `brotli-decompressor` |
| Window size attack surface | Up to 3.75 TiB (CVE risk) | Up to 16 MiB |

### Decision

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

### Consequences

- **Positive:** Single compression dependency reduces testing surface and
  build complexity. Only one decompression code path to audit for security
  (ADR-014).
- **Positive:** Zstd's dictionary support (reserved `dict_id` in file header)
  enables future 2–5× compression improvement on small messages without
  algorithm change.
- **Positive:** Reserved algorithm codes ensure brotli (or other algorithms)
  can be added without breaking the wire format.
- **Negative:** Cold storage workloads where write speed is unimportant
  cannot use brotli's marginally better ratios until a future version.
- **Negative:** `no_std` consumers who need decompression must use `ruzstd`
  (experimental) or implement their own decompressor.

### References

- ADR-014 (multi-layered decompression bomb mitigation)
- genome.md §Compression: algorithm comparison, zstd configuration
- genome.md §Future Scope: brotli compression, `ruzstd` for `no_std`
- `config.rs` — `Compression` enum: `None`, `Zstd { level }` (cfg-gated)
- `format.rs` — `ALGO_NONE: u8 = 0x00`, `ALGO_ZSTD: u8 = 0x01`
