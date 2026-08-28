# CHE-0062. Library Attaches Availability Layers via Per-Layer Limits

Date: 2026-05-16
Last-reviewed: 2026-08-27
Tier: B
Status: Accepted

## Related

Supersedes: CHE-0056
References: CHE-0030, CHE-0049, SEC-0003, SEC-0012, AFM-0022

## Context

CHE-0056 placed SEC-0003 R1–R3 enforcement (body cap, backpressure, WS
connection cap) at the consumer. R5 named the supersession trigger
verbatim:

> R5 [7]: When a third in-workspace consumer ships, this ADR is
>   reviewed; if the consumer-side duplication becomes load-bearing
>   (more than naming + sizing), supersede with an in-library binding
>   per COM-0013:R4 reversibility

Track 4 consolidates gh-report onto cherry-pit-web; the first
consumer's duplication is already load-bearing: sizing knobs plus a
`Semaphore` through two Extension layers. Track 4.3 cannot delete it
without opening a SEC-0003 window, so R5 fires.

CHE-0056 was equally explicit on the inverse: no `&ValidatedConfig`
parameter, no config field on `AppState`, no second public builder.
This ADR reverses those prohibitions while preserving the intent
differently.

## Decision

cherry-pit-web's router builders attach SEC-0003 R1/R3 availability
layers internally and accept per-layer numeric limits as parameters.
The library owns *what layer is attached where*; the consumer owns
*what number goes in*. No consumer config type, validated or
otherwise, crosses the library boundary.

R1 [10]: Layer attachment is per-surface, and each surface takes exactly
  the carriers its layers need — reversing CHE-0056:R1's prohibitions on
  these attachments:

  - **CQRS `build_router`** — `RequestBodyLimitLayer` (router-scoped) and
    a 503-shedding concurrency-limit layer matching gh-report's
    `http_concurrency_limit` (router-scoped). No WS upgrade exists on
    this surface, so it takes no WS carrier and no WS cap.
  - **`build_projection_router`** — the same two layers, plus a WebSocket
    connection semaphore enforced inside the WS upgrade handler.
  - **`serve::build_router` / `serve::start`** — the same two layers,
    plus the same WS connection semaphore inside its `/ws` upgrade
    handler.

  Every surface mounting a WS upgrade takes its WS connection cap from
  the `WsPolicy` carrier defined by SEC-0012:R1, never from
  `LayerLimits`.

R2 [10]: Numeric limits enter via library-owned value types passed by
  value. `LayerLimits` fields are exactly `max_body_bytes: NonZeroUsize`
  (a ceiling — routes may narrow it, none may widen it) and
  `max_inflight_requests: NonZeroUsize`. `max_ws_connections` is NOT a
  `LayerLimits` field; it lives on SEC-0012's `WsPolicy`. Both structs
  re-export via `lib.rs` per CHE-0030:R1. Zero is unrepresentable, not
  rejected — see Amendment 2026-08-27.

R3 [9]: `&ValidatedConfig` — or any config type carrying sizing that
  R2's carriers own — MUST NOT appear in any cherry-pit-web public
  signature. `serve::build_router` takes `&ServeOptions`, carrying
  presentation concerns only (`csp`, `error_page_key`). A sizing number
  reachable through two parameters is two sources of truth; after this
  change `LayerLimits` and `WsPolicy` are the only ones.

R4 [9]: Layer attachment is unconditional — `LayerLimits` carries
  numeric values, not `Option` per field. The contract is "you
  always have these layers; you choose their sizing". Disabling a
  layer is out of scope; the obligation under SEC-0003 R1/R3 is
  unconditional at every cherry-pit-web ingestion point.

R7 [9]: "Every ingestion point" includes consumer routes merged via
  `extra_routes`: the availability layers wrap the *merged* router on
  every surface. `max_body_bytes` is a ceiling, not a per-route
  budget — a route group MAY nest a tighter `RequestBodyLimitLayer`
  inside it, NONE may widen it, and a consumer needing a larger body
  raises the ceiling explicitly.

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

## Amendment 2026-08-27 (zero caps unrepresentable)

R2's field types change from `usize` to `NonZeroUsize`, and `WsPolicy::max_connections` follows. This amends ratified text: R2's word "exactly" bound field types as well as names, and R2 carries the ADR's highest leverage. The amendment is recorded rather than implied, per GND-0007.

