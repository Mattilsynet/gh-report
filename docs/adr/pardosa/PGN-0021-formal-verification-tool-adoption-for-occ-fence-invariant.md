# PGN-0021. Formal Verification Tool Adoption for the OCC-Fence Invariant

Date: 2026-07-15
Last-reviewed: 2026-07-17
Tier: B
Status: Accepted
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
A prior-art scan (bead adr-fmt-j33kz) found no directly reusable FV artifact
for this per-subject sequence-CAS invariant — a search-blind floor, not a
ceiling — so the BUILD path here is justified.

## Decision

Freeze the property: the PGN-0016 OCC-fence safety and liveness invariant
MUST be exhaustively model-checked, not merely sampled. Select Stateright as
the current lead exhaustive model-checker serving that property, with
proptest-extension authoring the properties; the tool selection is revisable,
the exhaustive-checking obligation is not. Defer TLA+, Kani, and Smithy as
current non-selections among tools serving the same frozen property.

R1 [6]: Freeze the property, not the tool — the PGN-0016 OCC-fence safety and
  liveness invariant MUST be exhaustively model-checked, modelling detection-
  not-prevention (PGN-0016 R2); any tool satisfying exhaustive coverage may
  serve it, and the selected tool is revisable while this obligation stands.
R2 [6]: Select Stateright as the current lead model-checker and proptest-
  extension as its property-authoring layer; properties must encode abort-and-
  replay as the only sanctioned recovery path, never in-band retry. This
  selection is revisable; the R1 exhaustive-checking property is not.
R3 [5]: Defer TLA+ as a current non-selection: exhaustive but outside the Rust
  toolchain, imposing a separate spec language and translation-fidelity risk
  against the shipped Rust fence.
R4 [5]: Defer Kani as a current non-selection: strong for single-function
  memory-safety proofs, weak fit for the fence's multi-process interleaving
  and liveness properties.
R5 [5]: Defer Smithy as a current non-selection: a protocol/interface-
  definition tool, not a state-space model-checker; does not verify concurrent-
  writer interleavings.
R6 [5]: Whichever tool serves R1, model the fence per GND-0010 R7's
  observation-mechanism obligation: the FV property suite must detect any
  deviation from always-consistent behaviour, not merely confirm the happy
  path.

## Consequences

+ becomes easier: PGN-0016's abort/replay liveness claim gains exhaustive
  evidence instead of resting on test-suite sampling; regressions in fence
  interleaving surface before shipping.
− becomes harder: the team carries a second Rust-native verification
  dependency (Stateright) and a property-authoring layer (proptest-
  extension) alongside the existing test suite.
risks/migration: no pardosa/pardosa-nats code changes ship with this ADR;
  the roadmap item tracked in bead adr-fmt-2ysyq schedules the FV harness
  build-out as follow-up work.
