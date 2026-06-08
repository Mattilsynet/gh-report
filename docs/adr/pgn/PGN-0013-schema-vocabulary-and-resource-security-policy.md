# PGN-0013. Schema Vocabulary and Resource Security Policy

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa-wire, pardosa-schema, pardosa-file, pardosa-derive

## Related

References: PGN-0003, PGN-0004, PGN-0006, GEN-0041, GEN-0042, GEN-0013, GEN-0014

## Context

Sources inherit GEN-0042 (bounded-wrapper invariant set), GEN-0041 (foreign-floor allowlist), GEN-0034 (codec fuzzing), GEN-0013 (page-class resource limits), and GEN-0014 (decompression-bomb mitigation). The PGN consolidation kept PGN-0003 / PGN-0004 / PGN-0006 silent on the bounded vocabulary, the foreign-type allowlist, the fuzz mandate, and the resource-limit dial: each is currently authoritative only in GEN. With PAR/GEN retirement deferred (PGN-0001 risks/migration), this ADR re-anchors those decisions inside the PGN tree without choosing new limits — GEN values remain canonical and are inherited verbatim. Crate-name updates (`pardosa-wire`/`pardosa-schema` versus GEN's `pardosa-encoding`/`pardosa-traits`) reflect the workspace today.

## Decision

The bounded-wrapper invariants of GEN-0042, the foreign-type allowlist of GEN-0041, the codec-fuzzing mandate of GEN-0034, the page-class resource limits of GEN-0013, and the decompression-bomb caps of GEN-0014 are inherited verbatim into the PGN domain under the crate names current to this workspace. No new limit, allowlist member, or fuzzing target is introduced. Current Rust implements only `uuid::Uuid` from the GEN-0041 floor (`pardosa-wire/src/foreign.rs`, `pardosa-schema/src/genome_safe.rs`); `bytes::Bytes` and `arrayvec::ArrayVec` are policy-permitted but not implemented and ship behind future feature gates per GEN-0041 R4.

R1 [5]: The bounded wrappers `EventString<MAX>`, `EventBytes<MAX>`,
  `EventVec<T, MAX>`, and `NonEmptyEventString<MAX>` ship in
  `pardosa-schema::bounded` with the full sealed-trait stack
  (`Sealed`, `EventSafe`, `Encode`, `Decode`, `Validate`) and
  inherit the GEN-0042 R1–R7 invariants unchanged.
R2 [5]: The foreign-type allowlist is exactly `uuid::Uuid`,
  `bytes::Bytes`, and `arrayvec::ArrayVec<T, N>`; impls live behind
  feature gates `uuid`, `bytes`, `arrayvec` per GEN-0041 R4.
  Today only `uuid::Uuid` is implemented in this workspace; `Bytes`
  and `ArrayVec` are policy-permitted and unshipped.
R3 [5]: No foreign type joins the allowlist without a successor PGN
  ADR keyed on GEN-0041's S1 (byte-shape conformance) and S2
  (post-decode validity) invariants, naming the wire shape and the
  capacity-check surface for any type carrying a runtime length.
R4 [5]: Codec integrity is exercised by structured fuzzing per
  GEN-0034: round-trip and decode-from-arbitrary corpora cover every
  `Encode`/`Decode` impl in `pardosa-wire` and every bounded wrapper
  in `pardosa-schema`; a new impl ships paired with a fuzz target.
R5 [5]: Per-message decoded-payload size is bounded by
  `ReaderOptions::max_decompressed_message_bytes` (PGN-0004 R2);
  the page-class resource-limit dial of GEN-0013 is the source of
  truth for the default value, inherited by `pardosa-file` without
  rechoice in this ADR.
R6 [5]: Decompression-bomb mitigation follows GEN-0014: the reader
  validates the stored (post-compression) `xxh64` body checksum
  before invoking the decompressor, and bounds the decompressed
  output by `max_decompressed_message_bytes` — the substrate never
  allocates a decompressed buffer larger than the cap.
R7 [5]: No bounded wrapper, foreign-type impl, fuzz target, or
  resource-limit change introduces a new `EventError`/`PardosaError`
  variant (GEN-0042 R7, GEN-0041 R5); the taxonomy frozen by
  PGN-0006 is the sole failure surface.

## Consequences

+ becomes easier: locating bounded-vocabulary, foreign-type, fuzzing,
  and resource-limit policy inside the active PGN tree without
  chasing GEN ADRs scheduled for retirement; pairing every new
  `Encode`/`Decode` impl with a fuzz target as the acceptance gate;
  bounding decoded-payload allocation per field rather than only at
  the substrate cap.
− becomes harder: shipping a foreign type outside the GEN-0041
  floor (successor PGN ADR required); raising
  `max_decompressed_message_bytes` without a page-class rationale;
  widening `EventError` to carry a new failure axis.
risks/migration: GEN-0041 R3 (`ArrayVec` S2 check) and R2 (`Bytes`
  wire shape) are policy-only today — impls do not exist in
  `pardosa-wire`. A future implementation mission lands them under
  the feature-gate placement of R2. PAR/GEN retirement stays
  deferred per PGN-0001.
