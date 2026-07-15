# PGN-0024. Writer-Coordination-and-Horizontal-Scaling Posture

Date: 2026-07-15
Last-reviewed: 2026-07-15
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0016, PGN-0010, PGN-0023, FLO-0006, GND-0010, GND-0011

## Context

PGN-0016 established the JetStream write-path OCC fence as single-writer
correctness; PGN-0023 layered read-tier bounded-staleness, monotonic tokens,
and RYW on top. Neither ADR states the standing posture on two adjacent
questions an adopter or future contributor will keep re-asking: whether
JetStream Raft leader election is a usable writer-coordination primitive, and
whether/how gh-report/pardosa scale writers and readers horizontally. This
ADR records that posture as one ratified decision so it is not re-litigated
per-PR.

Evidence bead adr-fmt-tstwp (OBSERVE pass, 2026-07-15) grounds every
primitive-level claim below in primary sources, confirming and sharpening
what PGN-0016/PGN-0023 already assert:

- JetStream Raft (Meta/Stream/Consumer groups) is server-fleet-internal
  configuration; no NATS client, including async-nats, exposes an API to
  join a RAFT group as a voter or leader. A client can only read
  server-reported leader state after the fact (`StreamInfo`/`ClusterInfo`),
  never participate in the election.
- NATS ADR-8 (rev 13, 2026-02-02) states KV reads have no read-after-write
  consistency ("this should not be relied on to be true"); a KV-CAS lease is
  an application-level composition atop `Create`/`Update`/CAS + TTL, not a
  first-class client primitive, and a lease-holder cannot safely poll "is my
  lease still mine" through a plain `Get` during a partition or replica-lag
  window.
- NATS ADR-42 (rev 8, 2026-04-29) states pull-consumer `pinned_client` gives
  "no such guarantee" of exclusivity and that the `failover` option is "not
  implemented" as of nats-server 2.14 (this workspace pins 2.14.3); the
  `priority_policy`/`priority_groups` fields are gated
  `#[cfg(feature = "server_2_11")]` and this workspace's async-nats feature
  set (`server_2_14`, `jetstream`, `ring`) does not enable `server_2_11` —
  flat, non-hierarchical feature flags mean pinning is not compiled here.
- Cloud Run request-based billing throttles CPU outside request handling, so
  a background lease-renewal loop risks starvation and a false lease-expiry;
  instance-based billing fixes that but idle instances remain killable with
  only ~10s SIGTERM grace. The fence needs no background renewal loop, which
  is itself an argument for fence-over-lease.
- PAR-0020 (stale, Tier A, retired 2026-04-29) proposed a NATS KV-CAS lease
  as single-writer enforcement; it was parked, not rejected on
  primitive-semantics grounds — retired because PGN-0010 R6 chose a
  different enforcement locus (constructor-time exclusion / adopter
  constraint) and PGN-0016 later supplied the JetStream-native OCC fence
  instead. PGN-0024 is the live home for the lease-as-optional-efficiency
  stance; PAR-0020 stays retired and is cited here as historical context
  only.

## Decision

Record the standing posture on writer coordination and horizontal scaling
for the JetStream backend: the fence (PGN-0016), not election or locking, is
the correctness primitive; scaling is partition-based, not
contention-based; a lease is a future optional efficiency layer, never a
correctness substitute.

R1 [5]: JetStream Raft leader election is not an application-writer
  coordination primitive. Raft groups elect a server-internal leader; no
  client API exists to participate in or trigger an election. An
  application must not be designed around becoming, detecting via
  participation, or racing for Raft leadership.
R2 [5]: The append-path OCC fence (PGN-0016 R1-R2) is the single-writer
  correctness primitive on its own; it requires no leader election. Safety
  across overlapping Cloud Run revisions during deploy is achieved by
  detection-not-prevention — a stale writer aborts and replays (PGN-0016
  R7), observed by PGN-0022 — never by electing or assuming a leader.
R3 [5]: Write-side horizontal scaling is single-writer-per-aggregate
  partitioning combined with FLO-0006 late work-binding: partition the
  aggregate space across N instances so each instance is the sole intended
  writer for its assigned aggregates, and let the fence catch accidental
  overlap. This explicitly excludes N-writers-sharing-one-stream: PGN-0016
  R7 already scopes the fence to single-writer-plus-fast-failover, not
  concurrent multi-writer work-sharing, and the fence converts contention
  into wasted aborts rather than throughput.
R4 [5]: Read-side horizontal scaling is the GND-0011/PGN-0023 read tiers
  (bounded-stale default, monotonic reads, per-request RYW opt-in) served
  against one designated writer's projection; reads scale independently of
  and without altering the single-writer-per-aggregate write model in R3.
R5 [5]: A KV-CAS advisory lease (the mechanism PAR-0020 parked) is an
  optional efficiency layer only, never a correctness mechanism. It may be
  revived in future solely to reduce abort-waste under measured write
  contention, layered strictly above the fence and never replacing it;
  any future adoption must account for KV's lack of read-after-write
  consistency (NATS ADR-8) and the Cloud Run background-renewal billing
  caveat above.
R6 [5]: Consumer pinning (`pinned_client`) is rejected as a
  writer-coordination or exclusivity mechanism: NATS ADR-42 states it
  provides no exclusivity guarantee and its `failover` option is not
  implemented at nats-server 2.14; this workspace's async-nats feature set
  does not compile the `server_2_11`-gated pinning fields at all. This is
  the same primitive-level reason PGN-0016 R3 already excludes it.
R7 [8]: Revisit lease adoption (R5) only on a concrete trigger: measured
  write contention or append-abort-waste exceeding a declared SLO. Until
  that trigger fires, the lease stays a deliberate future option, not a
  standing TODO, and no lease-shaped code or scaffolding should be added
  speculatively.

## Consequences

+ becomes easier: a contributor proposing leader-election, consumer
  pinning, or a lease for single-writer coordination has one ADR to cite
  showing the primitive-level reason each is rejected or deferred, instead
  of re-deriving it from NATS upstream ADRs per-PR.
+ becomes easier: horizontal scaling has a named shape (partition writers
  per-aggregate, scale readers via the existing read tiers) instead of an
  implicit assumption that JetStream itself provides multi-writer
  throughput scaling.
+ becomes easier: a future lease-adoption proposal has a pre-agreed
  trigger condition (R7) and known constraints (R5) to satisfy, rather than
  starting from an open question of whether leases are in-scope at all.
− becomes harder: an adopter that assumed Raft or consumer pinning give
  free coordination guarantees must re-architect around the fence
  (PGN-0016) and per-aggregate partitioning (R3) instead.
risks/migration: if measured abort-waste crosses the R7 trigger before a
  concrete SLO exists, the resulting ADR revision must define that SLO
  first — adopting a lease against an undefined threshold would reintroduce
  exactly the "standing TODO" shape R7 forbids.
