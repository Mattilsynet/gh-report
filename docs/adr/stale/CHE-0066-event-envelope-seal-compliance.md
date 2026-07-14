# CHE-0066. EventEnvelope and AggregateId Seal Compliance

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: B
Status: Deprecated

## Related

(no lineage edges — successor recorded in Status)

## Retirement

Moved-to-stale: 2026-07-14
Reason: `cherry_pit_core::EventEnvelope` now derives `Serialize`/`Deserialize` (crates/cherry-pit-core/src/event.rs), not `GenomeSafe`; the `pardosa_genome`/`pardosa_encoding` crates this ADR's seal-derive story depended on no longer exist. The seal-compliant derive path was overtaken by the CHE-0071→CHE-0074 native-pardosa lineage, which made the derive question moot rather than resolving it — no single successor carried this ADR's decision whole, so it is retired narratively rather than marked Superseded.
