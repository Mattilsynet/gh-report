# GEN-0017. 4 GiB Per-Message Limit — u32 Offsets

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: B
Status: Superseded by GEN-0035

## Retirement

Superseded-by: GEN-0035
Moved-to-stale: 2026-05-15
Reason: The 4 GiB per-message cap and the 0xFFFFFFFF None sentinel
were properties of the offset-based layout. GEN-0035 uses a
configurable decoder cap (default 1 MiB) and a tag-byte Option
encoding (0u8 None / 1u8 Some); neither sentinel survives.
