# CHE-0063. Mid-Sweep Partial Evidence Rendered as Non-Terminal Event

Date: 2026-05-17
Last-reviewed: 2026-06-10
Tier: B
Status: Superseded by CHE-0073

## Retirement

Moved-to-stale: 2026-06-10
Reason: CHE-0073 demoted the Run aggregate and its mid-sweep `PartialEvidenceRendered` event to non-persisted in-memory/tracing behaviour. No rule in this ADR survives once durable Run phase and terminal `EvidencePublished` ordering are removed.
