# pardosa Phase-1 observability: backend + SLO/alerting bands

Status: advisory, in-repo contract (NOT a ratified ADR). Sequenced as
`pacelc-exec` Seq 0 (mission `pacelc-exec-seq0`); roadmap source
`bd show adr-fmt-2ysyq` §B.0/B.1/B.4; ground-truth re-verification
`bd show adr-fmt-facpa`. Oracle disposition (`adr-fmt-jjp82` Q2a): this is
an implementation choice under COM-0019 (GND-0002 intent≠mechanism) — no
new ADR required for either the backend naming below or these bands, as
long as the caveat in § Binding-caveat holds.

## Backend: log-based-metric over `pardosa::jetstream::metrics` (B.0 option b)

The named metrics backend is **option (b): a log-based-metric rule set**
that lifts the already-emitted `pardosa::jetstream::metrics` `info!`
events (`crates/pardosa/src/backend/jetstream.rs`, `record_metric`,
target `pardosa::jetstream::metrics`) into aggregated time-series via a
log-metrics pipeline (e.g. a Loki/promtail metric-rule, a Vector `log_to_metric`
transform, or equivalent — the specific pipeline is an operational choice,
not fixed here).

This is **zero new workspace dependency**: no Prometheus client crate, no
OpenTelemetry SDK, no `metrics`/`metrics-exporter-*` crate is added to any
`Cargo.toml`. The existing `tracing::info!` emission at `jetstream.rs` is
the entire in-repo surface; aggregation and alerting live outside the
workspace, in whatever log-metrics pipeline consumes the structured
`info!` events. Rationale: most reversible option, respects substrate
ring purity (COM-0019:R6 — no instrumentation dependency reaches
`pardosa-nats`; PGN-0015 — instrumentation stays in the adapter ring, not
the sync-facade substrate).

### Emit-only vs detect (honesty caveat — roadmap B.0 item 1)

**Until a log-based-metric aggregation rule set is actually deployed
against the `pardosa::jetstream::metrics` `info!` stream, Phase-1 signals
below are EMIT-ONLY, not DETECT.** A structured log line that nobody
aggregates cannot page anyone. This document names the backend and the
bands the aggregation rules must implement; it does not itself deploy
those rules. Do not claim end-to-end DETECT coverage for any Phase-1
signal until its aggregation rule is live and its alert condition is
wired to an on-call path.

### Binding-caveat (COM-0035 ratchet boundary)

This contract is **advisory**, not binding. If the SLO / cardinality
bands below are later promoted to a *binding* operational contract
(e.g. gating deploys, feeding an SLA), that promotion is a COM-tier child
ADR under the COM-0035 ratchet — out of scope for this document and for
mission `pacelc-exec-seq0`. Until such an ADR lands, treat every band
below as a design target for the aggregation rules, not a ratified
guarantee.

## SLO / alerting bands (roadmap B.1)

Because pardosa/pardosa-nats are ratified PACELC PC/EC-always (§A,
`adr-fmt-2ysyq`), a deviation signal firing at all above its healthy
floor is a **correctness incident**, not a latency-budget burn.

| Signal (Phase-1 unless noted) | Healthy band | Alert condition (deviation) | Discriminates |
|---|---|---|---|
| `pardosa.occ.fence.conflict` (I1) counter by `err_code` 10071/10164 | non-zero but **bounded, transient** under contention — a fenced conflict is the fence *working* | conflict-rate **spike** OR any conflict on a subject with only one known writer | fence-working vs **fence-bypass suspicion** / unexpected multi-writer |
| `pardosa.occ.conflict_unhandled` (I2) counter — `FencedConflict` returned to caller AND not followed by a clean abort/re-drive | **zero** | any non-zero, sustained | **non-convergence** (swallowed conflict → lost write). Post-amendment this replaces v1's "retry storm" discriminator |
| `pardosa.occ.self_fence` (I2b) counter — intra-handle self-fence | **zero** (Semaphore(1) makes it impossible) | any non-zero | mis-built `append_gate` |
| `pardosa.bridge.block_on.duration` (I5) histogram + `ack_timeout` counter | p99 under bridge budget | p99 breach / sustained ack-timeout | `block_on` stall → tail-latency cliff |
| `pardosa.replay.lag` (I6) gauge | bounded, converging | monotonically growing | read-side staleness |
| `pardosa.dedup.hit` + `pardosa.redelivery.observed` (I8) counters | **zero** redelivery; dedup-hits only from legitimate retries | any redelivery-driven append attempt; **any dedup-hit near the 2-min window boundary** | **duplicate-append** residual (F3) |

Note: v1's I2 "replay-retry storm → PC/EL livelock" alert is **retired** —
the PGN-0016 amendment removed in-band retry, so intra-handle livelock is
no longer a failure mode. The replaced discriminator is
`conflict_unhandled` (swallowed conflict), the actual post-amendment
non-convergence risk.

## Cardinality (COM-0019:R6)

Current registered metrics (`OPERATION_TERMINAL_COUNTER`,
`APPEND_LATENCY_HISTOGRAM`) carry labels `op` (3 values) ×
`terminal_category` (6 values) = 18 series/metric, against the
COM-0019:R6 bound of 500 (`adr-fmt-facpa`). Ample headroom for the
Phase-1 signals named above (Seq 1, not this document).

## Scope note

Naming the signals and their bands is this document's job (Seq 0).
*Emitting* the Phase-1 counters/histograms/gauges above (`fence.conflict`,
`conflict_unhandled`, `self_fence`, `block_on.duration`, `replay.lag`,
`dedup.hit`/`redelivery.observed`) is Seq 1 — out of scope here.
