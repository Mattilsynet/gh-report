# PGN-0025. Schema-Migration Cutover Topologies

Date: 2026-07-16
Last-reviewed: 2026-07-16
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0020, PGN-0009, PGN-0008, PGN-0019, PGN-0003

## Context

PGN-0020 R7 left consumer cutover across the fresh-identity boundary open:
routed to re-scrape by default "until a dedicated cutover mechanism is
ratified." ARCH-DIRECTION (user, 2026-07-16) fixes the target migration model:
event-sourcing with hard-delete plus whole-log schema migration of the complete
log, so that never more than one live schema version exists per reader/writer.
The whole-log transform is the already-ratified `ShadowCopy` + `migrate_keep`
operation (PGN-0009 R2/R3, PGN-0008 R7, PGN-0020 R1); this ADR closes only the
open cutover question — *where the transform runs and how the reader switches* —
across two deployment topologies. It ships no code; it records the ordering and
invariant that any cutover implementation must honour.

## Decision

Close PGN-0020 R7 by ratifying two cutover topologies over the ratified
`ShadowCopy` + `migrate_keep` whole-log rewrite: a single-node startup-phase
transform ordered before warm-start, and a multi-component standalone
transformer service deploy-ordered ahead of the new consumer. Both are the same
logical operation (read-old, upcast, write-new, switch, hard-delete-old) under a
single-live-schema-version invariant and the PGN-0009/PGN-0020 fresh-identity
boundary.

R1 [5]: SINGLE-NODE cutover runs the whole-log transform as a discrete startup
  step ordered strictly BEFORE warm-start (projection replay) initialises: on
  boot, if the schema marker (PGN-0019, PGN-0003 `SCHEMA_HASH`) mismatches, run
  the old->new `migrate_keep` whole-log rewrite to completion, then let
  warm-start initialise FROM the new-version log only; warm-start never observes
  the old schema.

R2 [5]: MULTI-COMPONENT cutover runs the transform in a standalone transformer
  service deployed BEFORE the new memoization/consumer version: deploy order is
  transformer service -> (migrates log) -> new consumer version -> old consumer
  retired. The transformer is deploy-ordered ahead of the new reader; no
  consumer reads the new stream before the transform completes.

R3 [5]: Maintain a SINGLE-LIVE-SCHEMA-VERSION invariant across the cutover: no
  reader or writer serves two concurrent schema versions of one log. The old log
  is retired by the PGN-0020 R5 operator hard-delete (out-of-band, explicit, no
  automatic trigger, no grace period) only after the new log is authoritative;
  hard-delete is the end state, not part of the `ShadowCopy` rewrite.

R4 [5]: Honour the fresh-identity boundary (PGN-0009 R4/R5, PGN-0020 R2):
  migrated events carry fresh `EventId`/`FiberId`, cross-version correlation
  lives in the upcast payload only, and a consumer MUST NOT carry a `LineCursor`
  from the old stream across the boundary — cutover restarts the consumer's
  read position against the new stream, it does not resume it.

R5 [5]: Ship no code with this ADR: it ratifies the cutover ordering and
  invariant that supersede PGN-0020 R7's re-scrape default. `MigrationPolicy`,
  `open_with_migration` (PGN-0020 R6), and any JetStream-backed `migrate_keep`
  extension remain unshipped future work; the startup-step ordering (R1) and the
  transformer-service deploy ordering (R2) are the sanctioned implementations of
  those slots when built.

## Consequences

+ becomes easier: adopters have a ratified cutover ordering for both single-node
  and multi-component deployments instead of the PGN-0020 R7 re-scrape default;
  the single-live-version invariant makes the migration boundary auditable.

- becomes harder: cutover requires deploy-ordering discipline (transform before
  warm-start / before new consumer); a consumer that resumes a stale
  `LineCursor` across the boundary is now an explicit ratified violation, not a
  merely-defaulted one.

risks/migration: Accepted status closes PGN-0020 R7. No pardosa/pardosa-nats
  code ships with this ADR; the transformer service and startup-step wiring are
  follow-up implementation work. GND-0011 causal floors do not apply — migration
  is an operator-ordered step, not an aggregate read feeding a command.