The basis is a gap, not a preference. Commit `76d618e` removed `ConfigError::ConcurrencyLimitZero`, `WsMaxConnectionsZero` and `MaxRequestBodyBytesZero` and replaced them with nothing; its stated intent addressed *overflow* clamping only. Since then a zero-valued cap has been accepted unguarded on all three surfaces, governed by no rule. Meanwhile R4 already forbids `Option` per field precisely so that a layer cannot be disabled — yet a zero cap disables it in fact, admitting no request at all. R4's intent and the shipped code have contradicted each other since `76d618e`. This amendment closes that contradiction; it does not open a new one.

The competing reading — that a zero cap is a deliberate operational kill-switch or drain control — was tested and falsified rather than merely disputed. Both semaphores are constructed exactly once at router build (`serve/runtime.rs`) from hard-coded consumer constants (`gh-report/src/server.rs`). There is no runtime setter, no environment path, and no operator interface that can reach either value after construction. A drain control no operator can reach is not a control. Zero therefore denotes misconfiguration and nothing else, which makes uniform rejection the more resilient contract on evidence, not on taste.

Encoding follows SEC-0014:R4 (unrepresentable, not validated) and the workspace's existing numeric precedent CHE-0011:R1, where `AggregateId` wraps `NonZeroU64` to eliminate zero as an identity. `NonZeroUsize` is not `Option` and permits no disabling, so it does not collide with R4.

`WsPolicy::max_connections` needs no rule amendment: SEC-0012:R1 names the field but pins no type, so the change lands within what SEC-0012 leaves open. Its constructor `WsPolicy::new(max_connections)` — the sanctioned construction path per SEC-0012's Consequences — changes signature, and that is the real consumer-facing break. Semver-major across cherry-pit-web and gh-report, pre-authorised by R6. R1's per-surface asymmetry (the CQRS surface takes no `WsPolicy`) and SEC-0012:R1's ratified parameter order are unchanged.

## R7 ceiling semantics (2026-08-27)

R7 does not narrow CHE-0049:R2. `extra_routes` remains the consumer's auth-attachment surface and the consumer still composes auth and rate-limit policy there. What R2 never granted — and what the pre-2026-08-27 CQRS merge order accidentally conferred — was exemption from SEC-0003.

Nesting is the mechanism that makes one ceiling workable across unlike routes: `cherry-pit-web` nests 1 KB on its own GET-only serve built-ins, and gh-report's webhook receiver nests 1 MiB on `/webhook`. The ceiling bounds both. Sizing the ceiling below a merged route's genuine need rejects that route at the ceiling before its own wider limit is consulted, which is why raising it is a visible decision rather than an accident of merge order.

Falsifier: a merged route accepting a body larger than `max_body_bytes`, or answering without the CHE-0049:R5 correlation echo, on any of the three surfaces.

## Consequences

`LayerLimits` is library-owned, so no consumer config type crosses the
boundary and future layers extend the struct rather than any consumer's
schema. The cost is one struct construction per consumer.

CHE-0049:R1, R11, R12 remain ratified verbatim — `LayerLimits` is a
non-generic value parameter. CHE-0049:R14 (`middleware`) is the home for
the new helpers; CHE-0030:R1 requires a `pub use LayerLimits` in
`lib.rs`.

`max_ws_connections` moved to SEC-0012's `WsPolicy`: a WS cap without an
origin policy is the omission U1 recorded, and the field was inert on the
CQRS surface, which mounts no WS upgrade.

R3 now also bars a *library-owned* config type carrying R2's sizing —
`serve`'s `ValidatedConfig` was library-owned and still wrong.

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

**`&ValidatedConfig` parameter (option i).** cherry-pit-web takes a
reference to the consumer's validated config (or a trait abstracting
one) and reads sizing fields off it — one parameter, simple call site.
Rejected because cherry-pit-web is shared across an unknown number of
consumers (CHE-0056:R5 anticipated multiplicity): coupling its public
signature to `&ValidatedConfig` either forces every consumer to name a
type of that exact shape, or pulls the library toward a
`ValidatedConfigLike` trait whose surface drifts as consumer needs
diverge. Both move opinion-bearing schema into the library's contract —
precisely the coupling CHE-0056:R1 protected against. A library-owned
`LayerLimits` instead names exactly the sizing surface the library
consumes (two `NonZeroUsize`s) and nothing else, leaving consumers free
over where those numbers come from. Schema coupling outlives API
surface savings.

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
