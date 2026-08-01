# CHE-0103. Server-Sent Events Replace WebSocket for Projection Push (Cloud Run HTTP/2 Prereq)

Date: 2026-08-01
Last-reviewed: 2026-08-01

Tier: B
Status: Accepted
Crates: cherry-pit-web

## Related

References: CHE-0049, CHE-0086, SEC-0012

## Context

`build_projection_router` (CHE-0049 R11-R14) mounts an unversioned `/ws`
upgrade pushing `PageUpdate` deltas on every projection sweep
(`crates/cherry-pit-web/src/projection/handlers.rs`). The channel is
UNIDIRECTIONAL: server-to-client only, no inbound DTO exists. A full-duplex
WebSocket is more protocol than that needs; SSE is the narrower HTTP-native
primitive for one-way server push, with `EventSource` reconnect built in.

Second-order consequences: SEC-0012 governs WS-upgrade-time origin auth,
which does not apply to a plain SSE `GET`; the Leptos client (CHE-0087)
swaps `WebSocket` for `EventSource` (no CSP widening — `connect-src 'self'`
already covers it); the route sits in the surface CHE-0049 and CHE-0086
jointly describe.

HARD PREREQUISITE: Cloud Run downgrades HTTP/2 to HTTP/1.1 at the ingress
by default, and browser SSE is capped at 6 connections per domain under
HTTP/1.1. Cutting over before Cloud Run is configured for end-to-end
HTTP/2 (where SSE multiplexes over one connection) would reproduce that
cap for every multi-tab user — a regression, not an improvement. This ADR
ratifies the decision and gates the cutover on that prerequisite.

Tension (COM-0011:R2): SSE trades bidirectional capability for protocol
simplicity; the capability is unused today (no inbound DTO), so the cost
is currently zero, not deferred risk.

## Decision

Ratify Server-Sent Events as the transport for projection-push, replacing
the `/ws` WebSocket upgrade in `cherry-pit-web`'s projection adapter. The
cutover itself is GATED and NOT YET PERFORMED.

R1 [5]: The projection-push transport moves from WebSocket upgrade (`/ws`)
  to Server-Sent Events (a plain `GET` HTTP response with
  `Content-Type: text/event-stream`, chunked, streaming `PageUpdate` JSON
  frames as SSE `data:` events). The frame payload contract (`"v": 1`
  JSON per CHE-0049 R13) is preserved; only the transport envelope
  changes.

R2 [5]: The drop-and-resync backpressure semantics (CHE-0049 R11: on
  `broadcast::RecvError::Lagged` the connection closes, client re-fetches
  the snapshot then re-attaches) are PRESERVED under SSE: the SSE stream
  ends (server closes the response), the client's `EventSource` fires its
  `onerror`/close handling, re-fetches the snapshot (CHE-0048:R2), then
  re-opens the `EventSource`. No new backpressure primitive is introduced.

R3 [5]: This cutover is GATED on Cloud Run being configured for
  end-to-end HTTP/2 (ingress through to the container). Cutting over
  while Cloud Run runs HTTP/1.1 at the edge reproduces the browser's
  6-connections-per-domain SSE cap for every multi-tab user session — a
  regression, not an improvement, over the current WebSocket transport
  (which is not subject to that per-domain HTTP/1.1 cap). The cutover
  MUST NOT be performed until that infrastructure precondition is
  independently verified.

R4 [4]: SEC-0012 (`WebSocketOriginPolicy`, `WsAuthLimits`) is RETIRED for
  the projection-push path as part of the (gated, not-yet-performed)
  cutover: an SSE `GET` has no upgrade handshake for
  `WebSocketOriginPolicy` to govern. Retirement happens atomically with
  the R1 cutover, not before — SEC-0012 stays fully in force for `/ws`
  until the replacement ships (this ADR ratifies the decision; a future
  amendment records the actual retirement once the cutover lands).

R5 [4]: This amends CHE-0049 (typed projection read/WS surface) and
  CHE-0086 (generic read-serve surface) without superseding either: both
  ADRs' surface descriptions gain SSE as the eventual push transport:
  once cut over, "WS upgrade" language in CHE-0049 R11/R13 and any
  WS-specific CHE-0086 cross-references describe the historical
  transport, not the current one. Neither ADR's non-transport rulings
  (route shape, snapshot/ETag semantics, generic-read-serve scope) are
  affected.

R6 [4]: The Leptos web client (CHE-0087) switches from `WebSocket` to
  `EventSource` against the new endpoint as part of the (gated) cutover.
  `DEFAULT_CSP`'s `connect-src 'self'` already permits a same-origin
  `EventSource`; no CSP directive change is required, only updating the
  code comment/doc that currently describes the WS connection.

R7 [3]: No reversible in-repo scaffolding (e.g. a dormant additive SSE
  endpoint alongside `/ws`) is introduced by this ADR. Rejected for this
  iteration: even the smallest scaffold is new `axum` code requiring the
  linus review loop, and exercising it before the R3 prerequisite is
  verified would validate behaviour under forbidden conditions. This ADR
  is deliberately ADR-only; code is deferred to the gated cutover mission.

## Consequences

+ becomes easier: transport matches actual usage (unidirectional),
  dropping unused WS full-duplex machinery and SEC-0012 once cut over.
+ becomes easier: `EventSource` reconnect subsumes the hand-rolled
  re-fetch-then-re-attach logic CHE-0049 R11 requires of the client today.
− becomes harder: bidirectional push, if ever needed, requires
  re-introducing WebSocket; SSE cannot serve it. Acceptable — no inbound
  DTO exists today and none is anticipated.
− becomes harder: the cutover cannot proceed independent of
  infrastructure — Cloud Run's HTTP/2 config is an external dependency
  this ADR cannot resolve, so decision and execution are decoupled (R3).
risks/migration: additive-then-swap, not immediately reversible once
  performed — cutting `/ws` to SSE and retiring SEC-0012:R1-R5 (R4)
  removes the WS auth contract; rollback needs re-adding the WS handler
  and SEC-0012, not a flag flip. No code accompanies this ADR; `/ws` and
  SEC-0012 stay fully in force until a follow-up mission cuts over.
