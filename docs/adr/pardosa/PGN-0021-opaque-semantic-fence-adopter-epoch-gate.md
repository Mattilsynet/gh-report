# PGN-0021. Opaque Semantic Fence: Adopter-Epoch Gate

Date: 2026-07-10
Last-reviewed: 2026-07-10
Tier: B
Status: Accepted
Crates: pardosa, pardosa-file, pardosa-nats

## Related

References: PGN-0011, PGN-0019, PGN-0004, PGN-0006, PGN-0001, PGN-0009, PGN-0003

## Context

pardosa's schema gate is wire-shape-only (PGN-0003 R4): a bump that changes
no struct field passes silently, even when the adopter's semantic
interpretation changed. gh-report's `EVIDENCE_SCHEMA_VERSION` 15.0→16.0 bump
(v0.1.17, no struct change) is the confirmed instance (oracle adr-fmt-4e9zp).
Full semantic AWARENESS in pardosa is abort-class (PGN-0001, PGN-0003 R4,
PGN-0019 R5, PGN-0011 R5, COM-0004 R1/R3). Feynman orientation (adr-fmt-18fax)
narrows the live design: an OPT-IN, OPAQUE, adopter-supplied token, stored in
gate metadata and byte-compared, fits PGN-0019 R5 and PGN-0001, but collides
with PGN-0011 R5's "never persists an application-owned identity" precedent.
This ADR ratifies that narrow carve-out — the Opaque Semantic Fence (OSF) —
and specifies the compound-marker format on both backends. Copernicus
evidence (adr-fmt-9nlvf) shows the `.pgno` header (PGN-0004) is a strict
40-byte region with an exact-match version gate and no forward-compatible
extension path, so the epoch needs a version bump there, unlike JetStream's
already-string marker (PGN-0019).

## Decision

Extend the pardosa stream/container schema marker from a single structural
value to a compound `(envelope_hash, adopter_epoch)` pair. `adopter_epoch` is
an `Option<opaque token>` (string or byte sequence), adopter-supplied,
opt-in, stored by pardosa in gate metadata only, and compared byte-for-byte at
store-open; pardosa never interprets, orders, normalizes, or mixes it into any
schema hash, canonical bytes, envelope hash, frontier CRH, or payload
encoding. `adopter_epoch = None` reproduces exactly today's structural-only
gate. `.pgno` carries the new field behind a `FORMAT_VERSION` bump (5 → 6);
JetStream carries it inside the existing `stream_description_marker` string
with no version bump. A new `SemanticEpochMismatch` error variant gates at
open, before any frame is decoded, parity with `SchemaHashMismatch`. OSF
fences the mistake; it does not migrate — a legitimate bump still runs
`migrate_keep` (PGN-0009, PGN-0020) to a fresh stream/container.

R1 [5]: This ADR amends PGN-0011 R5 with one narrow exception: pardosa may
  persist a single adopter-supplied opaque token — `adopter_epoch` — in gate
  metadata only, opt-in, byte-compared at open. Every other clause of
  PGN-0011 R5 stays binding: never mixed into a schema hash or payload
  bytes, never assigned meaning. Full semantic AWARENESS remains abort-class
  per PGN-0001, PGN-0003 R4, PGN-0019 R5 (adr-fmt-4e9zp); out of scope here.
R2 [5]: The stream/container schema marker is a compound value
  `(envelope_hash: u128, adopter_epoch: Option<opaque>)`. `envelope_hash`
  derivation, comparison, and gate position (before first frame decode) are
  unchanged from PGN-0003/PGN-0004 R1/PGN-0019 R2 — this ADR extends the
  marker's composition, it does not alter structural-gate semantics or
  timing.
R3 [5]: Presence of `adopter_epoch` is carried by an explicit discriminant
  distinct from length: on `.pgno`, a presence-flag bit in header `flags`
  (see R5); on JetStream, a sentinel-prefixed segment in the marker string
  (see R6). Zero-length MUST NOT be conflated with absent — a
  present-but-empty `adopter_epoch` differs from `None` on both backends,
  reversing the existing `.pgno` `schema_source` conflation (adr-fmt-9nlvf
  Q5).
