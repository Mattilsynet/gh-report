# PGN-0027. Serving Process Sources writer_head_seq from the JetStream Stream Head

Date: 2026-08-01
Last-reviewed: 2026-08-01
Tier: B
Status: Draft
Crates: pardosa, pardosa-nats

## Related

References: PGN-0023, PGN-0022, PGN-0024, PGN-0016, CHE-0048, COM-0019

## Context

The #8 operational split runs the collector (writer) and a separate serving
process (read tier, PGN-0024 R4) as distinct processes. PGN-0023 R1 defines
read-model lag as `writer_head_seq - projection_applied_seq`, but PGN-0022 R3
and PGN-0023 R8 both deferred *how a separate serving process sources the true
`writer_head_seq`* to a future multi-process ADR (via CHE-0048's
single-process deferral). That ADR did not exist, leaving the #8 read tier a
load-bearing gap: the lag signal it must enforce named a quantity it had no
ratified cross-process way to obtain.

The mechanism exists and is invariant-clean:
`StreamInfo.state.last_sequence`, surfaced through the sync-over-async facade
bridge (`pardosa-nats/src/handle.rs`, the `block_on` run-op path per
PGN-0010 R5). It is a server-reported, read-only projection of stream head —
never a write, never fence participation (PGN-0024 R1). This ADR ratifies that
primitive as the serving tier's cross-process `writer_head_seq` source,
without reopening CHE-0048's single-process fold scope.

## Decision

Ratify `StreamInfo.state.last_sequence`, read through the existing
sync-bridged facade, as the serving process's source of `writer_head_seq` for
PGN-0023 R1 lag; scope the read strictly to the read tier; keep the serving
process a non-writer; keep the deferred single-process projection binding
intact.

R1 [6]: The serving process sources `writer_head_seq` from the JetStream
  stream head via `StreamInfo.state.last_sequence`, read through the existing
  sync-bridged facade path (PGN-0010 R5), as a server-reported read-only
  value. This is the ratified cross-process source PGN-0022 R3 and PGN-0023 R8
  deferred; it is not re-derived from any other primitive.

R2 [6]: Compute serving-tier lag as PGN-0023 R1's
  `writer_head_seq - projection_applied_seq`, where `writer_head_seq` is the
  R1 stream-head value and `projection_applied_seq` is the serving process's
  own applied high-water mark. The dual-form ceiling ratified in PGN-0023's
  2026-08-01 amendment governs unchanged; this ADR only names the head source.

R3 [5]: Scope the stream-head read to the read tier only. It must not touch,
  gate, or resync the append path: PGN-0016 R10 and PGN-0023 R5 forbid
  sourcing an expected-sequence from the subject tip to retry inside the
  append path, and a serving-tier head read must never regress into that
  write-path shape.

R4 [5]: The serving process must remain a read-only consumer and must not
  become a second writer (PGN-0024 R4): reading stream head confers no
  append capability, dispatches no command, and authors no truth. Resolve
  every serving read through the typed read-side port (CHE-0075, PGN-0023 R7)
  against projection state.

R5 [5]: Leave CHE-0048's single-process projection-replay scope (its R1, R2,
  R7) and the PGN-0022 R3 / PGN-0023 R8 single-process projection binding
  intact. This ADR governs only how the serving tier *observes* the writer's
  head, never how the projection fold is partitioned across processes.

R6 [5]: Keep `writer_head_seq`, `projection_applied_seq`, and the derived lag
  on trace spans and logs only, never as metric labels, per COM-0019 R6 and
  PGN-0023 R6 — these are the same high-cardinality sequence identifiers those
  rules already exclude from labels.

R7 [5]: Place the lag instrumentation that consumes R1's head value in the
  pardosa adapter ring, never in `pardosa-nats`: the substrate ring stays pure
  (PGN-0010, PGN-0015 R6), exposing only the raw `StreamInfo` read, while
  lag computation and its telemetry live in the adapter ring above it.

R8 [8]: Treat the monotonic-reads token and per-request RYW fence
  (PGN-0023 R2 / R4) as out of scope for this ADR's cross-process head-source
  question: they remain the caller-carried, opt-in mechanisms PGN-0023 already
  governs and require no new fence to source `writer_head_seq` here. A
  cross-process RYW contract, if ever needed, is separate future work.

## Consequences

+ becomes easier: the #8 serving process has a ratified, invariant-clean
  source for `writer_head_seq`, so PGN-0023 R1's lag enforcement is buildable
  cross-process instead of resting on a deferred future ADR.
+ becomes easier: the existing `StreamInfo.state.last_sequence` primitive
  gains a named second consumer (serving-tier lag) without inventing a
  parallel head-tracking mechanism.
+ becomes easier: the read-tier/write-path boundary for head sourcing is now
  explicit (R3), so a reviewer rejects a "resync head before append"
  regression by citing one ADR.
− becomes harder: the serving process must read stream head and thread the
  value through adapter-ring lag computation; a path skipping the head read
  silently serves bounded-stale-only without disclosure.
− becomes harder: any change touching both this ADR and the append path must
  cite both and justify the head read stays read-side of the R3 boundary.
risks/migration: no code ships with this Draft ADR; it ratifies the sourcing
  decision so the #8 build has governance to cite. PGN-0022 R3 and PGN-0023 R8
  stay live and are not retired — this ADR resolves only the sourcing question
  they deferred, not their single-process projection scope.
