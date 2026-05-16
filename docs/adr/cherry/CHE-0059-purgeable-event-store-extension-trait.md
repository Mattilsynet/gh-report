# CHE-0059. PurgeableEventStore Extension Trait

Date: 2026-05-16
Last-reviewed: 2026-05-16
Tier: B
Status: Accepted

## Related

References: CHE-0057, CHE-0019, CHE-0023, CHE-0039, PAR-0001

## Context

Pardosa's fiber state machine (PAR-0001) supports a physical `Purged`
state with a `Purged → Defined` re-creation transition, severing logical
continuity (`precursor = Index::NONE`) while preserving event history
for audit. The file-backed `cherry-pit-storage` adapter has no such
capability. CHE-0057:R1 forbids adding `purge`/`recreate` methods to
core `EventStore`. CHE-0023:R1 forbids a framework lifecycle trait.
CHE-0019:R1 binds `load` to return `Ok(Vec::new())` for unknown
aggregates. An opt-in extension trait reconciles all three: substrates
that support purge implement the trait; substrates that do not simply
omit it; capability-aware downstream code bounds on the extension.

## Decision

R1 [5]: PurgeableEventStore extends EventStore as a supertrait bound
  per CHE-0057:R2 and lives in cherry-pit-core; standalone definition
  is forbidden.

R2 [5]: PurgeableEventStore exposes exactly two methods: load_history
  returning the full event history including purged aggregates, and
  recreate accepting a caller-supplied tombstone event, a fresh event
  stream, and a CorrelationContext per CHE-0039:R1. The tombstone is
  the final domain event recorded against the prior incarnation; the
  adapter cannot fabricate Self::Event because the type is opaque at
  the port boundary (oracle adjudication adr-fmt-1clv).

R3 [5]: Substrates that cannot physically purge MUST NOT implement
  PurgeableEventStore per CHE-0057:R3; returning a not-implemented
  stub from required methods is forbidden, and the rollout-stub
  carve-out in CHE-0057:R3 does not apply to purge.

R4 [5]: Recreate MUST sever logical continuity with the prior
  incarnation at the substrate layer; pardosa achieves this by
  recording the tombstone via Dragline::detach (Defined → Detached),
  purging via migrate_fiber with MigrationPolicy::Purge (Detached →
  Purged), then re-creating via create_reuse (Purged → Defined) which
  sets the new aggregate's precursor index to NONE per PAR-0001.

R5 [5]: load_history MUST return Ok(Vec::new()) for genuinely unknown
  aggregates per CHE-0019:R1; this preserves the EventStore::load
  contract on the wider surface.

## Consequences

Pardosa's runtime `Purged → Defined` reuse becomes observable through
the cherry-pit port without forcing every adapter to carry a purge
implementation. CHE-0023:R1's framework-lifecycle prohibition is
preserved — purge is substrate capability, not framework lifecycle.
Downstream code requiring purge bounds on PurgeableEventStore per
CHE-0057:R4. The method signatures are append-only per CHE-0057:R5
once this ADR is committed; pre-publication signature drafting (e.g.
adding the tombstone parameter to recreate per adr-fmt-1clv) is
permitted while the ADR remains uncommitted.