R4 [5]: Equality is byte-for-byte opaque comparison: no case-folding, no
  Unicode normalization, no trimming, no locale awareness. `None` compares
  unequal to every `Some(_)` including `Some("")`; two `Some(_)` values
  compare equal only on exact byte identity. Both backends implement
  identical comparison semantics; no per-backend divergence is permitted.
R5 [5]: `.pgno` (PGN-0004) carries `adopter_epoch` behind a `FORMAT_VERSION`
  bump (5 → 6): the exact-match version gate (`reader.rs:137-139`) and
  zero-asserted reserved region admit no forward-compatible extension
  (adr-fmt-9nlvf). Version 6 adds one presence bit to `flags` plus a new
  padded region after `schema_source`, bit — never length — as the absence
  signal. Version-6 readers cannot open version-5 files: a deliberate,
  ADR-sanctioned breaking change (Consequences).
R6 [5]: JetStream (PGN-0019) carries `adopter_epoch` inside the existing
  `stream_description_marker` string, no version bump: the marker is
  `"{envelope_hash_hex}"` when `None`, or
  `"{envelope_hash_hex}:e:{adopter_epoch_hex}"` when `Some(_)` (including
  `Some("")`, rendered as a zero-length hex segment, distinguishable from
  the no-sentinel `None` form). The existing empty-string rejection
  (`authoritative.rs:266-272`) gates only the `envelope_hash_hex` segment.
R7 [5]: `SemanticEpochMismatch { expected: Option<opaque>, found: Option<opaque> }`
  is a new `#[non_exhaustive]`-path variant alongside `SchemaHashMismatch`
  (PGN-0006 R3), raised fail-closed at open before any frame decode.
  Structural gate (`SchemaHashMismatch`) evaluates first; the semantic gate
  is reached only once it has already passed.
R8 [5]: `adopter_epoch = None` reproduces exactly today's behavior: only the
  structural `envelope_hash` gate runs, and an existing stream/container
  with no epoch opens unchanged. Seeding rule: an absent epoch against an
  empty stream/container seeds nothing (the marker stays `None`); a stored
  epoch is set only when the adopter first passes `Some(_)` against an
  empty stream/container, mirroring PGN-0019 R3's envelope-hash seeding.
R9 [5]: Downgrade from `Some(_)` to `None` on a populated stream/container is
  refused with `SemanticEpochMismatch`, parity with PGN-0019 R3's refusal of
  an absent marker against a populated stream. Mismatch fires whenever
  stored and expected disagree on presence (`Some` vs `None`) or, when both
  are `Some`, on byte content; it never fires when both are `None`.
R10 [5]: OSF fences; it does not migrate. On `SemanticEpochMismatch`: (a)
  unintended-mix suspected — halt, investigate before force-opening; (b)
  intended bump — run `migrate_keep` (`ShadowCopy`, PGN-0009 R2/R3, PGN-0020
  R1/R3) to a fresh stream/container and set the new epoch there; the old
  one stays byte-identical (PGN-0009 R3). `.pgno` additionally requires the
  target container be `FORMAT_VERSION = 6`; no in-place upgrade exists.

## Consequences

+ becomes easier: the gh-report v0.1.17 silent-mix scenario becomes a
  fail-closed boot error (`SemanticEpochMismatch{expected:"16.0",
  found:"15.0"}`) instead of an accepted structural-gate pass; adopters gain
  a substrate-enforced semantic fence without pardosa becoming semantically
  aware of the token's meaning.
− becomes harder: `.pgno` readers/writers upgrade in lockstep on the
  `FORMAT_VERSION` 5 → 6 bump (no in-place gain of an epoch; cross-version
  files mutually unreadable) — an accepted breaking-format cost, not a
  defect; two-backend parity requires the presence discriminant implemented
  identically on both, doubling future-audit surface.
risks/migration: ships no code; Phase 2 implements the compound marker +
  gate + error on both backends behind the `None`-default opt-in, TDD with
  live JetStream tests and `.pgno` tests, linus-reviewed. The deployed prod
  stream already contains mixed 15.0/16.0 events; enabling the fence in
  Phase 3 fails closed against it — a reset-vs-migrate call surfaced to the
  user before Phase 3 ships, never a silent daemon break. Non-normative:
  gh-report opts in by passing `EVIDENCE_SCHEMA_VERSION` as `adopter_epoch`
  at store-open (`store/mod.rs:429`, `state.rs:1727`) — adopter wiring, not
  a binding rule here.
