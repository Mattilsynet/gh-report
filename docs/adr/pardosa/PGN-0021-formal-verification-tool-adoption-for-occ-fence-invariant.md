# PGN-0021. Formal Verification Tool Adoption for the OCC-Fence Invariant

Date: 2026-07-15
Last-reviewed: 2026-07-15
Tier: B
Status: Proposed
Crates: pardosa, pardosa-nats

## Related

References: PGN-0016, GND-0010

## Context

PGN-0016's OCC fence is safety- and liveness-critical, but no tool has yet
model-checked it. GND-0010 frames PC/EC as binding on the substrate and
requires an observation mechanism for any consistency deviation (R5, R7).
Candidate tools: Stateright, TLA+, Kani, Smithy. Each trades exhaustiveness,
Rust-native fit, and authoring cost differently; the fence's abort/replay
liveness property needs exhaustive state-space coverage, not sampling alone.

## Decision

Adopt Stateright as the lead exhaustive model-checker for the OCC-fence
safety and liveness invariant; proptest-extension authors the properties
feeding it. Defer TLA+, Kani, and Smithy.

R1 [5]: Adopt Stateright as the lead exhaustive model-checker verifying the
  PGN-0016 OCC-fence safety and liveness invariant, modelling detection-not-
  prevention (PGN-0016 R2) rather than a blocking lock.
R2 [5]: Use proptest-extension as the property-authoring layer feeding
  Stateright's model; properties must encode abort-and-replay as the only
  sanctioned recovery path, never in-band retry.
R3 [5]: Defer TLA+: exhaustive but outside the Rust toolchain, imposing a
  separate spec language and translation-fidelity risk against the shipped
  Rust fence.
R4 [5]: Defer Kani: strong for single-function memory-safety proofs, weak
  fit for the fence's multi-process interleaving and liveness properties.
R5 [5]: Defer Smithy: a protocol/interface-definition tool, not a state-space
  model-checker; does not verify concurrent-writer interleavings.
R6 [5]: Model the fence per GND-0010 R7's observation-mechanism obligation:
  the FV property suite must detect any deviation from always-consistent
  behaviour, not merely confirm the happy path.

## Consequences

+ becomes easier: PGN-0016's abort/replay liveness claim gains exhaustive
  evidence instead of resting on test-suite sampling; regressions in fence
  interleaving surface before shipping.
− becomes harder: the team carries a second Rust-native verification
  dependency (Stateright) and a property-authoring layer (proptest-
  extension) alongside the existing test suite.
risks/migration: Proposed status — no rule is extracted by `--context` and
  no pardosa/pardosa-nats code changes ship with this ADR; the roadmap item
  tracked in bead adr-fmt-2ysyq schedules the FV harness build-out as
  follow-up work once this ADR ratifies.

This ADR is a proposal; ratification is the user's decision. Until Accepted,
no rule above is extracted by `--context` and no pardosa or pardosa-nats
source changes are made.
