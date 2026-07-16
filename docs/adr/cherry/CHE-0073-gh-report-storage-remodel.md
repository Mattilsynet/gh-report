# CHE-0073. gh-report Storage Remodel

Date: 2026-06-10
Last-reviewed: 2026-07-16 — amended — added R10 TeamStateCaptured as the third persisted current-state class (mirrors R8; specified by CHE-0089); recorded roster durability requirement and render-time orphan-attribution boundary (mission:fr44n)
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0074, CHE-0072, CHE-0048, CHE-0022, CHE-0024, CHE-0009, CHE-0005 | Supersedes: CHE-0054

## Context

M2 collapses gh-report's durable event surface after the M1 pardosa adapter proved that cherry-pit logical streams can be reconstructed from pardosa fiber-per-append storage. The earlier three-aggregate model persisted sweep lifecycle, repository lifecycle, and webhook delivery events; the ratified end state persists current-state facts and reserves historical sweep/webhook analytics for a future service.

The P3 native-store port (CHE-0074) later removed the cherry-pit byte adapter entirely: gh-report now persists native `gh_report::event::DomainEvent` values directly through a gh-report-owned pardosa store port, one fiber per repository domain key, and projection deletion is signalled by the pardosa envelope `detached` flag. R1, R2, R6, and R7 are amended below to that as-shipped model; R3, R4, R5 stand.

## Decision

gh-report persists durable current-state event variants: `RepositoryStateCaptured` per repository and `OrgStateCaptured` per org. Sweep/run lifecycle and webhook delivery signals remain in memory and tracing, not in the store. `EvidenceProjection` keeps the latest applied event per live repository fiber and per org fiber as the current-state read model.

R1 [5]: gh-report persists durable current-state facts as `RepositoryStateCaptured` on repository fibers and `OrgStateCaptured` on org fibers. `RepositoryStateCaptured` remains one event per repository domain key, realised physically as fiber-per-append and recovered across restarts by `FiberIndex<domain_key>` lookup plus `resume_defined` (CHE-0074:R4/R5). The earlier `(org, repo) -> AggregateId` logical-stream reconstruction through the byte adapter is superseded by CHE-0074.

R2 [5]: `RepositoryStateCaptured` carries full `RepositoryEvidence`, timestamp, and repository identity fields; it has no payload presence marker. A-WS3 removed the write-only native presence field and moved SCHEMA_HASH. Removal detaches the repository fiber (`pardosa::StoreWriter::detach`); envelope `detached` is the durable soft-delete signal, and returning repositories use `rescue_detached`. Removal is neither a second durable variant nor a substrate purge.

R3 [5]: Sweep/run lifecycle and webhook delivery are non-persisted in-memory concerns; this reverses CHE-0054:R1 and CHE-0054:R3 and reverses CHE-0024:R1 persist-then-publish for those event classes only.

R4 [5]: The durable sweep/webhook audit trail is intentionally lost from the gh-report store; the retained operational trail is in-memory state plus tracing logs, with any historical analytics delegated to a future external analytics service.

R5 [5]: Repository identity is `(org, repo) -> domain_key -> pardosa fiber`; this supersedes CHE-0054's R5/R6/R11 routing machinery for Run, Repo, WebhookDelivery, and the `AggregateId(1)` org-governance singleton.

R6 [5]: The storage substrate is `pardosa::store::EventStore<gh_report::event::DomainEvent>` through gh-report's native store port (CHE-0074), not the removed `PardosaEventStore` byte adapter; backend selection is constrained by CHE-0072.

R7 [5]: `EvidenceProjection` folds only `NativeStore::events()` in line order: non-detached snapshots upsert; envelope `detached == true` removes. The fold reads the envelope flag, not a domain tombstone or payload presence marker, and matches an external journal consumer (EDA boundary).

R8 [5]: Org-level state is captured as `OrgStateCaptured` on a dedicated pardosa fiber keyed by org identity (`org -> org_domain_key -> pardosa fiber`), one fiber per org, on its own stream/subject per PGN-0010:R6. The projection folds the latest `OrgStateCaptured` per org fiber into the org read-model part.

R9 [5]: The org and repository read-model parts are folded independently and are eventually consistent; no cross-stream ordering or atomic-snapshot consistency is promised between them. The terminal render reflects the latest applied event of each stream; an org snapshot and a repo snapshot rendered together need not correspond to the same wall-clock instant.

R10 [5]: Team-membership state is captured as `TeamStateCaptured` on a dedicated per-team pardosa fiber keyed `(org, team) -> team_domain_key -> fiber` (PGN-0010:R6) — a THIRD persisted current-state class mirroring the R8 `OrgStateCaptured` shape, not the R4 future-analytics bucket; CHE-0089 specifies variant, routing, and projection part. It is the read-side source of truth for the roster; orphan attribution stays a render-time derivation (kqavx CLASS B) adding no `RepositoryStateCaptured` field (R2 SCHEMA_HASH unaffected).

## Consequences

+ becomes easier: gh-report's durable schema now matches the user's current-state read-model intent, and boot replay reconstructs only repository state rather than lifecycle audit streams.

− becomes harder: durable run/webhook failure analytics no longer exist inside gh-report's EventStore; operators rely on tracing until a future analytics service exists.

risks/migration: the remodel is a hard cut over the gh-report store; old CHE-0054 logs are not migrated. A-WS3 is a second hard cut that drops the write-only presence field. Re-scrape repopulates `RepositoryStateCaptured` streams, and rollback is by reverting the relevant native-store commit range.
