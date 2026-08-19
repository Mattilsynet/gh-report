# CHE-0107. WASM Gate G3: Async-in-Components Posture

Date: 2026-08-01
Last-reviewed: 2026-08-01

Tier: B
Status: Accepted
Crates: gh-report, cherry-pit-core

## Related

References: CHE-0018, CHE-0025, CHE-0105, CHE-0106, PGN-0010, SEC-0014

## Context

CHE-0105 adopts #5 for production and names an open gap (bd adr-fmt-4yn7f G3):
no ADR states the async-in-WASM-components stance. CHE-0018 governs the Rust
trait axis (domain traits synchronous R1; infra ports asynchronous R2; core has
zero async-runtime dependency R3) but the guest-boundary async story is
unaddressed.

The tech review (bd adr-fmt-if886) is decisive here: wasmtime v47 makes
component-to-component SYNC-TO-SYNC fused adapters first-class and optimized
(changelog #13695); async in the Component Model (WASIp3, `Accessor`
concurrency) is still maturing and is NOT the stable baseline. CHE-0018's
sync-domain stance ALIGNS exactly with a synchronous WIT guest boundary: the
guest runs synchronous domain logic, invoked synchronously by the host; all
async (I/O, store, transport) stays HOST-side (CHE-0018:R2/R3), which is
precisely where wasmtime and the event store live. PGN-0010:R5 (public facade
stays synchronous over internally-async backends) is the same shape one axis up.

## Decision

Establish the async posture at the WASM component boundary. This is gate G3
for CHE-0105.

R1 [5]: The host/guest domain-logic boundary is SYNCHRONOUS. The guest domain
  app exposes and is invoked through synchronous WIT functions
  (sync-to-sync fused adapters, wasmtime v47); this directly satisfies
  CHE-0018:R1 (domain traits — `apply`/`handle`/`react` — are synchronous).
  Domain logic running as a WASM guest invoked synchronously by the host
  matches the sync-domain invariant without amendment.

R2 [5]: Async at the component boundary is FORBIDDEN for the guest domain-logic
  seam in this production adoption. The guest does not host an async runtime,
  does not await, and does not import async host capabilities for domain logic.
  All asynchrony (store I/O, transport, timers) stays HOST-side (CHE-0018:R2/R3);
  the guest is a pure synchronous compute unit over copy-by-value inputs.

R3 [5]: The host presents a SYNCHRONOUS facade to the guest over its internally
  async substrate, imitating PGN-0010:R5 (sync facade, async hidden) and the
  established sync-over-async bridge pattern. Backends own their async runtime
  internally; the guest never observes it.

R4 [4]: `cherry-pit-core` retains zero async-runtime dependency (CHE-0018:R3).
  The wasmtime host embedding and any async bridging live in the host/adapter
  crate (CHE-0106:R1), never in core. RPITIT-over-async_trait discipline
  (CHE-0025) on the host-side infra ports is unaffected by the guest boundary.

R5 [4]: WASIp3 / Component-Model async is DEFERRED, not adopted. Should a future
  domain-app requirement genuinely need async at the guest boundary, it requires
  a new ADR superseding or amending this one — it is not admitted incrementally.
  The conservative default (sync-only guest seam) holds until the async
  Component-Model surface stabilizes (bd adr-fmt-if886: WASIp3 in-progress as of
  2026-07, WASI 0.2 remains the pinned-stable floor).

## Consequences

+ becomes easier: the boundary rides the mature, optimized sync-to-sync fused
  adapter path (wasmtime v47), avoiding the still-maturing async Component-Model
  surface entirely.
+ becomes easier: CHE-0018 is satisfied, not amended — the guest seam is a
  natural home for the sync-domain invariant, and async stays where it already
  lives (host-side).
- becomes harder: a domain app that genuinely needs async at the guest boundary
  cannot have it under this ADR; it needs a superseding ADR (R5). Acceptable —
  no such requirement exists, and the sync path is the stable one.
risks/migration: additive — no async-runtime dependency enters core; the sync
  facade reuses the established sync-over-async bridge shape. Gate G3 for
  CHE-0105; no code lands before CHE-0105:R3 is satisfied.
