# PGN-0023. Read-Model Lag-SLO, Monotonic Token, and Per-Request RYW Fence

Date: 2026-07-15
Last-reviewed: 2026-07-15
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0016, GND-0011, GND-0010, PGN-0022, CHE-0075, CHE-0048, COM-0019

## Context

GND-0011 declares the ground-tier principle: a cross-machine derived view is
a bounded-stale, read-only fold over a log prefix, with monotonic reads and
per-request read-your-writes (RYW) as an opt-in, never a global mode. That
ADR is the "what"; this ADR is the "how" for pardosa and pardosa-nats: the
concrete lag-SLO signal, the monotonic-reads token, and the RYW fence
mechanism, scoped to the crates that hold the JetStream single-writer fence
(PGN-0016) and the JetStream applied-sequence observability signal
(PGN-0022).

PGN-0022 already emits a structured signal at the point PGN-0016 R7's fence
detects an overlapping writer, keyed by owner id, and separately tracks a
projection applied-sequence high-water mark for multi-process detection.
CHE-0075 R2 already binds every typed read-side port to resolve queries from
projection state only, never dispatching writes from the read port — the
compile-time expression of GND-0011 R4's "derived views never author truth."
CHE-0048 scopes projection replay correctness to a checkpointed, single-
process fold; this ADR's lag-SLO and token mechanisms sit on top of that
fold without reopening CHE-0048's single-process scope.

## Decision

Instantiate GND-0011's bounded-stale-default, monotonic-reads, and
per-request-RYW principles as three concrete pardosa/pardosa-nats
mechanisms, each read-tier only.

R1 [6]: Enforce the lag-SLO as `writer_head_seq - projection_applied_seq`: a
  real-numbered lag bound with a hard ceiling. A read from a projection whose
  lag exceeds the ceiling is refused or flagged, never silently served as
  current. This signal is the same primitive as PGN-0022's projection
  applied-sequence high-water mark used for multi-process detection — it is
  not re-derived or re-defined here, only reused for a second purpose (lag
  enforcement rather than overlap detection).
R2 [6]: Implement monotonic reads via a client-carried high-water-mark
  sequence token: the read tier refuses to serve a response older than the
  token the client presents, so a client can never observe sequence N and
  later observe a sequence below N.
R3 [6]: Implement per-request RYW as an explicit opt-in fence: the write
  path returns its committed sequence (the OCC fence's PublishAck sequence —
  the same JetStream ack-position token PGN-0016's fence produces); the
  caller passes that sequence on a subsequent read as a request-scoped flag;
  that read fences to at-least that sequence before returning. No global RYW
  mode exists; a caller that does not pass the token gets the bounded-stale
  default (R1).
R4 [7]: Scope every fence in R1-R3 to the read tier only. The RYW
  PublishAck-sequence fence in R3 must never leak into the append path as a
  catch-up retry or resync-before-append: PGN-0016 R10 already forbids
  resyncing expected-sequence from the subject tip and retrying inside the
  append path, because that lets a stale writer win and defeats PGN-0016 R7.
  A read-tier fence that blocks or retries a read until its target sequence
  is visible is fine; the same mechanism reappearing on the write path is the
  R10 violation and is forbidden here explicitly, not merely by omission.
R5 [5]: Keep every sequence value used by R1-R3 (`writer_head_seq`,
  `projection_applied_seq`, the RYW token) on trace spans and logs only,
  never as a metric label, per COM-0019 R6 — these are exactly the
  high-cardinality identifiers that rule already excludes from labels.
R6 [5]: Resolve every read affected by R1-R3 through the typed read-side
  port (CHE-0075 R1-R2): the lag-SLO check, monotonic token check, and RYW
  fence all execute against projection state only, never against write-side
  history or a command dispatch path.
R7 [5]: Leave CHE-0048's single-process projection-replay scope (its R1,
  R2, R7) untouched; this ADR governs the read-tier consistency contract on
  top of that fold, not the fold's process-boundary scope.

## Consequences

+ becomes easier: a caller can reason about read freshness in one place —
  the lag-SLO ceiling, the monotonic token, and the RYW opt-in are named,
  enforced mechanisms instead of implicit assumptions about projection
  timing.
+ becomes easier: PGN-0022's existing applied-sequence signal gets a second,
  explicitly-named consumer (lag enforcement) without inventing a parallel
  tracking primitive.
− becomes harder: every read-tier call site must thread the monotonic token
  and, where RYW is requested, the PublishAck sequence, through to the
  fencing check; a read-side port that skips this threading silently
  reverts to bounded-stale-only behavior.
risks/migration: the strict boundary in R4 is easy to violate by well-
  intentioned "just retry the read a bit longer" code that quietly grows
  into a write-path resync; any change touching both this ADR and PGN-0016's
  append path must cite both and justify which side of the R4 boundary it
  sits on.
</content>
