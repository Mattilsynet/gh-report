# PGN-0016. JetStream Subject-Sequence Single-Writer Fence

Date: 2026-06-14
Last-reviewed: 2026-06-15
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0010, PGN-0001, PGN-0008, PGN-0006, CHE-0061, CHE-0006, CHE-0024

## Context

gh-report can run overlapping Cloud Run instances; local file locks have no
cross-instance meaning. PGN-0010 kept JetStream single-writer enforcement
deferred after PAR-0004. Synadia's "JetStream Expected Sequence Headers:
Optimistic Concurrency Without Locks" names expected-sequence headers as OCC:
detect races, reject losers, and retry from current sequence. NATS ADR-42 says
`pinned_client` has no exclusivity guarantee, and ADR-8 says KV direct reads
lack read-after-write consistency. NATS ADR-50 makes atomic batch publish
all-or-nothing in nats-server 2.12; this workspace pins nats-server 2.14.3 and
async-nats 0.49.1. The server reports sequence conflicts as err_code 10071.

## Decision

JetStream backend single-writer enforcement is a write-path OCC fence, not a
deployment singleton or reader-side lock. The final target is atomic batch
publish; the current Rust client admits an interim per-event fence.

R1 [5]: Use `Nats-Expected-Last-Subject-Sequence` at the pardosa append
  boundary as the JetStream single-writer fence; classify wrong-last-sequence
  failures by NATS err_code 10071 or constant form 10164, never by description
  text.
R2 [5]: Treat OCC as detection, not prevention: a losing writer is rejected
  without authoritative append, aborts the run, and may retry only after
  replaying the current stream state.
R3 [5]: Reject consumer singletons, `pinned_client`, Cloud Run singleton
  assumptions, and KV read-then-act as authoritative fences; KV create/update
  may serve only as advisory acceleration when the append-path expect header
  remains decisive.
R4 [5]: Target nats-server 2.12 atomic batch publish with
  `Nats-Expected-Last-Sequence` on the first message as the authoritative
  all-or-nothing fence; under async-nats 0.49.1, per-event subject-sequence
  expect is the sanctioned interim.
R5 [5]: Keep `Nats-Msg-Id` BLAKE3 dedup and the 2-minute duplicate window
  orthogonal to sequence expectations: expect headers fence concurrent writers;
  dedup suppresses exact retries. Interim singleton-conflict recovery replays
  persisted events per CHE-0024 and dedups exact payloads.
R6 [5]: Implement expect-header and expected-sequence threading in the pardosa
  adapter ring, where the `publish_once` caller owns storage policy; keep
  `pardosa-nats` substrate-pure and keep cherry-pit storage free of NATS
  dependencies.
R7 [5]: Scope this fence to single-writer plus fast failover, not concurrent
  multi-writer useful-work sharing; overlapping Cloud Run instances become safe
  because stale writers fail at append, not because overlap is impossible.
R8 [5]: Mint a UUID v7 owner id at process start for fencing audit; never
  derive owner identity from hostname, shared `K_REVISION`, or unconfirmed
  Cloud Run metadata instance ids.
R9 [5]: Surface JetStream wrong-last-sequence (10071/10164) as neutral typed
  conflict errors: `pardosa-nats` exposes no `async_nats` type or err_code,
  `pardosa::BackendError` carries `ConcurrencyConflict`, and `PardosaError`
  preserves a matchable conflict variant across the store boundary without
  string-flattening.

## Consequences

+ becomes easier: CHE-0061's marker-trait claim can rely on a concrete
  JetStream fence; gh-report Cloud Run overlap fails at append instead of
  depending on deployment singleton folklore.
+ becomes easier: conflict recovery is neutral and matchable across the store
  boundary; fast-failover writers abort/replay per R2/R7 without
  string-parsing or multi-writer coordination.
− becomes harder: append callers must carry expected stream state, classify
  NATS conflicts by err_code, abort losing runs, and preserve replay paths for
  interim per-event publishing.
risks/migration: async-nats 0.49.1 exposes per-event expect constants but no
  pardosa-ready atomic-batch publisher binding the commit and expect header set;
  atomic batch needs explicit header plumbing or a client bump. Interim
  per-event publishing is correctness-safe, but migration must prefer atomic
  batch once the gap closes.
