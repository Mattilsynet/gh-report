# SEC-0011. Tamper-Evident Event Logs

Date: 2026-04-29
Last-reviewed: 2026-07-19
Tier: B
Status: Accepted

## Related

References: SEC-0002, SEC-0005, SEC-0008, COM-0025, GND-0005, PGN-0005

## Context

SEC-0008 requires append-only event logs for tamper-evidence, but append-only APIs do not prove that privileged infrastructure did not rewrite history. Checksums detect accidental corruption, not malicious tampering. Three options were evaluated: trust storage permissions, per-event signatures, or hash-chain anchoring with optional signatures. Hash chains provide low-complexity tamper evidence and can later be strengthened with signatures or external anchoring. Under v0.x the unkeyed BLAKE3 chain (PGN-0005) is mutation detection relative to a trusted anchor, not non-repudiation on its own (PGN-0005:R4); no external anchor seam ships under v0.x (PGN-0005:R6).

## Decision

Event logs use tamper-evident metadata where tamper-evidence matters. The first required mechanism is hash chaining over persisted envelope metadata and payload bytes.

R1 [5]: EventStore backends that claim tamper-evidence store a previous_hash and current_hash for each persisted EventEnvelope record
R2 [5]: current_hash covers event_id, aggregate_id, sequence, timestamp, correlation_id, causation_id, event_type, payload bytes, and previous_hash
R3 [5]: Under v0.x, hash-chain verification runs on checked cursor reads (stream-level, on-read) rather than on EventStore open; whole-stream verification prior to aggregate replay on the default boot path is not required. Backends that surface hash-chain verification at all MUST reject discontinuity as StoreError::CorruptData when a checked read is performed.
R4 [5]: Repair workflows preserve the original corrupted bytes and hash-chain failure evidence before rewriting event logs
R5 [5]: External anchoring or digital signatures are added for deployments where storage administrators are in the threat model
R6 [4]: The hash chain provides tamper-evidence (mutation detection) relative to a trusted anchor, not non-repudiation, under v0.x: the unkeyed BLAKE3 chain has no security claim without an independently trusted anchor (PGN-0005:R4); absence of an anchor is unanchored, not invalid (PGN-0005:R6).

## Consequences

Tampering becomes detectable on checked reads even when storage allows overwrite; the default v0.x boot path does not verify the whole stream before replay. This adds metadata and verification cost to backends that opt into tamper-evidence. Hash chains do not prevent deletion or rollback by themselves, and without an external anchor they do not provide non-repudiation; external anchoring or signatures are required for stronger adversaries.

## Proposed amendment (b) — NOT YET RATIFIED

Status: proposal only. R3's opt-in ("backends that surface hash-chain verification at all MUST reject discontinuity") remains the in-force policy. This section records the text R3 would receive if reconciliation path (b) from roadmap adr-fmt-i8j15 (phase adr-fmt-eelvc) is ratified — it does not itself amend the Decision.

Background: PGN-0010:R8 (reconciliation (a), already applied) mandates that an on-read verify stage exists universally above the `AuthoritativeBackend` seam, but leaves each adapter free to declare whether it surfaces hash-chain verification (default true; explicit opt-out permitted with an ADR-cited reason). (a) closes the stage-location gap without touching R3's opt-in. Path (b) would go further: remove the opt-out entirely, so every `AuthoritativeBackend` impl surfaces hash-chain verification and R3's MUST-reject applies unconditionally.

Proposed replacement text for R3, if ratified: "Under v0.x, hash-chain verification runs on checked cursor reads (stream-level, on-read) rather than on EventStore open; whole-stream verification prior to aggregate replay on the default boot path is not required. Every AuthoritativeBackend impl MUST surface hash-chain verification and MUST reject discontinuity as StoreError::CorruptData when a checked read is performed; no per-adapter opt-out is permitted." This drops only the "backends that surface hash-chain verification at all" conditional and the PGN-0010:R8 ADR-cited-opt-out escape hatch; it does not touch R1, R2, R4, R5, or R6, and it makes no authentication claim beyond what R6 already disclaims (PGN-0005:R4 — mutation detection, not authentication).

Precondition gate (must all hold before ratification): (i) reconciliation (a) stays in force and stable; (ii) the P2b `PrecursorCheckMode` enforce capability (PGN-0010, Amendment 2026-07-21) is flipped from its shipped `ObserveOnly` default to `Enforce`; (iii) that `Enforce` default has run through a clean production soak window with zero un-triaged rejections. As of this phase (P3), (ii) has not happened — `Enforce` exists as a capability but is not the shipped default — so the gate is not met and (b) cannot be ratified yet.

Blast radius if ratified: one inbound reference edge today (PGN-0010, References — `adr-fmt --refs SEC-0011`); removing the opt-out also retires the PGN-0010:R8 "ADR-cited reason" escape hatch, which would need a follow-up PGN-0010 amendment note (not a Decision change, since R8 already frames the obligation as deferred). No downstream security ADR (SEC-0002, SEC-0005, SEC-0008) conditions its own rules on R3's opt-in surviving; no contradiction found.

This proposal does not alter R1–R6 above. Reconciliation (a) is the policy in force until a separate ratification decision amends this Decision section directly.
