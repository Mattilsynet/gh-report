# PGN-0026. FizzBee for Design-Level UCA Modeling

Date: 2026-08-01
Last-reviewed: 2026-08-01
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0021, PGN-0016, GND-0010

## Context

PGN-0021 froze the PGN-0016 OCC-fence safety/liveness property and selected
Stateright as its current lead exhaustive model-checker (R2), deferred and
still unbuilt (roadmap bead adr-fmt-2ysyq). Independently of that roadmap item,
`spec/fizzbee/` already carries four design-level FizzBee (`.fizz`, fizz
v0.5.1) models — `occ_fence.fizz`, `budget_gate.fizz`,
`ordering_single_flight.fizz`, `token_bucket.fizz` — none of which any ADR
currently governs. `occ_fence.fizz` in particular models the same OCC-fence
property PGN-0021 froze, which creates a live ambiguity: does a passing
`occ_fence.fizz` discharge PGN-0021:R1's exhaustive-checking obligation?

An STPA safety analysis (bead adr-fmt-pq1b6.1.1) derived ten monitor/tripwire
requirements from gh-report's control structure; DR-10 names FizzBee
explicitly as the consumer for a concurrent-writer-overlap property "a formal
model should verify independently of runtime instrumentation." Expanding the
FizzBee corpus to model further STPA UCAs (this ADR's companion mission,
bead adr-fmt-wmd74) makes the ambiguity above load-bearing rather than
theoretical: without a ruling, every future `.fizz` model risks being read
either as advisory design documentation or as silently discharging a frozen
ADR obligation, depending on the reader.

An oracle ruling (bead adr-fmt-zjk1r) resolved the ambiguity: FizzBee-as-
design-level-modeling and FizzBee-as-R1's-designated-checker are two distinct
postures, and only the latter requires amending PGN-0021:R2. No hard
contradiction exists between the FizzBee corpus and PGN-0021 today; a
lightweight sibling ADR closes the gap the oracle ruling flagged (bead
adr-fmt-zjk1r's own "Gaps" section) — no ADR distinguishes FizzBee's current
design-level role from the OCC-fence obligation it happens to overlap with.

## Decision

Adopt FizzBee (`.fizz`, fizz v0.5.1) as the design-level / STPA-UCA modeling
tool for pardosa-substrate and gh-report-control-structure concurrency
properties, wired into CI as a gate. This is explicitly a sibling scoping
decision, not a supersede: PGN-0021:R2's tool selection (Stateright, deferred)
is untouched.

R1 [6]: FizzBee models under `spec/fizzbee/` are design-level advisory-turned-
  gate verification: CI runs every `.fizz` spec and fails the build on any
  non-PASSED result (wiring is a wholesale corpus gate, not per-spec advisory
  status — a rotted model is worse than no model).
R2 [6]: No `.fizz` model — INCLUDING `occ_fence.fizz` and this ADR's sibling
  `concurrent_writer_overlap.fizz` — is the PGN-0021:R1 verification-of-record
  for the OCC-fence exhaustive-checking obligation. That obligation remains
  assigned to Stateright (PGN-0021:R2, deferred per bead adr-fmt-2ysyq) until
  and unless a future ADR amends PGN-0021:R2 directly.
R3 [6]: Any future proposal to designate a FizzBee model as PGN-0021:R1's
  discharge MUST amend PGN-0021:R2 explicitly (per PGN-0021:R2's own "tool is
  revisable, obligation is not" clause) rather than accruing that status
  silently through CI-wiring or corpus growth alone.
R4 [5]: Every FizzBee model modeling a consistency or ordering property
  observes GND-0010:R7's deviation-detection obligation — assertions must
  detect ANY deviation from the modeled invariant, not merely confirm one
  happy-path trace (already the shape of the existing corpus's safety/
  liveness assertion pairs; this makes it binding for new specs too).

## Consequences

+ becomes easier: STPA-derived UCAs and future concurrency-design changes gain
  a fast, exhaustive-over-modeled-state-space feedback loop before Rust
  implementation, with CI preventing corpus rot (R1).
+ becomes easier: the FizzBee/Stateright ambiguity this ADR closes cannot
  recur per-spec — R2/R3 give every future `.fizz` model, including
  `occ_fence.fizz`, an unambiguous non-discharging status without re-litigating
  it each time.
− becomes harder: two design-level verification surfaces now exist for the
  OCC-fence property (`occ_fence.fizz` design-advisory, Stateright deferred-
  but-still-the-R1-discharge) — readers must check this ADR (or PGN-0021
  directly) to know which is authoritative, rather than assuming CI-green
  implies R1 satisfied.
risks/migration: no pardosa/pardosa-nats/gh-report Rust code ships with this
  ADR. The Stateright build-out (adr-fmt-2ysyq) is unaffected and remains the
  sanctioned path to actually discharge PGN-0021:R1.
