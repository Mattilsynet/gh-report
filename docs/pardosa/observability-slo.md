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

Current registered metrics carry labels `op` (3 values) ×
`terminal_category` (7 values, `fence_conflict` added Seq 1) for the
two-label metrics (`OPERATION_TERMINAL_COUNTER`, `APPEND_LATENCY_HISTOGRAM`,
`BRIDGE_DURATION_HISTOGRAM`) = 21 series/metric, plus three op-only-label
metrics (`ACK_TIMEOUT_COUNTER`, `OCC_CONFLICT_UNHANDLED_COUNTER`,
`OCC_SELF_FENCE_COUNTER`) at 3 series/metric. Total 72 series against the
COM-0019:R6 bound of 500 (`adr-fmt-facpa`). Ample headroom remains.

## Scope note

Naming the signals and their bands is Seq 0's job. Seq 1 implemented,
in `crates/pardosa/src/backend/jetstream.rs`:

- **I1** (`fence.conflict`): `ConcurrencyConflict` (mapped from
  `JetStreamRuntimeError::WrongLastSequence`, `jetstream.rs:325-331`) now
  gets its own `TerminalCategory::FenceConflict` / `"fence_conflict"`
  label value, distinct from `TerminalCategory::Publish`. Previously it
  was folded into `Publish` (ground-truth gap confirmed by
  `adr-fmt-facpa`).
- **I2** (`conflict_unhandled`): a dedicated
  `pardosa_jetstream_occ_conflict_unhandled_total` counter fires every
  time a `ConcurrencyConflict` is returned to the caller. **Honest
  boundary limit**: the adapter cannot observe whether the caller then
  performs a clean abort/re-drive or silently drops the conflict — that
  happens above this adapter's boundary. This counter is therefore
  scoped to *conflict-surfaced-to-caller*, which is the adapter's whole
  observable surface; a genuinely tighter "conflict THEN no abort"
  signal would require call-site instrumentation outside
  `crates/pardosa/src/backend/jetstream.rs`, out of scope for Seq 1.
- **I2b** (`self_fence`): `pardosa_jetstream_occ_self_fence_total` is
  registered with a healthy value of zero. `append_gate` is a
  `Semaphore(1)` (`pardosa-nats/src/handle.rs:~203-208`), which makes
  intra-handle self-fence structurally unreachable via any public entry
  point today; the emitting function (`record_self_fence`) therefore has
  no call site on the normal path (kept for defense-in-depth, marked
  `#[expect(dead_code, ...)]`). A test pins that normal operation never
  emits it.
- **I5** (`block_on.duration` + `ack_timeout`): the append-only
  `APPEND_LATENCY_HISTOGRAM` is kept unchanged (backward-compatible);
  a new `pardosa_jetstream_bridge_duration_seconds` histogram now fires
  for **every** op (append, sync, replay), closing the "only append"
  gap. A new `pardosa_jetstream_ack_timeout_total` op-labelled counter
  fires whenever `TerminalCategory::Timeout` is observed.
- **I8** (`dedup.hit` + `redelivery.observed`) — **re-scoped, not
  implemented as originally named.** Investigation
  (`async-nats-0.49.1/src/jetstream/publish.rs::PublishAck.duplicate`)
  found the JetStream server *does* expose a per-publish `duplicate:
  bool` flag driven by its own `Nats-Msg-Id` dedup window — this is data
  `pardosa-nats` could surface through `JetStreamAckPosition` (an
  additive field, not a metrics call — allowed under this mission's
  `out_of_scope` clause), but threading it through changes a
  `#[repr(transparent)]` public type's shape across the crate boundary,
  which did not fit this sub-mission's time budget. Per this mission's
  `abort_if` I8 fallback ("if async-nats handles redelivery/dedup
  transparently, re-scope to dedup-window-boundary only and document"):
  **I8 is deferred to a follow-up sub-mission** that threads
  `PublishAck.duplicate` through `JetStreamAckPosition` (or an
  equivalent additive return-type extension) so `dedup.hit` and the
  window-boundary signal can be emitted from the adapter ring without a
  pardosa-nats metrics call. `redelivery.observed` is additionally
  scoped down: the replay consumer uses `AckPolicy::None`
  (`pardosa-nats/src/handle.rs:~637`), so there is no ack-driven
  redelivery to observe on the current consumer path — that counter may
  be vacuous by construction and needs a design decision before
  emission, not just wiring.
- **I6** (`replay.lag`) remains out of scope for Seq 1 per the roadmap
  (unchanged from Seq 0 naming).

