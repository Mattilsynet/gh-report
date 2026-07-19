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
