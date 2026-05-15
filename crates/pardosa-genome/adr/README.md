# pardosa-genome — Architecture Decision Records

This directory records the architectural decisions in pardosa-genome,
verified against the implementation (April 2025). Each entry follows the
[Michael Nygard ADR format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions).

## Index

| ADR | Decision | Status | Implementation |
|-----|----------|--------|---------------|
| [001](0001-serde-native-serialization-with-genomesafe-marker-trait.md) | Serde-native + GenomeSafe marker | Accepted | Phase 1 ✅ |
| [002](0002-no-schema-evolution-fixed-layout.md) | No schema evolution / fixed layout | Accepted | Phase 1 (format) ✅, Phase 2 (ser/de) pending |
| [003](0003-compile-time-xxhash64-schema-hashing.md) | xxHash64 schema hashing | Accepted (amended) | Phase 1 ✅, stability docs added |
| [004](0004-reject-non-deterministic-types-and-serde-attrs.md) | Compile-time type/attr rejection | Accepted (amended) | Phase 1 ✅, 3 bugs fixed |
| [005](0005-two-pass-serialization-architecture.md) | Two-pass serialization | Accepted | Phase 2 pending |
| [006](0006-zero-copy-deserialization-with-forbid-unsafe-code.md) | Zero-copy + forbid(unsafe_code) | Accepted | Phase 2 pending |
| [007](0007-flatbuffers-style-offset-based-binary-layout.md) | Offset-based binary layout | Accepted | Phase 2 pending |
| [008](0008-transport-agnostic-core-with-companion-crate-separation.md) | Transport-agnostic core | Accepted | Phase 1 ✅ |
| [009](0009-one-schema-per-file-with-embedded-schema-source.md) | One schema per file + embedded source | Accepted | Phase 1 (source gen) ✅, Phase 3 (writer) pending |
| [010](0010-std-only-for-now-no-std-deferred.md) | std-only, no_std deferred | Accepted (amended) | Phase 1 ✅, claims removed |
| [011](0011-inline-verification-check-catalog.md) | Inline verification check catalog | Accepted | Phase 1 (error types) ✅, Phase 2 (checks) pending |
| [012](0012-little-endian-wire-encoding-no-pointer-casts.md) | LE wire encoding, no pointer casts | Accepted | Phase 2 pending |
| [013](0013-page-class-dos-protection.md) | Page-class DoS protection | Accepted | Phase 1 (config) ✅, Phase 2 (enforcement) pending |
| [014](0014-multi-layered-decompression-bomb-mitigation.md) | Decompression bomb mitigation | Accepted | Phase 3 pending |
| [015](0015-forward-compatibility-contract.md) | Forward compatibility contract | Accepted | Phase 1 (format) ✅ |
| [016](0016-xxhash64-for-file-integrity-checksums.md) | xxHash64 for file integrity | Accepted | Phase 1 (constants) ✅, Phase 3 (writer/reader) pending |
| [017](0017-4gib-per-message-limit-u32-offsets.md) | 4 GiB per-message limit | Accepted | Phase 1 (error/sentinel) ✅ |
| [018](0018-non-zero-padding-is-hard-error.md) | Non-zero padding is hard error | Accepted | Phase 1 (error) ✅, Phase 2 (enforcement) pending |
| [019](0019-box-arc-hash-transparency-rc-exclusion.md) | Box/Arc transparency, Rc excluded | Accepted | Phase 1 ✅ |
| [020](0020-empty-containers-always-allocate-heap-entries.md) | Empty containers allocate heap entries | Accepted | Phase 2 pending |
| [021](0021-breadth-first-heap-ordering.md) | Breadth-first heap ordering | Accepted | Phase 2 pending |
| [022](0022-externally-tagged-enums-discriminant-offset-encoding.md) | Externally tagged enum encoding | Accepted | Phase 2 pending |
| [023](0023-i128-u128-alignment-capped-at-8-bytes.md) | i128/u128 alignment capped at 8 bytes | Accepted | Phase 2 pending |
| [024](0024-nan-bit-pattern-preservation-no-canonicalization.md) | NaN bit-pattern preservation | Accepted | Phase 2 pending |
| [025](0025-bare-messages-structural-validation-only.md) | Bare messages: structural validation only | Accepted | Phase 2 pending |
| [026](0026-no-format-auto-detection-bare-vs-file.md) | No format auto-detection | Accepted | Phase 1 (API) ✅ |
| [027](0027-full-serde-data-model-ron-algebraic-types.md) | Full serde data model support | Accepted | Phase 1 (derive) ✅, Phase 2 (ser/de) pending |
| [028](0028-tuple-struct-tuple-wire-equivalence.md) | Tuple struct / tuple wire equivalence | Accepted | Phase 2 pending |
| [029](0029-reject-serde-default-at-compile-time.md) | Reject #[serde(default)] at compile time | Accepted | Phase 1 ✅ |
| [030](0030-zstd-only-compression-in-v1.md) | Zstd-only compression in v1 | Accepted | Phase 3 pending |
| [031](0031-rust-only-cross-language-read-deferred.md) | Rust-only in v1, cross-language deferred | Accepted | Phase 1 ✅ |
| [032](0032-canonical-encoding-contract.md) | Canonical encoding contract | Accepted | Phase 1 ✅ |
| [033](0033-genome-ord-marker-trait-for-map-keys.md) | GenomeOrd marker trait for map keys | Accepted | Phase 1 ✅ |

---

*Provenance: ADRs 001–010 were split from a single `adr.md` file. Last commit of the original: `8b9688e`. ADRs 011–020 added April 2026. ADRs 021–033 added April 2026.*
