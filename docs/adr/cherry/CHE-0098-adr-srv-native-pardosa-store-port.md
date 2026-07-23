# CHE-0098. adr-srv Native Pardosa Store Port

Date: 2026-07-23
Last-reviewed: 2026-07-23
Tier: B
Status: Accepted
Crates: adr-srv
Parent-cross-domain: PGN-0008 — adr-srv consumes pardosa through the public typed facade

## Related

References: PGN-0008, PGN-0003, PGN-0013, PGN-0014, CHE-0074, CHE-0075, CHE-0072, AFM-0027 | Supersedes: none

## Context

adr-srv persists `AdrIngested` scrape records through `cherry-pit-gateway`'s
`MsgpackFileStore`, a plain serde struct with no GenomeSafe scaffolding.
`adr-srv/Cargo.toml` carries a comment citing a "CHE-0054:R8/R10 carve-out"
authorizing this. That carve-out does not exist: CHE-0054 is a stale,
superseded stub that never named adr-srv. adr-srv is therefore unlisted in
every domain's crate list and its persistence choice is ungoverned by any
live ADR — the citation is a fabricated provenance for the status quo.

This ADR does two jobs at once: it ratifies a native-pardosa store port for
adr-srv mirroring CHE-0074's gh-report port, and it first-time-governs
adr-srv's crate placement and persistence choice, closing the governance gap
CHE-0074 did not have to close (gh-report was already crate-listed).

adr-srv's event log is a derived projection of the `docs/adr/` markdown
corpus, which is the real source of truth (AFM-0027). Read access already
stays on the CHE-0075 `ReadPort` seam. Corpus re-scrape is the sanctioned
rebuild path, not a durable-data guarantee.

## Decision

adr-srv persists `AdrIngested` scrape records through an adr-srv-owned
native pardosa store port over a native, GenomeSafe adr-srv event type. The
existing serde `AdrIngested` struct is not part of the durable payload
contract; it is mapped to the native event at the persistence boundary. The
migration is a hard cut: existing on-disk `.msgpack` data is abandoned, and
recovery is the boot-time corpus re-scrape defined in AFM-0027, not a
data-migration path. `cherry-pit-gateway`'s `MsgpackFileStore` remains
in-tree as the cherry-pit test-suite reference `EventStore`; adr-srv's
departure removes its only production consumer without mandating removal of
the store itself.

R1 [5]: adr-srv MUST NOT depend on `cherry-pit-pardosa` for production persistence. It consumes pardosa's public typed facade directly through `pardosa::store::EventStore<AdrEvent>` and sealed backend handles.
R2 [5]: The durable payload type is a native adr-srv event tree, schema-hashed per PGN-0003 and bounded per PGN-0013. The serde `AdrIngested` scrape struct is not the durable pardosa payload.
R3 [5]: The boundary mapping from scrape structs to the native event is total for the scrape vocabulary and MUST preserve every durable field on both sides. A missing native home for a durable field blocks the port.
R4 [5]: The store port uses one pardosa fiber per ADR-file aggregate key (CHE-0005:R1). First observation of a key begins a fiber; subsequent scrapes of the same file append to that fiber.
R5 [5]: On boot, adr-srv rebuilds a `FiberIndex<adr_key>` from the log and uses `resume_defined` to append to an existing Defined fiber. No fiber starts a new one; a divergent lookup is a storage-integrity failure owned by adr-srv.
R6 [5]: Scrape is full-rebuild-per-scrape (AFM-0027:R5); adr-srv does not model ADR removal as soft-delete, so no detach/rescue transition applies. If a future scrape mode introduces partial updates, that mode MUST define its own detach model before shipping.
R7 [5]: Read access stays on the CHE-0075 `ReadPort` seam adr-srv already uses to resolve GraphQL from `AdrCorpus`; adr-srv MUST NOT tail `.pgno` bytes directly. Backend selection is governed by CHE-0072.
R8 [5]: The `cherry-pit-gateway` `MsgpackFileStore` contract is retired from adr-srv wiring whole: no `MsgpackFileStore` in adr-srv production wiring, and the `rmp-serde` dev-dependency is dropped once tests retarget to the native event's own serde.
R9 [5]: This ADR is adr-srv's first governing ADR. The `adr-srv/Cargo.toml` comment citing "CHE-0054:R8/R10 carve-out" names no rule that exists; that citation is retired (deleted, per fleet-wide no-comment house style) as part of the migration, and adr-srv's crate placement plus persistence choice are governed here going forward.
R10 [4]: With adr-srv migrated off it, `cherry-pit-gateway`'s `MsgpackFileStore` has no production consumer and is retained solely as the cherry-pit test-suite reference `EventStore` (cherry-pit-app, projection-conformance, durable-scheduler, and two-aggregate fixtures). Full code removal of `MsgpackFileStore` remains blocked until those tests migrate to a native-pardosa test store, a separate future decision.

## Consequences

+ becomes easier: adr-srv's durable bytes are schema-hashed native events, one fiber per ADR file, and adr-srv finally has a governing ADR instead of a fabricated citation.
− becomes harder: adr-srv owns the scrape-to-native mapping and must prove field preservation; the on-disk `.msgpack` store is abandoned rather than migrated.
risks/migration: this is a hard cut for adr-srv's event log. The sanctioned recovery path is the boot-time corpus re-scrape (AFM-0027), not replay of the abandoned `.msgpack` data — the follow-on M2 sub-mission MUST demonstrate a passing rebuild-from-corpus test before M4 drops the old dependencies. `MsgpackFileStore` code removal is deferred to a separate package per R10; this ADR ratifies its test-only status but does not schedule its removal.
