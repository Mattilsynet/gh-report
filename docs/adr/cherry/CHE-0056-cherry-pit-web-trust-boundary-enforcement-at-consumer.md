# CHE-0056. Cherry Pit Web Trust-Boundary Enforcement at Consumer

Date: 2026-05-14
Last-reviewed: 2026-05-14
Tier: B
Status: Proposed

## Related

References: CHE-0030, CHE-0049, COM-0013, SEC-0003

## Context

`cherry_pit_web::build_router` returns an `axum::Router` that does not
enforce SEC-0003 R1–R3 obligations (request body cap, ingestion-point
backpressure, recursion/iteration limits). Today the only workspace
consumer (gh-report) attaches `DefaultBodyLimit::max(…)`,
`ConcurrencyLimitLayer::new(…)`, and a WebSocket connection semaphore
in `infra/server/server.rs`. The arrangement works but is not named in
an ADR — a future application author wiring cherry-pit-web has no
ratified guide on which side of the library/consumer boundary owns
trust-boundary enforcement. The binding is a coverage gap (bead
adr-fmt-spsd).

## Decision

The trust boundary for SEC-0003 R1–R3 on a cherry-pit-web ingestion
point lives at the **consumer**, not inside the library. The crate
ships an enforcement-free `Router` primitive and documents the
composition contract; the consumer attaches tower layers via
`Router::layer` (axum 0.8.x) before binding.

R1 [10]: `cherry_pit_web::build_router` does not attach
  `DefaultBodyLimit`, `ConcurrencyLimitLayer`, or any WebSocket
  connection cap; the returned `Router` is the unenforced primitive
R2 [10]: Consumers MUST attach SEC-0003 R1–R3 layers via
  `Router::layer` on the value returned from `build_router`, before
  passing it to `axum::serve`; failure to do so is a SEC-0003 R1–R3
  violation at the consumer, not at the library
R3 [9]: The composition contract names three minimum layers: body
  cap (per-POST via `DefaultBodyLimit::max`), concurrency cap
  (router-global via `tower::limit::ConcurrencyLimitLayer`), and
  WebSocket connection cap (route-scoped when `feature = "projection"`
  is enabled; vacuous otherwise per CHE-0049:R3 + R11)
R4 [8]: gh-report's `infra/server/server.rs` is the canonical worked
  example; new application authors copy its layer-attachment shape
  rather than each rediscovering it
R5 [7]: When a third in-workspace consumer ships, this ADR is
  reviewed; if the consumer-side duplication becomes load-bearing
  (more than naming + sizing), supersede with an in-library binding
  per COM-0013:R4 reversibility

## Consequences

Future cherry-pit-web consumers learn the layer-attachment pattern
from this ADR and the gh-report worked example. The library API stays
generic — no `&ValidatedConfig` parameter on `build_router`, no
config field on `AppState`, no second public builder. CHE-0049 R1 and
CHE-0050 R2 signatures remain ratified verbatim. Consumer code
duplicates four to six lines of `ServiceBuilder` composition per
application; reviewed against CHE-0049 R5 (correlation) and R10
(status mapping) — those ARE in-library because their value is
correctness, not sizing; sizing is per-deployment and belongs at the
consumer.

If a future consumer surfaces a pain point this contract does not
cover (e.g. shared rate-limit storage across replicas), the resolution
is a new ADR superseding CHE-0056 with a per-deployment library
binding; reversible per COM-0013:R4.

## Falsifiers

The contract is exercised by gh-report's existing SEC-0003 test sites
(no new test work required for ratification):

- Body cap rejection: `crates/gh-report/src/infra/server/server.rs:1236`
- Concurrency cap shedding: `:2164, :2209`
- WebSocket connection cap: `:3144, :3559`

A future second consumer satisfying R4 (worked-example clone) is the
secondary falsifier for the contract's wide-variety claim per Phase 2
commander intent.
