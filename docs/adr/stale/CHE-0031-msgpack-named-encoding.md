# CHE-0031. MessagePack with Named Encoding for Persistence

Date: 2026-04-25
Last-reviewed: 2026-05-18
Tier: D
Status: Superseded by CHE-0065

## Related

(no lineage edges — this ADR is superseded; reverse direction recorded in `Status:`)

## Retirement

Superseded-by: CHE-0065
Moved-to-stale: 2026-05-18
Reason: pardosa-genome ratified as the canonical event-store wire
format per the 2026-05-18 prime directive. CHE-0065 supersedes both
the rmp_serde mechanism mandate and the `#[serde(default)]` evolution
strategy this ADR enabled (CHE-0022:R3, now amended).
