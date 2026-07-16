# CHE-0089. gh-report TeamStateCaptured Event and Per-Team Fiber

Date: 2026-07-16
Last-reviewed: 2026-07-16
Tier: B
Status: Accepted
Crates: gh-report

## Related

References: CHE-0073, CHE-0074, CHE-0072, CHE-0048, CHE-0068

## Context

CHE-0073 R10 sanctions `TeamStateCaptured` as the third persisted current-state
class for gh-report, mirroring the R8 `OrgStateCaptured` shape. The kqavx
verdict (bd adr-fmt-kqavx) established that team-membership and orphan
attribution do not belong in the `RepositoryStateCaptured` payload — that path
is a SCHEMA_HASH break forcing a re-scrape, and orphan attribution is a
render-time derivation (CLASS B). The roadmap now requires the roster to be
durable on its own stream so the read model has a source of truth surviving
restarts, while orphan attribution stays render-side. This ADR specifies the
event variant, its per-team fiber routing, and its projection part; it ships no
code.

## Decision

Persist team-membership state as a `DomainEvent::TeamStateCaptured` variant on a
per-team pardosa fiber with its own SCHEMA_HASH, folded into a dedicated
read-model part, decoupled from the collect-cycle freshness barrier and from the
repository stream.

R1 [5]: Add `DomainEvent::TeamStateCaptured { org, team_slug, members, orphan_attribution_inputs, fetched_at, status }` as a new durable current-state variant carrying full team identity and membership; it owns its own `GenomeSafe` SCHEMA_HASH, pinned by a regression test, and adds no field to the `RepositoryStateCaptured` or `OrgStateCaptured` payloads (CHE-0073 R2/R8/R10 unaffected, their SCHEMA_HASH values unchanged).

R2 [5]: Route `TeamStateCaptured` on a per-team fiber keyed `(org, team_slug) -> team_domain_key -> pardosa fiber`, one fiber per team, on its own stream/subject (PGN-0010:R6). `team_domain_key` derives from `(org, team_slug)` as a NATS-safe token per CHE-0072:R7/R8 and PGN-0010:R4; the per-team `FiberStore` gives single-writer per team for free. Team is NOT the repository aggregate — team↔repo is many-to-many via CODEOWNERS.

R3 [5]: Map every field TOTALly onto native storage per CHE-0074:R3; recover fibers across restart by `FiberIndex<domain_key>` plus `resume_defined` (CHE-0074:R4/R5); signal removal by `pardosa::StoreWriter::detach` with the envelope `detached` flag as the durable soft-delete (CHE-0074:R6). A team whose membership cannot be mapped totally is an abort, not a partial write.

R4 [5]: Fold `TeamStateCaptured` into a dedicated team read-model part in `EvidenceProjection`, folding the latest snapshot per team fiber and reading the envelope `detached` flag, not a domain tombstone (CHE-0073:R7 pattern). The team part is the read-side source of truth for the roster; render-time orphan attribution joins over CODEOWNERS at the Evidence-assembly seam (kqavx CLASS B), authoring no persisted decision.

R5 [5]: Decouple the team read-model part from the CHE-0068 collect-cycle freshness barrier; it is eventually consistent with the repository and org parts (CHE-0073:R9), bounded-stale by default (GND-0011:R1), and does not gate the terminal render barrier. No causal-consistency floor (GND-0011:R3) applies: the roster feeds a render, not a command.

## Consequences

+ becomes easier: the roster survives restart as a durable read-model source instead of a per-render fetch; the per-team fiber gives single-writer team updates without touching the repository stream or its SCHEMA_HASH.

- becomes harder: a third persisted stream adds a SCHEMA_HASH to pin and a fiber-key derivation to maintain; team removal must go through detach rather than a purge.

risks/migration: additive — `TeamStateCaptured` is a new stream with a new SCHEMA_HASH, so no existing repository or org stream is re-scraped. Rollback is removing the variant before any team events are written (bd adr-fmt-glpuf). Field mapping that cannot be made TOTAL (CHE-0074:R3) is an abort.
