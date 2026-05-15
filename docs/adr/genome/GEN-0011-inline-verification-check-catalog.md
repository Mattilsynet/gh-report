# GEN-0011. Inline Verification Check Catalog

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: GEN-0006, GND-0005

## Context

Binary deserialization formats face a fundamental API design question: should
verification be a separate step (`verify` then `decode`), an opt-in mode
(`decode_unchecked` vs `decode_verified`), or inline during every deserialization?

Separate verification invites "forgot to verify" bugs — the most dangerous class
of deserialization vulnerability. Opt-in modes create two code paths with different
safety properties, making it easy for callers to choose the wrong one under
performance pressure.

GEN-0006 establishes the always-verify philosophy and `#![forbid(unsafe_code)]`.
This ADR catalogs the specific 20 structural checks that constitute "verification"
and defines their error semantics.

## Decision

Every call to `decode` or `decode_with_options` performs all 20 structural checks
inline during deserialization. There is no `decode_unchecked`, no standalone
`verify` function, and no way to skip individual checks. Verification adds overhead
(not yet benchmarked) that is branch-predicted away on well-formed input.

The 20 checks, performed on every deserialization:

| # | Check | Error |
|---|-------|-------|
| 1 | Format version matches expected | `DeError::VersionMismatch` |
| 2 | Schema hash matches target type | `DeError::SchemaMismatch` |
| 3 | Buffer length >= minimum for root type | `DeError::BufferTooSmall` |
| 4 | All u32 offsets within `[0..buf.len()]` (excl. None sentinel) | `DeError::OffsetOutOfBounds` |
| 5 | All `offset + len` computations do not overflow | `DeError::OffsetOverflow` |
| 6 | Backward offset check: offsets must point into heap region | `DeError::BackwardOffset` |
| 7 | String data is valid UTF-8 | `DeError::InvalidUtf8` |
| 8 | Char values are valid Unicode scalars | `DeError::InvalidChar` |
| 9 | Bool values are `0x00` or `0x01` | `DeError::InvalidBool` |
| 10 | Alignment padding bytes are `0x00` | `DeError::NonZeroPadding` |
| 11 | Message size matches `msg_data_size` (bare) | `DeError::TrailingBytes` |
| 12 | Trailing bytes rejected (default on) | `DeError::TrailingBytes` |
| 13 | File header magic, version, footer magic | `FileError::InvalidMagic` |
| 14 | Per-message xxHash64 in index entries | `DeError::ChecksumMismatch` |
| 15 | Post-decompression size matches `msg_data_size` | `DeError::PostDecompressionTrailingBytes` |
| 16 | Reserved bytes in header/footer are all zeros | `DeError::NonZeroPadding` |
| 17 | `compressed_size` bounds-checked | `DeError::BufferTooSmall` |
| 18 | Index validation: offsets >= 32, monotonic, non-overlapping | `FileError::InvalidIndex` |
| 19 | All offset arithmetic widened to u64 (32-bit safety) | Prevents overflow |
| 20 | `message_count * entry_size` overflow check | `FileError::InvalidIndex` |

Enum unit variant offset fields are treated as padding (must be `0x00000000`) and
are exempt from the backward offset check (#6).

R1 [5]: Every call to decode performs all 20 structural checks inline
  during deserialization with no way to skip individual checks
R2 [5]: There is no decode_unchecked, no standalone verify function,
  and no opt-out mechanism for verification
R3 [6]: Each check maps to a specific DeError or FileError variant for
  unambiguous error reporting

## Consequences

- **Positive:** Eliminates the "forgot to verify" bug class entirely. No API
  misuse can produce unverified deserialization.
- **Positive:** Single code path for all callers — no security-critical branching.
- **Negative:** Modest overhead on well-formed input (not yet benchmarked) that
  cannot be opted out of. Acceptable for pardosa-genome's use case (event storage,
  not hot-path game rendering).
- **Negative:** Adding a new check requires updating this catalog and the
  `DeError`/`FileError` enums.
