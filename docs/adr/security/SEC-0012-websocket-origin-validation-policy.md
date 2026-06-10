# SEC-0012. WebSocket Origin Validation Policy — Default-Deny Absent Origin

Date: 2026-06-10
Last-reviewed: 2026-06-10
Tier: B
Status: Accepted

## Related

References: SEC-0005, SEC-0001, SEC-0002, SEC-0004, COM-0021, COM-0006, CHE-0049, CHE-0062

## Context

CWE-346 Origin Validation Error and CWE-1385 Insufficient Standardized Cross-Origin Restrictions enable Cross-Site WebSocket Hijacking (CSWSH): a browser context loaded from an attacker-controlled origin opens a WebSocket to the target carrying victim cookies, because the browser cross-origin policy does not restrict WS upgrades on its own. The defence is server-side: reject the upgrade when `Origin` does not authenticate the loading context. SEC-0005:R3 ("identity at the infrastructure boundary") names this surface but is silent on Origin semantics; SEC-0012 narrows authenticity at the WS upgrade and supplements SEC-0005 per SEC-0001:R3. The vulnerable site is `crates/cherry-pit-web/src/projection/handlers.rs:389-392` where absent `Origin` is unconditionally permitted.

## Decision

The projection WS upgrade defaults to rejecting absent `Origin` headers; consumers opt into permissive posture via a typed policy parameter carried on a library-owned value type.

R1 [5]: `build_projection_router` accepts a `WsAuthLimits` value carrying a `WebSocketOriginPolicy`; the policy parameter is positional between `limits` and `extra_routes` per the CHE-0049 amendment grammar

R2 [5]: `WebSocketOriginPolicy::Strict` is the default; absent `Origin` headers are rejected at the WS upgrade with `403 FORBIDDEN`

R3 [5]: `WebSocketOriginPolicy::AllowAbsent` is the documented escape hatch for non-browser clients; consumers electing it accept CWE-346 / CWE-1385 risk

R4 [5]: `WebSocketOriginPolicy` and `WsAuthLimits` carry `#[non_exhaustive]`; future authentication knobs land as new fields on `WsAuthLimits` per CHE-0062:R6 pattern

R5 [6]: SEC-0012 is consumer-electable per SEC-0005 boundary discretion; this is distinct from SEC-0003 availability layers which remain unconditional per CHE-0062:R4

## Consequences

Status-quo deployments break loudly on the signature change (semver-major); non-browser clients without an `Origin` header must explicitly opt into `AllowAbsent` at router construction. `WsAuthLimits` is a sibling to `LayerLimits` rather than an extension of it, keeping CISQ primaries MECE per COM-0028 (authenticity vs availability). The `#[non_exhaustive]` attribute on `WsAuthLimits` blocks the struct-literal idiom outside the crate; consumers construct via `WsAuthLimits::default()` or `WsAuthLimits::permissive_for_tests()`, matching the `LayerLimits` precedent. A future `AllowMatching` variant (Origin must match an allowlist) is reserved but out of scope. No general HTTP-header validation policy is established here — scope is limited to WS Origin; CSP source-list, CORS allow-origin, and Sec-Fetch-* validation remain ungoverned and may motivate future SEC ADRs.
