# GND-0010. Pardosa and Cherry-Pit Choose PC/EC — Consistency Over Availability

Date: 2026-07-15
Last-reviewed: 2026-07-15
Tier: S
Status: Accepted

## Related

References: GND-0005, PGN-0016, COM-0019

## Context

PACELC extends CAP: if Partitioned, choose Availability or
Consistency (PA/PC); Else (normal operation), choose Latency or
Consistency (EL/EC). CAP alone describes only the partition case;
most operational time is the "Else" branch, where the tradeoff is
latency versus consistency.

pardosa and cherry-pit target intra-org, sub-PB, single-region
workloads carrying high-value-density information where correctness
is the product, not a throughput-optimized mass-scale consumer
surface (the Solon niche, GND-0001 applied to storage architecture).
A wrong or inconsistent read costs more here than a slow or
momentarily unavailable one: the workload cannot absorb silent
divergence the way a webscale cache can.

Three options considered:

1. **PA/EL** — favor availability and latency; accept eventual
   consistency and reconcile divergence after the fact. Wrong fit —
   reconciliation cost on high-value-density data exceeds latency
   saved.
2. **Mixed per-operation tunability** — let each call site pick its
   tradeoff point. Maximizes flexibility but produces an
   unauditable consistency surface GND-0005 cannot make observable
   as one directive.
3. **PC/EC throughout** — consistency at all times, under
   partition and in normal operation, accepting the latency and
   availability cost.

Option 3 chosen: the only option whose guarantee is uniform enough
to name, observe, and enforce as one directive across the substrate.

## Decision

pardosa and cherry-pit choose PC/EC: consistency at all times. This
is the Solon stance applied to distributed-systems architecture —
correctness-first, subtractive, intra-org, not webscale — trading
latency and availability for the generalizability that comes from
never having to reconcile a divergent read.

R1 [3]: Classify the substrate as PC/EC on the PACELC axis —
  consistency is chosen both under partition (PC) and during normal
  operation (EC); this classification is binding on every pardosa
  and cherry-pit component
R2 [3]: Under partition, refuse writes or reads that cannot be
  linearized against the single writer rather than serve a
  possibly-stale value (PC)
R3 [3]: In normal operation, accept added latency over serving a
  value that has not been confirmed consistent (EC); latency is the
  price paid, not a defect to optimize away
R4 [7]: Consistency is achieved structurally via single-writer-per-
  aggregate (COM-0018), not via runtime consensus or quorum
  reconciliation — this is a design rule, not a policy toggle
R5 [8]: Concurrency violations are detected, not prevented, via
  OCC fencing (PGN-0016) — the fence rejects a losing writer after
  the fact rather than blocking contention up front, keeping the
  domain synchronous
R6 [3]: Domain logic stays synchronous; only the edges (transport,
  persistence I/O) are async — sync-over-async bridging is
  deliberate infrastructure (PGN-0010, PGN-0015), never a
  workaround
R7 [5]: Every component that touches consistency must define an
  observation mechanism (GND-0005) that detects ANY deviation from
  always-consistent — a silent consistency violation is
  indistinguishable from a correctness defect and must surface as
  loudly as one

## Consequences

- **Single-writer-per-aggregate is a direct consequence**, not an
  independent choice — COM-0018 and PGN-0016's subject-sequence
  fence instantiate this axiom at the storage layer.
- **OCC-fence detection-not-prevention.** PGN-0016 rejects a losing
  concurrent writer after a fenced append; the fence is the
  observability mechanism (R7) that surfaces a would-be consistency
  violation and unwinds it, rather than preventing contention
  structurally.
- **Sync domain, async edges** (PGN-0010, PGN-0015): the domain
  never awaits, keeping linearizability tractable; EC's latency
  cost is absorbed at the async boundary, not smuggled into the
  domain as eventual consistency.
- **Observability obligation.** COM-0019's design-for-observability
  discipline is where the PC/EC guarantee is instrumented; a
  component with no way to detect a consistency deviation has not
  satisfied this ADR.
- **Distributed-failure vocabulary.** COM-0025's shared failure
  model expresses this ADR's partition (PC) behavior; atomic
  durability primitives — CHE-0032's temp-file/fsync/rename path,
  CHE-0048's checkpointed projection replay — keep PC/EC intact
  across crash and restart.
- **Retroactive cost.** A component that silently trades
  consistency for latency/availability without citing this ADR is a
  defect; such deviations must be reported per GND-0004.
- **Explicit non-goal.** Webscale, multi-region, PA/EL-shaped
  workloads are out of scope; PC/EC is correct for the Solon niche,
  not claimed universal.
