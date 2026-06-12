# CHE-0073. gh-report Storage Remodel

Date: 2026-06-10
Last-reviewed: 2026-06-12
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0074, CHE-0072, CHE-0048, CHE-0022, CHE-0024, CHE-0009, CHE-0005 | Supersedes: CHE-0054

## Context

M2 collapses gh-report's durable event surface after the M1 pardosa adapter proved that cherry-pit logical streams can be reconstructed from pardosa fiber-per-append storage. The earlier three-aggregate model persisted sweep lifecycle, repository lifecycle, and webhook delivery events; the ratified end state persists only repository current-state facts and reserves historical sweep/webhook analytics for a future service.

The P3 native-store port (CHE-0074) later removed the cherry-pit byte adapter entirely: gh-report now persists native `gh_report::event::DomainEvent` values directly through a gh-report-owned pardosa store port, one fiber per repository domain key, and projection deletion is signalled by the pardosa envelope `detached` flag. R1, R2, R6, and R7 are amended below to that as-shipped model; R3, R4, R5 stand.

## Decision

gh-report persists one durable `RepositoryStateCaptured` event variant per repository, carrying repository evidence and identity fields, onto one pardosa fiber per repository domain key. Sweep/run lifecycle and webhook delivery signals remain in memory and tracing, not in the store. `EvidenceProjection` keeps the latest event per live (non-detached) repository fiber as the current-state read model.

R1 [5]: gh-report has exactly one durable event kind, `RepositoryStateCaptured`; each repository domain key maps to one pardosa fiber, realised physically as fiber-per-append and recovered across restarts by `FiberIndex<domain_key>` lookup plus `resume_defined` (CHE-0074:R4/R5). The earlier `(org, repo) -> AggregateId` logical-stream reconstruction through the byte adapter is superseded by CHE-0074.

R2 [5]: `RepositoryStateCaptured` carries full `RepositoryEvidence`, timestamp, and repository identity fields; it has no payload presence marker. A-WS3 removed the write-only native presence field and moved SCHEMA_HASH. Removal detaches the repository fiber (`pardosa::StoreWriter::detach`); envelope `detached` is the durable soft-delete signal, and returning repositories use `rescue_detached`. Removal is neither a second durable variant nor a substrate purge.

R3 [5]: Sweep/run lifecycle and webhook delivery are non-persisted in-memory concerns; this reverses CHE-0054:R1 and CHE-0054:R3 and reverses CHE-0024:R1 persist-then-publish for those event classes only.

R4 [5]: The durable sweep/webhook audit trail is intentionally lost from the gh-report store; the retained operational trail is in-memory state plus tracing logs, with any historical analytics delegated to a future external analytics service.

R5 [5]: Repository identity is `(org, repo) -> domain_key -> pardosa fiber`; this supersedes CHE-0054's R5/R6/R11 routing machinery for Run, Repo, WebhookDelivery, and the `AggregateId(1)` org-governance singleton.

R6 [5]: The storage substrate is `pardosa::store::EventStore<gh_report::event::DomainEvent>` through gh-report's native store port (CHE-0074), not the removed `PardosaEventStore` byte adapter; backend selection is constrained by CHE-0072.

R7 [5]: `EvidenceProjection` folds only `NativeStore::events()` in line order: non-detached snapshots upsert; envelope `detached == true` removes. The fold reads the envelope flag, not a domain tombstone or payload presence marker, and matches an external journal consumer (EDA boundary).

## Consequences

+ becomes easier: gh-report's durable schema now matches the user's current-state read-model intent, and boot replay reconstructs only repository state rather than lifecycle audit streams.

− becomes harder: durable run/webhook failure analytics no longer exist inside gh-report's EventStore; operators rely on tracing until a future analytics service exists.

risks/migration: the remodel is a hard cut over the gh-report store; old CHE-0054 logs are not migrated. A-WS3 is a second hard cut that drops the write-only presence field. Re-scrape repopulates `RepositoryStateCaptured` streams, and rollback is by reverting the relevant native-store commit range.
