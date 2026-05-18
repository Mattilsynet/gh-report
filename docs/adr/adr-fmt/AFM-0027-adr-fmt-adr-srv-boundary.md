# AFM-0027. adr-fmt ↔ adr-srv Boundary

Date: 2026-05-18
Last-reviewed: 2026-05-18
Tier: S
Status: Proposed

## Related

References: AFM-0006, AFM-0017, AFM-0001, CHE-0029, CHE-0030, COM-0012, COM-0013

## Context

Track 3.2 introduces `adr-srv`, a service crate that scrapes the ADR
corpus via the `adr-fmt` library (AFM-0026, a sibling Proposed ADR in
this same commit) and projects records into pardosa-genome event
envelopes. The seam needs an explicit contract before `adr-srv` is
implemented, otherwise pardosa types creep into `adr-fmt`, idempotency
state seeps into `AdrRecord`, and ad-hoc dependency on `Diagnostic`
internals erodes AFM-0026's surface and CHE-0029's acyclic workspace
DAG. AFM-0026 is cited only in prose because a `References:` edge to
a Proposed sibling trips L012.

The oracle summary at bd `adr-fmt-d7ao` answers the questions that
shape this boundary: which crate owns the pardosa bridge (Q4:
`adr-srv`), which owns scrape idempotency (Q4 + COM-0013:R1:
`adr-srv`), and how discovery composes (Q5: both walk-up and
pre-resolved root supported by AFM-0026:R1). COM-0012:R1 binds the
directional rule: inner-layer types are defined inner; outer adapters
live outer. `adr-fmt::model` is inner; pardosa-event mapping is outer.

The boundary is asymmetric by design. `adr-fmt` is leaf-ish governance
(it lints itself) and must not know `adr-srv` exists. `adr-srv` knows
`adr-fmt` and adapts to whatever AFM-0026 pins.

## Decision

Anchor the seam: dependency points one way, the pardosa bridge lives
on the `adr-srv` side, `Diagnostic` is re-projected not re-exported,
idempotency is `adr-srv`'s problem, and parser-shape changes invalidate
prior scrapes wholesale.

R1 [5]: `adr-srv` depends on `adr-fmt` (library); the reverse is
  forbidden. `adr-fmt` MUST NOT name `adr-srv` in any
  `[dependencies]`, `[dev-dependencies]`, or `cfg`-gated path. Pins
  the acyclic workspace DAG per CHE-0029:R1.

R2 [5]: The pardosa bridge — the mapping from `adr_fmt::AdrRecord`
  (and friends re-exported per AFM-0026:R1) into pardosa-genome event
  envelopes — lives in `adr-srv`, not in `adr-fmt`. `adr-fmt` MUST
  NOT take any `pardosa-*` dependency, direct or transitive via
  workspace features. Cites COM-0012:R1 (inner types inner, outer
  adapters outer) and CHE-0029 (adr-fmt is leaf-ish governance, not
  an event source). Per oracle bd `adr-fmt-d7ao` Q4.

R3 [5]: `adr-srv` re-projects `adr_fmt::report::Diagnostic` (per
  AFM-0026:R3) into its own API type — GraphQL, JSON, or otherwise.
  `adr-srv` may add fields to its projection but MUST NOT depend on
  `Diagnostic` internals beyond what AFM-0026:R3 pins. When
  AFM-0026:R3's stability posture evolves (e.g. v0.2 migration to
  `#[non_exhaustive]` + accessors), `adr-srv` adapts on its side
  without `adr-fmt` knowing.

R4 [5]: Scrape idempotency is `adr-srv`'s responsibility. `adr-srv`
  computes a content hash (`body_hash` or equivalent) per
  `AdrRecord` and skips unchanged events on re-scrape. `adr-fmt`'s
  `parse_domain` and `parse_stale` remain pure walk-and-parse,
  stateless; no body-hash field is added to `AdrRecord` for
  `adr-srv`'s benefit per COM-0013:R1 (no speculative complexity
  without a current consumer in the inner crate).

R5 [7]: When `adr-fmt`'s parser shape changes — a new `P0xx` code per
  AFM-0017, an added `AdrRecord` field, a removed `Tier` variant, or
  any other change to `parse_domain` / `parse_stale` output —
  `adr-srv` MUST re-scrape from scratch. There is no migration
  contract between `adr-fmt` versions; AFM-0006 pins the parsing
  approach for v0.1, and parser evolution lives under its successor.

R6 [5]: Discovery is composable. `adr-srv` MAY use
  `adr_fmt::load_quiet` plus `resolve_corpus_root` (re-exported per
  AFM-0026:R1) for walk-up parity with the binary's AFM-0001:R1
  semantics, OR pass a pre-resolved corpus root from its own
  service-level configuration. Both modes are supported by
  AFM-0026:R1 and neither is preferred at the ADR level. Per oracle
  bd `adr-fmt-d7ao` Q5.

## Consequences

+ becomes easier: the `adr-srv` skeleton (Track 3.2) can dispatch
  without ambiguity about idempotency, the pardosa bridge, or
  `Diagnostic` shape. `adr-fmt` stays leaf-ish; CHE-0029's acyclic
  DAG holds.
− becomes harder: any future feature requiring stateful parsing on
  `adr-fmt`'s side — incremental re-lint, a watch mode, cached parses
  across runs — faces R4 and must justify the inversion with its own
  ADR. Parser evolution forces full re-scrape on the `adr-srv` side
  per R5; there is no incremental-migration escape hatch.
risks/migration: AFM-0026:R3's stability posture propagates here via
  R3; if AFM-0026:R3 changes in v0.2 (e.g. to `#[non_exhaustive]`),
  R3 inherits the new posture automatically and `adr-srv` absorbs the
  migration on its side. `pardosa-genome` evolution is out of scope
  on the `adr-fmt` side by construction (R2).
