# SEC-0011. Tamper-Evident Event Logs

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: SEC-0002, SEC-0005, SEC-0008, COM-0025, GND-0005

## Context

SEC-0008 requires append-only event logs for non-repudiation, but append-only APIs do not prove that privileged infrastructure did not rewrite history. Checksums detect accidental corruption, not malicious tampering. Three options were evaluated: trust storage permissions, per-event signatures, or hash-chain anchoring with optional signatures. Hash chains provide low-complexity tamper evidence and can later be strengthened with signatures or external anchoring.

## Decision

Event logs use tamper-evident metadata where non-repudiation matters. The first required mechanism is hash chaining over persisted envelope metadata and payload bytes.

R1 [5]: EventStore backends that claim non-repudiation store a previous_hash and current_hash for each persisted EventEnvelope record
R2 [5]: current_hash covers event_id, aggregate_id, sequence, timestamp, correlation_id, causation_id, event_type, payload bytes, and previous_hash
R3 [5]: EventStore load verification rejects hash-chain discontinuity as StoreError::CorruptData before aggregate replay
R4 [5]: Repair workflows preserve the original corrupted bytes and hash-chain failure evidence before rewriting event logs
R5 [5]: External anchoring or digital signatures are added for deployments where storage administrators are in the threat model

## Consequences

Tampering becomes detectable even when storage allows overwrite. This adds metadata and verification cost to backends that opt into non-repudiation. Hash chains do not prevent deletion or rollback by themselves; external anchoring is required for stronger adversaries.
