# SEC-0012. WebSocket Origin Validation Policy — Default-Deny Absent Origin

Date: 2026-06-10
Last-reviewed: 2026-08-27
Tier: B
Status: Accepted

## Related

References: SEC-0005, SEC-0001, SEC-0002, SEC-0004, COM-0021, COM-0006, CHE-0049, CHE-0062

## Context

CWE-346 Origin Validation Error and CWE-1385 Insufficient Standardized Cross-Origin Restrictions enable Cross-Site WebSocket Hijacking (CSWSH): a browser context loaded from an attacker-controlled origin opens a WebSocket to the target carrying victim cookies, because the browser cross-origin policy does not restrict WS upgrades on its own. The defence is server-side: reject the upgrade when `Origin` does not authenticate the loading context. SEC-0005:R3 ("identity at the infrastructure boundary") names this surface but is silent on Origin semantics; SEC-0012 narrows authenticity at the WS upgrade and supplements SEC-0005 per SEC-0001:R3. The vulnerable site is `crates/cherry-pit-web/src/projection/handlers.rs:389-392` where absent `Origin` is unconditionally permitted.

## Decision

Every cherry-pit-web WS upgrade defaults to rejecting absent `Origin` headers; consumers opt into permissive posture via a typed policy carried on a library-owned value type that also carries the WS connection cap, so a surface cannot take one without the other.

R1 [5]: Every cherry-pit-web router builder that mounts a WS upgrade accepts a `WsPolicy` value carrying both `max_connections` and a `WebSocketOriginPolicy`; the parameter is positional after `limits` per the CHE-0049 amendment grammar. This covers `build_projection_router` and `serve::build_router`/`serve::start`; a surface with no WS upgrade (the CQRS `build_router`) takes no `WsPolicy`

R2 [5]: `WebSocketOriginPolicy::Strict` is the default on every WS-bearing surface; absent `Origin` headers are rejected at the WS upgrade with `403 FORBIDDEN`

R3 [5]: `WebSocketOriginPolicy::AllowAbsent` is the documented escape hatch for non-browser clients; consumers electing it accept CWE-346 / CWE-1385 risk

R4 [5]: `WebSocketOriginPolicy` and `WsPolicy` carry `#[non_exhaustive]`; future authentication knobs land as new fields on `WsPolicy` per CHE-0062:R6 pattern

R5 [6]: The `origin_policy` field is consumer-electable per SEC-0005 boundary discretion; the `max_connections` field on the same carrier is a SEC-0003 availability layer and remains unconditional per CHE-0062:R4. Fusing them onto one carrier does not make either electable-by-omission — a consumer must name both

R6 [5]: `Origin` and `Host` are each parsed exactly once into a private validated-authority type whose only constructor is that parse; the comparison operates solely on two validated, normalised values. An authority that is malformed, ambiguous, or carries userinfo is unrepresentable as a comparison operand rather than rejected by a runtime guard over raw strings (SEC-0002:R3 parse-don't-validate at the WS trust boundary)

## Consequences

Status-quo deployments break loudly on the signature change (semver-major); non-browser clients without an `Origin` header must explicitly opt into `AllowAbsent` at router construction. The `#[non_exhaustive]` attribute on `WsPolicy` blocks the struct-literal idiom outside the crate; consumers construct via `WsPolicy::new(max_connections)` — which yields `Strict` without naming it — or `WsPolicy::permissive_for_tests()`, matching the `LayerLimits` precedent. A future `AllowMatching` variant (Origin must match an allowlist) is reserved but out of scope. No general HTTP-header validation policy is established here — scope is limited to WS Origin; CSP source-list, CORS allow-origin, and Sec-Fetch-* validation remain ungoverned and may motivate future SEC ADRs.

### REVERSED 2026-08-27: separate carriers, falsified by U1

This ADR originally reasoned: "`WsAuthLimits` is a sibling to `LayerLimits` rather than an extension of it, keeping CISQ primaries MECE per COM-0028 (authenticity vs availability)." **That rationale is reversed. U1 is the falsifier.**

Splitting the WS connection cap (availability, on `LayerLimits`) from the WS origin policy (authenticity, on `WsAuthLimits`) let a surface take one carrier and omit the other with nothing in the type system objecting. `serve::build_router` did exactly that — it took the cap and hardcoded `AllowAbsent` at the upgrade, so no origin-strict WebSocket ran anywhere in production despite R2 declaring `Strict` the default since 2026-06-10. The rule was ratified and the code was green; the omission was invisible because it was a *missing* argument, not a wrong one. MECE-by-CISQ-primary is a property of the taxonomy, not of the call site, and the call site is where the omission happened.

The replacement principle is **capability-cohesion**: a carrier groups the knobs one capability needs to be safe, not the knobs sharing a CISQ label. `WsPolicy` fuses `max_connections` and `origin_policy` because mounting a WS upgrade safely requires both, making the omission a compile error. COM-0028's MECE obligation is discharged by each ADR mapping to its CISQ primaries (SEC-0001:R1), not by carrier shape; R5 records that SEC-0012 carries one SEC-0003 number. Where taxonomy-cohesion and compile-time enforceability conflict at a security boundary, enforceability wins.
