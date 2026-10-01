# AFM-0026. adr-fmt Library API Surface

Date: 2026-05-18
Last-reviewed: 2026-10-01
Tier: S
Status: Accepted

## Related

References: AFM-0006, AFM-0017, AFM-0001, CHE-0030, SEC-0004, COM-0007, COM-0013

## Context

`adr-fmt` ships as both a binary (SSOT per AFM-0001) and a library in
the same crate. With Track 3.2 (`adr-srv`), the library seam becomes a
cross-crate contract requiring an explicit pin. Predecessor mission
`ghr-72558ee7` tightened the in-code surface; this ADR ratifies it.

Three pressures shape the decision: AFM-0001:R1 freezes the binary CLI
for v0.1; the library MUST NOT widen what the binary promises.
SEC-0004:R3 and COM-0007:R4 require minimal default-private surfaces.
COM-0013:R1+R4 forbids speculative complexity and prefers flat `pub use`
at the crate root, reversible into a future `adr-fmt-core` split without
consumer changes. `adr-srv` is the sole intended consumer.

Amendment 2026-05-19: R1 broadened to re-export `model::{Status,
Relationship, RelVerb}` for `adr-srv` event ingestion, avoiding private
module path references.

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
  surface as `Result` to the caller; `Mattilsynet/adr-fmt:src/main.rs` is
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
  bd `ghr-19eb22f6` (T1) is resolved.
− becomes harder: any future need for an item outside the R1 set —
  `config::load`, `nav::ChildEntry`, `rules::run_all`, deeper
  `context` access — requires a follow-up ADR rather than ad-hoc
  exposure. Speculative widening is forbidden.
risks/migration: reversibility per COM-0013:R4 — the current `lib+bin`
  arrangement can later be split into `adr-fmt-core` + `adr-fmt`
  without surface change for consumers, since the surface is at the
  crate root via flat `pub use`. This ADR does not pre-authorise that
  split; re-evaluate when a second non-`adr-srv` consumer appears.

## Tier and reference count footnote

AFM-0026 is Tier S (Intent) per AFM-0011 R1 first-yes-wins as it defines
the public library API surface for the adr-fmt governance tool.
References include AFM-0006 (Tier D) as primary parent, triggering an
intentional tier inversion (L016) to bind the library directly to the
canonical CLI flag parser. Seven references exceed the S-tier limit of 3
(T020) because the boundary cross-cuts workspace containment (CHE-0030),
capability restriction (SEC-0004), and minimalism (COM-0007, COM-0013).
