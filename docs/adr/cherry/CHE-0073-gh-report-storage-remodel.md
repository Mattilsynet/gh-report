# CHE-0073. gh-report Storage Remodel

Date: 2026-06-10
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0071, CHE-0072, CHE-0048, CHE-0022, CHE-0024, CHE-0009, CHE-0005 | Supersedes: CHE-0054

## Context

M2 collapses gh-report's durable event surface after the M1 pardosa adapter proved that cherry-pit logical streams can be reconstructed from pardosa fiber-per-append storage. The earlier three-aggregate model persisted sweep lifecycle, repository lifecycle, and webhook delivery events; the ratified end state persists only repository current-state facts and reserves historical sweep/webhook analytics for a future service.

## Decision

gh-report persists a single Repo aggregate stream per repository identity, using one durable `RepositoryStateCaptured` event variant carrying repository evidence and a `RepoPresence` tombstone. Sweep/run lifecycle and webhook delivery signals remain in memory and tracing, not in the EventStore. `EvidenceProjection` keeps the latest event per repository as the current-state read model.

R1 [5]: gh-report has exactly one durable aggregate kind, Repo; each `(org, repo)` maps to one logical `AggregateId` stream, while CHE-0071's pardosa adapter realises that stream physically as fiber-per-append and reconstructs it on load by filtering aggregate id and sorting `EventEnvelope.sequence`.

R2 [5]: Repo emits exactly one durable event variant, `RepositoryStateCaptured`, containing the full `RepositoryEvidence`, timestamp, repository identity fields, and `RepoPresence::{Active, Removed}`; removal is represented by appending a `Removed` tombstone, not by a second durable variant or substrate purge.

R3 [5]: Sweep/run lifecycle and webhook delivery are non-persisted in-memory concerns; this reverses CHE-0054:R1 and CHE-0054:R3 and reverses CHE-0024:R1 persist-then-publish for those event classes only.

R4 [5]: The durable sweep/webhook audit trail is intentionally lost from the gh-report EventStore; the retained operational trail is in-memory state plus tracing logs, with any historical analytics delegated to a future external analytics service.

R5 [5]: Repository identity is `(org, repo) -> logical AggregateId stream`; this supersedes CHE-0054's R5/R6/R11 routing machinery for Run, Repo, WebhookDelivery, and the `AggregateId(1)` org-governance singleton.

R6 [5]: The storage substrate is `pardosa::store::EventStore<T>` through `PardosaEventStore<DomainEvent>` per CHE-0071, with backend selection constrained by CHE-0072.

R7 [5]: `EvidenceProjection` is in scope and keeps latest-per-repo current state: `Active` snapshots with evidence upsert the repository row, and `Removed` tombstones delete the repository row before HTML rendering.

## Consequences

+ becomes easier: gh-report's durable schema now matches the user's current-state read-model intent, and boot replay reconstructs only repository state rather than lifecycle audit streams.

− becomes harder: durable run/webhook failure analytics no longer exist inside gh-report's EventStore; operators rely on tracing until a future analytics service exists.

risks/migration: the remodel is a hard cut over the gh-report store; old CHE-0054 logs are not migrated. Re-scrape repopulates `RepositoryStateCaptured` streams, and rollback is by reverting the M2 commit range to the M1 state.
