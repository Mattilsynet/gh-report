# PGN-0005. Frontier Integrity and Trusted Anchors

Date: 2026-06-08
Last-reviewed: 2026-06-08
Tier: B
Status: Accepted
Crates: pardosa, pardosa-wire

## Related

References: PGN-0001, PGN-0003, PGN-0004

## Context

Sources rescue ADR-0004 (frontier chaining) and rescue ADR-0021 (trusted frontier anchor — Accepted as a design contract; no Rust implementation yet). A `Frontier([u8; 32])` is a rolling unkeyed BLAKE3 hash over canonical wire-form bytes. Frontier is **mutation detection relative to a trusted anchor**, not authentication. Two prior bug classes motivated the rescue rules: forgetting to fold during rehydrate (a silent reset to GENESIS), and folding out of persisted line order. ADR-0021 names the admissible class of independently trusted anchor (external authenticated sidecar manifest); no public seam ships under PGN until the implementation ADR lands.

## Decision

Frontier rolls in persisted line order, exactly once per event, on the canonical wire-form bytes. `Dragline<T>` owns frontier and exposes it only through `Dragline::frontier()`. The only public API that re-derives frontier from a `.pgno` is `rehydrate(source) -> Dragline<T>`. `Frontier::GENESIS = Frontier([0; 32])`. ADR-0021 anchors arrive out-of-band via an external authenticated manifest distinct from the `.pgno`; no anchor seam, no `MigrationPolicy` symbol, and no anchor-aware constructor ships under v0.x.

R1 [5]: Frontier rolls in persisted line order, exactly once per event,
  on `Encode`'s canonical wire-form bytes — folding out of order or
  skipping a fold is a substrate-contract violation.
R2 [5]: `Dragline::frontier()` is the only exposed frontier accessor;
  there is no public free function that folds frontier and no public API
  accepts a caller-supplied frontier.
R3 [5]: `rehydrate(source)` is the only public path that re-derives the
  frontier from a `.pgno`; streaming consumers via `stream_events` fold
  their own frontier and the API documents that burden.
R4 [4]: Frontier is mutation detection, not authentication; the unkeyed
  BLAKE3 chain has no security claim without an independently trusted
  anchor held by the verifier out-of-band.
R5 [4]: Anchor manifests are external, authenticated, and distinct from
  the `.pgno` writer's authority domain; in-`.pgno` anchor embedding is
  forbidden because such an anchor is not independently trusted.
R6 [5]: A `.pgno` written under any current writer opens with today's
  behaviour even after an anchor-verifying implementation lands; absence
  of a manifest is unanchored, not invalid.

## Consequences

+ becomes easier: tamper-injection tests against a captured anchor; future
  anchored-open seam without `.pgno` byte change; retroactive anchoring
  of existing `.pgno` files.
− becomes harder: silent acceptance of any change to canonical wire form
  (invalidates existing `.pgno` and every captured anchor); claims of
  end-to-end tamper detection without an anchor source.
risks/migration: ADR-0021 ships no Rust types; the manifest format,
  authority topology, and public seam are gated on follow-up
  implementation ADRs. Pre-publish reset remains the recovery path for
  unintended mismatches.
