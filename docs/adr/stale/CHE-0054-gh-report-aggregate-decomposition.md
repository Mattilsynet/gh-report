# CHE-0054. gh-report Aggregate Decomposition

Date: 2026-05-10
Last-reviewed: 2026-06-10
Tier: B
Status: Superseded by CHE-0073

## Retirement

Moved-to-stale: 2026-06-10
Reason: M2 replaced the three-aggregate Run/Repo/WebhookDelivery durable model with CHE-0073's single Repo logical stream and one `RepositoryStateCaptured` durable event variant. Sweep/run lifecycle and webhook delivery moved to in-memory state plus tracing, and the projection became the current-state owner.
