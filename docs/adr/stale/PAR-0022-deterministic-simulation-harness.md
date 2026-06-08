# PAR-0022. Deterministic Simulation Harness

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: A
Status: Accepted

## Related

References: PAR-0017, PAR-0001, GND-0005

## Context

Distributed-system invariants in pardosa (PAR-0004 fencing, PAR-0007
monotonicity, PAR-0008 durable-first, PAR-0012 chain integrity) cannot
be exhaustively verified by integration tests against a real cluster.
S2.dev, Polar Signals, and TigerBeetle's Vörtex all report that
deterministic simulation testing (DST) — replaying a seeded execution
deterministically across thousands of seeds — is the only practical
oracle for these properties.

The state-machine bus (PAR-0017) makes DST a substitution: the bus
driver becomes a deterministic scheduler with seeded message ordering
and a virtual clock. Edge drivers (NATS, clock, RNG) are stubbed.
The S2 team documents that even one unstubbed entropy source
(getentropy, CCRandomGenerateBytes on Mac) defeats the harness.

## Decision

Pardosa ships a `pardosa::sim` harness driving the state-machine bus
with a seeded scheduler and stubbed edge drivers. CI runs DST on every
PR with a fresh seed and nightly across thousands of seeds. A meta-test
runs each seed twice and compares trace logs byte-for-byte to detect
non-determinism leaks.

R1 [4]: Provide a pardosa::sim::Harness type that drives the
  StateMachine bus deterministically given a u64 seed and a virtual
  clock
R2 [5]: Stub every external dependency including async-nats, the
  system clock, the random number generator, and entropy syscalls
  inside the simulation harness
R3 [6]: Run pardosa::sim with a fresh random seed in every pull request
  CI job and across at least ten thousand seeds in the nightly job
R4 [6]: Run a meta-test that executes the same seed twice and
  asserts byte-for-byte equality of the emitted trace log
R5 [5]: Tag every named simulation scenario with the PAR ADR rule
  identifiers whose invariants it exercises so coverage is auditable

## Consequences

Distributed invariants become testable as ordinary unit tests. New
ADRs gain a forcing function — every accepted invariant must come with
a named scenario. Trade-offs: every dependency boundary must be
abstracted through the bus, which constrains library choices. The
meta-test catches leaks but only after they exist; first detection
requires investigation. Nightly CI cost grows linearly with seed count
— budget accordingly.
