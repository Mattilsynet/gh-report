# CHE-0062. Library Attaches Availability Layers via Per-Layer Limits

Date: 2026-05-16
Last-reviewed: 2026-05-16
Tier: B
Status: Proposed

## Related

Supersedes: CHE-0056
References: CHE-0030, CHE-0049, SEC-0003, AFM-0022

## Context

CHE-0056 placed SEC-0003 R1–R3 enforcement (request body cap,
ingestion-point backpressure, WebSocket connection cap) at the
consumer. Its R5 named the supersession trigger verbatim:

> R5 [7]: When a third in-workspace consumer ships, this ADR is
>   reviewed; if the consumer-side duplication becomes load-bearing
>   (more than naming + sizing), supersede with an in-library binding
>   per COM-0013:R4 reversibility

Phase 2 v2 Track 4 (mission package
`.ooda/mission-package-phase2-v2-track-4-1778929160.md`) consolidates
gh-report onto cherry-pit-web. Track 4.1's diff inventory
(`.ooda/gh-report-cherry-pit-web-gap.md`) tags `RequestBodyLimitLayer`,
the `http_concurrency_limit` middleware, and the WS semaphore plumbing
as (a) reusable upstream. The first consumer's duplication is already
load-bearing — sizing knobs plus a `Semaphore` plumbed through two
Extension layers — and Track 4.3 cannot delete it without leaving a
SEC-0003 enforcement window. R5 fires.

CHE-0056's Consequences were equally explicit on the inverse: "no
`&ValidatedConfig` parameter on `build_router`, no config field on
`AppState`, no second public builder." This ADR reverses the layer
prohibitions while preserving that API-coupling intent via a
different shape.

## Decision

cherry-pit-web's router builders attach SEC-0003 R1/R3 availability
layers internally and accept per-layer numeric limits as parameters.
The library owns *what layer is attached where*; the consumer owns
*what number goes in*. No consumer config type, validated or
otherwise, crosses the library boundary.

R1 [10]: `cherry_pit_web::build_projection_router` attaches
  `RequestBodyLimitLayer` (router-scoped), a concurrency-limit layer
  with 503-shedding semantics matching gh-report's
  `http_concurrency_limit` (router-scoped), and a WebSocket connection
  semaphore enforced inside the WS upgrade handler — reversing
  CHE-0056:R1's prohibitions on these three attachments. The CQRS
  surface `build_router` follows the same pattern for body cap and
  concurrency cap; the WS semaphore is projection-only per CHE-0049:R3
  + R11.

R2 [10]: Numeric limits enter via a library-owned `LayerLimits` value
  type passed by value. Fields: `max_body_bytes: usize`,
  `max_inflight_requests: usize`, `max_ws_connections: usize`
  (projection only). The struct lives in cherry-pit-web, re-exports
  via `lib.rs` per CHE-0030:R1. Consumers build `LayerLimits` from
  any source; the library does not inspect that source.

R3 [9]: `&ValidatedConfig` (or any consumer-defined config type) MUST
  NOT appear in any cherry-pit-web public signature. CHE-0056's
  unenforced-primitive stance narrows from "no layers at all" to
  "no consumer-typed config at all" — the library discharges its
  SEC-0003 obligation without adopting consumer schema opinion.

R4 [9]: Layer attachment is unconditional — `LayerLimits` carries
  numeric values, not `Option` per field. The contract is "you
  always have these layers; you choose their sizing". Disabling a
  layer is out of scope; the obligation under SEC-0003 R1/R3 is
  unconditional at every cherry-pit-web ingestion point.

R5 [8]: gh-report's `infra/server/server.rs` removes its
  `RequestBodyLimitLayer`, `http_concurrency_limit`, and WS semaphore
  attachments in Track 4.3, after Track 4.2 lands the library-side
  attachments verified by gh-report's existing SEC-0003 test sites
  (body cap rejection, concurrency cap shedding, WS connection cap).
  No SEC-0003 enforcement window opens during the consolidation
  because 4.2 lands before 4.3 deletes.

