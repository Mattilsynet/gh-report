# CHE-0074. gh-report Native Pardosa Store Port

Date: 2026-06-11
Last-reviewed: 2026-06-12
Tier: B
Status: Accepted
Crates: gh-report
Parent-cross-domain: PGN-0008 — gh-report consumes pardosa through the public typed facade

## Related

References: PGN-0008, PGN-0003, PGN-0013, PGN-0014, CHE-0073, CHE-0072, CHE-0022 | Supersedes: CHE-0071

## Context

CHE-0071 let gh-report persist through pardosa before gh-report had a native GenomeSafe event tree. That adapter encoded cherry-pit `EventEnvelope<DomainEvent>` values as opaque bytes and reconstructed logical streams on load. P2 introduced a native gh-report event module whose payloads use PGN-0003 canonical schema hashing and PGN-0013 bounded wrappers while the existing domain model remains the serde report/cache model. P1 added `resume_defined` and `rescue_detached`, so gh-report can now own a native pardosa port without the byte adapter.

## Decision

gh-report persists repository state through a gh-report-owned native pardosa store port over `gh_report::event::DomainEvent`. The native event tree is built from the flat serde-facing `domain::{repository,evidence,checks,codeowners}` structs at the persistence boundary; the removed serde enum is not part of the port contract. A-WS3 removed the write-only native presence payload marker and moved the native schema hash. Removal detaches the repository's fiber, and the projection fold reads the pardosa envelope `detached` flag as the sole soft-delete signal (the CHE-0073:R2/R7 Detach model, shipped together with this port).

R1 [5]: gh-report MUST NOT depend on `cherry-pit-pardosa` for production persistence. It consumes pardosa's public typed facade directly through `pardosa::store::EventStore<gh_report::event::DomainEvent>` and sealed backend handles.
R2 [5]: The durable payload type is the native `gh_report::event::DomainEvent` tree. Its bounded strings, vectors, timestamps, and validation follow PGN-0003 and PGN-0013; the serde domain/report tree is not the durable pardosa payload.
R3 [5]: The boundary mapping from the flat `domain::{repository,evidence,checks,codeowners}` structs to `event::DomainEvent` is total for the P3-i vocabulary and MUST preserve every durable field that exists on both sides. A missing native home for a durable domain field blocks the port.
R4 [5]: The store port uses one pardosa fiber per repository domain key. First observation of a key begins a fiber; subsequent observations append to that same fiber.
R5 [5]: On boot, gh-report rebuilds a `FiberIndex<domain_key>` from the log and uses `resume_defined` to append to an existing Defined fiber. A lookup returning no fiber starts a new fiber; a divergent lookup is a storage-integrity failure owned by gh-report, not a pardosa choice.
R6 [5]: Removal appends a native `RepositoryStateCaptured` payload and detaches the repository's fiber (`pardosa::StoreWriter::detach`); the pardosa envelope `detached` flag is the sole soft-delete signal, and a returning repository is rescued via `rescue_detached`. `EvidenceProjection` folds only written events, reads envelope `detached` from the line-order stream, and removes detached repositories from the read model without a projection-side tombstone delete (CHE-0073:R2/R7).
R7 [5]: Projection rebuild stays on the gh-report store-port seam and never tails `.pgno` bytes, JetStream cursor state, or backend-specific messages directly. Backend selection remains governed by CHE-0072.
R8 [5]: The old CHE-0071 opaque-byte adapter contract is retired whole: no `EventEnvelope`-as-bytes payload, no adapter-owned logical stream reconstruction, and no `PardosaEventStore<DomainEvent>` in gh-report wiring.

## Consequences

+ becomes easier: gh-report's durable bytes are schema-hashed native events, one fiber per repository key, and restarts continue the same fiber via the public pardosa resume API.
− becomes harder: gh-report owns the domain-to-native mapping and must keep tests proving field preservation and same-fiber continuation across restart.
risks/migration: this is a hard cut for gh-report's event log. A-WS3 is a second hard cut that drops the write-only presence field and relies on re-scrape for recovery. Removal is a pardosa Detach transition carried by the envelope `detached` flag, not a projection-side tombstone delete; rollback is by reverting the native-port commit range.
