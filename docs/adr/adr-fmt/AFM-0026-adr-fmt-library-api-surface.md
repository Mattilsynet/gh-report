# AFM-0026. adr-fmt Library API Surface

Date: 2026-05-18
Last-reviewed: 2026-05-19
Tier: S
Status: Accepted

## Related

References: AFM-0006, AFM-0017, AFM-0001, CHE-0030, SEC-0004, COM-0007, COM-0013

## Context

`adr-fmt` ships as both a binary (the SSOT per AFM-0001) and a library
in the same crate. With Track 3.2 (`adr-srv`) imminent, the library
seam becomes a cross-crate contract and merits an explicit pin. The
surface is defined only by what `lib.rs` happens to expose; the oracle
summary at bd `adr-fmt-d7ao` enumerates the minimum set `adr-srv`
needs, items currently over-exposed, and the drift from CHE-0030. The
predecessor mission (bd `adr-fmt-mvtu`; commits `ebe791f` T2 lift,
`be0b552` Q2 trim) tightened the surface in-code; this ADR pins it.

Three pressures shape the decision. AFM-0001:R1 freezes the binary CLI
for v0.1; the library MUST NOT widen what the binary promises.
SEC-0004:R3 and COM-0007:R4 prefer minimal default-private surfaces.
COM-0013:R1+R4 forbids speculative complexity and prefers the more
reversible design — flat `pub use` at the crate root is reversible
into a future `adr-fmt-core` split without consumer-side change.

`adr-srv` is the sole intended consumer. Pinning a small surface now
is cheaper than negotiating a wider one later.

Amendment 2026-05-19 (Phase 2 v2 M1.3): R1 broadened to add
`model::{Status, Relationship, RelVerb}`. The `adr-srv` scrape
pipeline projects `AdrRecord`s into the `AdrIngested` event payload
and names these three types directly. They were already public on
`model`; the amendment moves them into the pinned crate-root re-export
set so `adr-srv` does not name a private path. No new types.

## Decision

Pin the `adr-fmt` library API to a flat re-export set at the crate
root, with all underlying modules private (CHE-0030:R1), the binary's
CLI shape unchanged (AFM-0001:R1), and the library forbidden from
calling `std::process::exit`.

R1 [5]: The library exposes exactly these items at the crate root via
  flat `pub use` per CHE-0030:R1; underlying modules are private and
  internal reorganisation is non-breaking for consumers:
  `config::{Config, LoadError, load_quiet, resolve_corpus_root}`,
  `containment::{ContainmentError, contained_join, contained_join_optional}`,
  `model::{AdrRecord, DomainDir, AdrId, Tier, Status, Relationship, RelVerb, parse_adr_id}`,
  `parser::{parse_domain, parse_stale, ParseOutcome}`,
  `report::{Diagnostic, Severity}`.
  `config::load` is intentionally absent; adding it requires a
  current-consumer justification per COM-0013:R1.

R2 [5]: Modules `context`, `nav`, `output`, `refs`, `rules`, and
  `guidelines` are crate-private. They are implementation details of
  the binary's `run()` entry point and MUST NOT be named by external
  consumers. Internal restructuring of these modules — splitting,
  merging, renaming — is a non-breaking change for downstream crates
  and requires no ADR.

R3 [5]: The `report::Diagnostic` struct's public-field shape is part
  of the v0.1 contract: fields `severity`, `rule`, `file`, `line`,
  `message`, `internal` are semver-stable. New fields may be added
  in minor versions; existing fields MUST NOT be removed or reshaped.
  Migration to `#[non_exhaustive]` plus accessors is deferred to v0.2
  and requires a successor ADR. `adr-srv` is the only known consumer
  and simplicity dominates.

R4 [5]: Library code MUST NOT call `std::process::exit`. Errors
  surface as `Result` to the caller; `crates/adr-fmt/src/main.rs` is
  the only authorised exit-code site. Pins the T2 lift landed in
  commit `ebe791f` against regression and reflects SEC-0004:R2
  (authority passed explicitly, never via global process state).

R5 [7]: The library MUST NOT widen what the binary's CLI promises per
  AFM-0001:R1 (frozen for v0.1). New public library items beyond the
  R1 set require their own ADR with current-consumer justification
  per COM-0013:R1. AFM-0006 (regex parsing) and AFM-0017 (P0xx
  namespace) further pin the shape of items already exposed.

## Consequences

+ becomes easier: Track 3.2 (`adr-srv`) depends on a pinned, documented
  library surface without spelunking through `lib.rs`. Internal
  reorganisation of the six private modules no longer risks breaking
  downstream crates. The CHE-0030 doctrinal drift recorded in oracle
  bd `adr-fmt-d7ao` (T1) is resolved.
− becomes harder: any future need for an item outside the R1 set —
  `config::load`, `nav::ChildEntry`, `rules::run_all`, deeper
  `context` access — requires a follow-up ADR rather than ad-hoc
  exposure. Speculative widening is forbidden.
risks/migration: reversibility per COM-0013:R4 — the current `lib+bin`
  arrangement can later be split into `adr-fmt-core` + `adr-fmt`
  without surface change for consumers, since the surface is at the
  crate root via flat `pub use`. This ADR does not pre-authorise that
  split; re-evaluate when a second non-`adr-srv` consumer appears.