R6 [7]: When a future availability layer arrives (rate limiting per
  client, slow-loris timeout, per-route quota), it is added to
  `LayerLimits` as a new field rather than introducing a parallel
  config struct or a builder pattern. Adding a non-`Default` field is
  a semver-major event for cherry-pit-web; the workspace tolerates
  this because cherry-pit-web is internal and `Cargo.lock` is
  committed per the crate README.

## Consequences

### On the `&ValidatedConfig` question (the load-bearing call)

Two shapes satisfy the SEC-0003 obligation for items 3/4/5:

**(i) `&ValidatedConfig` on the builder.** cherry-pit-web takes a
reference to gh-report's validated config (or a trait abstracting it)
and reads sizing fields off it. Simple call site; one parameter.

**(ii) Per-layer numeric limits via a library-owned struct.**
cherry-pit-web defines `LayerLimits { … }`; consumers construct it
from any source. Library never sees the consumer's config type.

This ADR takes **(ii)**. cherry-pit-web is shared across an unknown
number of future consumers (CHE-0056:R5 explicitly anticipated
multiplicity). Coupling its public signature to `&ValidatedConfig`
forces every consumer to either name a type with that exact shape or
pulls cherry-pit-web toward a `ValidatedConfigLike` trait whose
surface drifts as consumer needs diverge — both outcomes move
opinion-bearing schema into the library's contract, exactly the
coupling CHE-0056:R1 was protecting against. By contrast a
library-owned `LayerLimits` value type names *the sizing surface the
library actually consumes* (three `usize`s) and nothing else; consumers
retain full freedom over where those numbers come from. The
API-ceremony cost is one extra struct construction per consumer —
finite and lower than the long-term cognitive load of conforming to
`ValidatedConfig`'s contract. Future layers extend the struct without
touching any consumer's schema. Schema coupling outlives API surface
noise, so (ii) wins on long-term cognitive load.

### Other downstream effects

CHE-0049:R1, R11, R12 (generic typed state, independent builders, no
trait object) remain ratified verbatim — `LayerLimits` is a
non-generic value parameter. CHE-0049:R14 (`middleware` private
module) is the canonical home for the new helpers. CHE-0030:R1
requires a `pub use LayerLimits` entry in `lib.rs`. The SEC-0003
obligation against the merged binary surface is satisfied either way;
this ADR moves the discharge point inside the library and eliminates
the consumer-side duplication CHE-0056:R5 named as the supersession
trigger.

## Falsifiers

The contract is exercised by gh-report's existing SEC-0003 test sites
once they migrate to cherry-pit-web's router (Track 4.3):

- Body cap rejection: `crates/gh-report/src/infra/server/server.rs:1236`
- Concurrency cap shedding: `:2164, :2209`
- WebSocket connection cap: `:3144, :3559`

After Track 4.3 these tests target the cherry-pit-web-returned router
unchanged in behaviour; a regression in any one is a falsifier on
this ADR's contract. A future second cherry-pit-web consumer
constructing a `LayerLimits` from its own config without coupling to
gh-report's `ValidatedConfig` is the secondary falsifier on the
schema-decoupling claim.

## Rejected Alternatives

**`&ValidatedConfig` parameter (option i).** Couples cherry-pit-web's
public signature to a consumer-defined type (or to a trait abstracting
one). Rejected per the trade-off analysis above: schema coupling
outlives API surface savings.

**Builder pattern (`Builder::with_body_limit(n).build()`).** Adds API
surface (one type plus N setter methods plus a terminal `build`) for
no expressive gain over a struct literal at the call site. The
struct-literal form is also better for `#[non_exhaustive]` evolution
once cherry-pit-web ships externally.

**Optional layers (`max_body_bytes: Option<usize>`).** Permits a
consumer to silently disable SEC-0003 R1/R3 enforcement, which
contradicts the unconditional obligation. Per R4, the contract is
"you always have these layers".

**Per-layer separate `usize` parameters.** Three loose `usize` args
on each builder is ambiguous at call sites and brittle to extension;
the named-fields struct documents itself.

**Keep CHE-0056's consumer-side placement unchanged.** This was
CHE-0056:R5's explicit branch point: when duplication becomes
load-bearing, supersede. Track 4 is the trigger. Retaining the
status quo means Track 4.3 cannot delete gh-report's layer
attachments without creating a SEC-0003 enforcement window.
