# CHE-0044. Object Store Backend (Planned)

Date: 2026-04-25
Last-reviewed: 2026-06-19
Tier: D
Status: Deprecated

## Related

(no lineage edges — nothing superseded this planned backend)

## Retirement

Moved-to-stale: 2026-06-19
Reason: the `object_store` (Apache Arrow) backend was planned but never implemented. No code ever referenced `object_store`. The pardosa `.pgno` event-store substrate (CHE-0072 backend knob, CHE-0073 remodel, CHE-0074 native port) became the actual multi-backend path; `MsgpackFileStore` remains the single-machine store.
