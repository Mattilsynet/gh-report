# PGN-0020. Blocking Schema-Version Migration and Stream-Naming Convention

Date: 2026-07-06
Last-reviewed: 2026-07-16
Tier: B
Status: Accepted
Crates: pardosa, pardosa-nats

## Related

References: PGN-0009, PGN-0019, PGN-0008, PGN-0003, PGN-0012, PGN-0013

## Context

The user proposes a blocking schema migration: read the OLD stream, write a
NEW stream (optional upcast), switch to the new stream, and delete the OLD
stream out-of-band once unused; schema version is shown in stream names (V1,
V2, …). This resembles the retired PAR-0005 new-stream model. PGN-0009
already ratifies the copy-forward semantics as `ShadowCopy`, shipped as
`pardosa::store::migrate::migrate_keep`; PGN-0019 already assigns schema-gate
authority to an opaque stream-description marker, not the stream name. This
amendment reconciles the user's design onto both ratified mechanisms rather
than introducing a parallel one.

## Decision

Reconcile the user's blocking V1/V2 migration design onto the already-ratified
`ShadowCopy` + `migrate_keep` mechanism (PGN-0009, PGN-0008) rather than
inventing a parallel one; adopt V1/V2 as a human-facing `stream_name`
convention strictly subordinate to the PGN-0019 schema-marker gate; and
explicitly decline to re-import PAR-0005's retired KV-registry, grace-period,
and preserved-identity mechanics.

R1 [5]: Map the blocking read-old/write-new/switch/delete-old-later design
  onto the ratified `ShadowCopy` direction (PGN-0009 R2, R3) and the shipped
  `migrate_keep` operation (PGN-0008 R7); introduce no new migration
  direction, facade symbol, or parallel mechanism.
R2 [5]: Mint fresh `EventId`/`FiberId` values for every migrated event per
  PGN-0009 R4's `ShadowCopy` posture; carry cross-version correlation in the
  upcast payload only, consistent with PGN-0013's precedent of routing
  distinct schema concerns to distinct draglines rather than comingling
  them, and never assume a continuous identity space spanning old and new
  streams.
R3 [5]: Treat any V1/V2-style label as an adopter-chosen `stream_name`
  convention only (`pardosa-nats::JetStreamConfigBuilder::stream_name`);
  keep the PGN-0019 `stream_description_marker` — itself a JetStream
  expression of the PGN-0003 `SCHEMA_HASH` — as the sole schema-open gate,
  never deriving schema identity from the stream name.
R4 [5]: Do not reintroduce PAR-0005's retired KV-registry cutover,
  grace-period `max_age`, or rollback-by-re-point mechanics under this
  amendment; each remains retired and any future revival requires its own
  justified amendment.
R5 [5]: Delete the old stream only out-of-band, only after migration
  completes, and only by explicit operator action; this amendment
  authorises no automatic trigger and no time-boxed grace period for
  old-stream deletion.
R6 [5]: Invoke the blocking migration only as an explicit, operator-triggered
  `migrate_keep` call under the AGENTS.md ratified-operation non-goal, never
  as an automatic runtime behaviour during normal store open; the separate
  `MigrationPolicy`/`open_with_migration` per-open slot (PGN-0008 R7) stays
  unshipped and out of this amendment's scope.
R7 [5]: Route consumer cutover across the fresh-identity boundary through
  re-scrape by default, per PGN-0019's Consequences, until the dedicated
  cutover mechanism (PGN-0025) applies; a `LineCursor` sidecar from the old
  stream stays rejected against the new stream per PGN-0009 R5.

Open positions this amendment takes without fully closing every question: the
stream-naming convention needs no `pardosa`/`pardosa-nats` code change (an
adopter-level `stream_name` choice); consumer/cursor cutover across the
fresh-identity boundary defaults to re-scrape (R7) pending a dedicated
cutover ADR; old-stream deletion safety stays manual-only (R5) with no
automated interlock proposed. This amendment also ratifies `migrate_keep` as
the standing pattern once event data must be preserved across a version
boundary; PGN-0009's reset-first clean-break path (and PGN-0012's pre-1.0
major-version framing) remain the undisturbed, lighter-weight alternative
when preservation is not required. None of this ships code: ratifying this
ADR records positions and defaults only — `MigrationPolicy`,
`open_with_migration`, and any JetStream-backed `migrate_keep` extension
remain unshipped future work.

This ADR is ratified. Its rules are extracted by `--context`; the consumer
cutover mechanism left open at R7 is closed by PGN-0025. Ratification records
positions and defaults only — `MigrationPolicy`, `open_with_migration`, and any
JetStream-backed `migrate_keep` extension remain unshipped future work, and no
`pardosa`/`pardosa-nats` source changes ship with this ADR.

## Consequences

+ becomes easier: adopters cite one amendment instead of re-deriving
  `ShadowCopy` + `migrate_keep` semantics from PGN-0009/PGN-0008 each time a
  version-labelled stream is provisioned; the V1/V2 label becomes a
  documented, sanctioned `stream_name` convention.
− becomes harder: adopters who assumed PAR-0005-style preserved `event_id`
  continuity or KV-registry auto-cutover must build cross-version
  correlation and consumer cutover themselves; consumer cutover stays open
  pending a dedicated ADR.
risks/migration: Accepted status — rules are extracted by `--context`; no
  pardosa/pardosa-nats code changes ship with this ADR. Any JetStream-backed
  `migrate_keep` extension remains follow-up work; the consumer-cutover
  mechanism is ratified separately by PGN-0025.
