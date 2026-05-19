# CHE-0059. PurgeableEventStore Extension Trait

Date: 2026-05-16
Last-reviewed: 2026-05-19
Tier: B
Status: Deprecated

## Related

(no lineage edges — this ADR is retired without successor)

## Retirement

Moved-to-stale: 2026-05-19
Reason: cherry-pit-pardosa, the sole in-tree substrate implementing
this extension trait, was removed from `crates/`. No remaining
EventStore impl claims the trait; the `Defined → Detached → Purged →
Defined` state machine and `recreate(tombstone, …)` signature have
no code claimant in the workspace. The trait, the rules R1–R5, and
the oracle-adjudicated tombstone-parameter design (adr-fmt-1clv)
are preserved in git history under this ADR id.
