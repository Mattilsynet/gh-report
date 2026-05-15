# GEN-0006. Zero-Copy Deserialization with forbid(unsafe_code)

Date: 2026-04-25
Last-reviewed: 2026-04-25
Tier: A
Status: Accepted

## Related

References: GEN-0001, CHE-0007

## Context

rkyv achieves zero-copy via `unsafe` pointer reinterpretation, requiring careful
safety proofs. FlatBuffers' Rust bindings use `unsafe` for alignment-sensitive reads.
pardosa-genome aims for zero-copy `&'de str` and `&'de [u8]` without any `unsafe`.

## Decision

All reads use `from_le_bytes` on byte slices — no pointer casts, no alignment
requirements on the input buffer. String deserialization returns
`visitor.visit_borrowed_str` pointing directly into the input buffer (serde's standard
zero-copy mechanism). The crate uses `#![forbid(unsafe_code)]`.

Every deserialization performs the full verification suite: bounds checks on all offsets,
UTF-8 validation, char/bool value validation, padding zero checks, backward-offset
rejection. There is no unverified `decode` path — verification adds modest overhead
(not yet benchmarked) and is branch-predicted away on well-formed input.

R1 [4]: The crate uses forbid(unsafe_code) — all reads use
  from_le_bytes on byte slices with no pointer casts
R2 [4]: Every deserialization performs the full verification suite with
  no unverified decode path
R3 [4]: Zero-copy deserialization applies to strings and byte slices
  via serde's visit_borrowed_str mechanism

## Consequences

- **Positive:** No `unsafe` — soundness is guaranteed by the compiler. No safety proofs
  to maintain.
- **Positive:** Always-verify eliminates the footgun of accidentally using an unverified
  decode path on untrusted data.
- **Negative:** Cannot zero-copy deserialize structs-as-a-whole (rkyv can). Only
  strings and byte slices get zero-copy. Struct fields are read individually.
- **Negative:** `from_le_bytes` has marginal overhead vs. pointer casts on LE platforms.
  Negligible in practice.
- **Tradeoff:** Compressed messages require `DeserializeOwned` (decompression allocates
  a new buffer). Zero-copy only applies to uncompressed messages.
