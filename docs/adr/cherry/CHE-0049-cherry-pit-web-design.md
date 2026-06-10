# CHE-0049. Cherry Pit Web Design

Date: 2026-05-09
Last-reviewed: 2026-05-11
Tier: B
Status: Accepted

## Related

References: CHE-0005:R1, CHE-0014:R1, CHE-0018:R3, CHE-0021:R3, CHE-0024:R2, CHE-0024:R3, CHE-0024:R4, CHE-0029:R5, CHE-0030, CHE-0039:R1, CHE-0039:R2, CHE-0039:R3, CHE-0045, CHE-0046:R3, CHE-0046:R5

## Context

cherry-pit-web is an HTTP adapter over CommandGateway — it translates HTTP requests into domain commands, dispatches them via the gateway, and maps outcomes to HTTP responses. The donor (quics-web) is a static-content cache whose core abstraction (ServerState + CachedPage + PageUpdateEvent) does not fit this use case. The reusable donor surface is limited to security/transport plumbing: path normaliser, security headers, ETag, zstd compression, signal handling, and config builder.

The central question is how CommandGateway and EventStore seat into axum State without dynamic dispatch. CHE-0005:R1 forbids `Box<dyn EventStore>`; CHE-0029:R5 names axum; CHE-0039 mandates correlation propagation; CHE-0021:R3 mandates ErrorCategory-driven status mapping. These constraints fix most of the shape.

## Decision

cherry-pit-web exposes a generic typed state and router builder parameterised on concrete gateway and store types. v0.1 is HTTP-only with no WebSocket surface.

R1 [5]: The crate exposes `AppState<G, S>` with `G: CommandGateway<...>` and `S: EventStore<...>` as type parameters with no dynamic dispatch, and a `build_router<G, S>(...)` function that returns a configured `Router` — the consumer instantiates with concrete types in their main

R2 [5]: Authentication is out of scope — the crate ships zero default auth middleware and documents the `extra_routes` merge point as the consumer's auth-attachment surface, consistent with CHE-0001 P2 which requires any auth hook to be a typed extension point rather than default-permissive

R3 [5]: v0.1 is HTTP-only with no WebSocket surface — the donor's WebSocket session, ping/pong, lag handling, and CSWSH validator code stays in quics-web for gh-report's static-cache use, deferring event-stream WebSocket to a later increment when checkpoint and replay-on-reconnect logic can be designed properly

R4 [5]: Domain-rejected commands return 422 Unprocessable Entity with the typed error preserved losslessly in the response body per CHE-0015, while 409 Conflict is reserved exclusively for concurrency conflicts from expected_sequence mismatch per CHE-0041:R3

R5 [5]: Correlation context is extracted from the inbound `traceparent` header (W3C Trace Context) as the primary source with `X-Correlation-ID` as fallback — absent headers produce an explicit `CorrelationContext::none()` per CHE-0039:R2 which forbids Default — and every response echoes the correlation identifier

R6 [5]: Idempotency keys are consumer-supplied via the `Idempotency-Key` header and propagated into the command DTO at the boundary — the web layer does not auto-generate keys when the header is absent, because CHE-0046:R3 requires stable keys and auto-generation defeats that purpose

R7 [5]: Read endpoints that load by aggregate_id are in v0.1 scope, with EventStore::load of an unknown aggregate returning 200 with an empty list per CHE-0019:R1 rather than 404

R8 [5]: No static-content or cache surface ships in v0.1 — CachedPage, html_cache, cache_fallback, and resolve_cache_key are donor concepts that do not apply to a command-gateway adapter, though domain-free utility primitives (compute_etag, compress_zstd, security_headers, normalize_request_path, sanitize_path_segment) are reused

R9 [5]: URL paths use a `/v1/` prefix to establish a versioned DTO contract, with deprecation cadence deferred beyond v0.1

R10 [5]: HTTP error mapping consumes ErrorCategory via accessor per CHE-0021:R3 — Retryable maps to 503 with Retry-After, Terminal::AggregateNotFound to 404, Terminal infra errors to 500, StoreLocked to 503, and cancellation after persist per CHE-0046:R5 to 202 Accepted with idempotency-key echo indicating the outcome is unknown to the client but replay is safe

R11 [5]: Under `feature = "projection"`, a second adapter surface ships alongside default `cqrs` — independent builders `build_cqrs_router` and `build_projection_router`, no unified builder; CHE-0049:R3's no-WebSocket clause is narrowed to permit WS push of projection snapshot-delta DTOs since CHE-0048:R2's snapshot is the durable checkpoint discharging CHE-0024:R3 and CHE-0024:R4; direct `EventEnvelope<E>` or EventBus subscribe over WS remains forbidden per CHE-0024:R2

R12 [5]: The projection adapter exposes `ProjectionSource` as a generic parameter `P: ProjectionSource` on `ProjectionState<P>` and `build_projection_router<P>`, with no `Box<dyn ProjectionSource>` or trait-object form per CHE-0049:R1, CHE-0005:R1, CHE-0050:R4 — `ProjectionSource` is a transport-side observer distinct from `cherry-pit-core::Projection` (storage adapter lives in cherry-pit-projection per CHE-0048), and MUST NOT introduce a `Deserialize` bound on any domain type per CHE-0014:R2

R13 [5]: DTO versioning uses URL prefix `/v1/` on HTTP routes per CHE-0049:R9 unchanged, AND a WS DTO envelope field `"v": 1` on the unversioned `/ws` route — the WS route lacks a clean URL-prefix migration path so envelope-versioning is the load-bearing version contract there, while the HTTP `/v1/` prefix remains the contract for request/response shapes

R14 [5]: Shared transport plumbing reused by both `cqrs` and `projection` adapters lives in a private `cherry-pit-web::middleware` module covering `security_headers`, correlation propagation per CHE-0049:R5, and the ErrorCategory→HTTP envelope per CHE-0049:R10 — `middleware` is implementation detail per CHE-0030:R2 and only the items already named in CHE-0049:R8 (`compute_etag`, `compress_zstd`, `security_headers`, `normalize_request_path`, `sanitize_path_segment`) remain re-exported from `lib.rs` per CHE-0030:R1

## Consequences

WebSocket deferral means v0.1 consumers observe state via HTTP reads only — consistent with CHE-0024:R2 (no EventBus subscribe), so near-real-time updates require polling.

Static-content deferral means consumers wanting a bundled UI host it separately or attach via extra_routes, keeping the crate focused on the command-gateway adapter role.

Generic `AppState<G, S>` means no pre-built Router ships — only a parameterised builder; the consumer's main performs type instantiation, a one-time cost.

Under `feature = "projection"`, snapshot-delta WS push is permitted since the snapshot is the durable checkpoint. Consumers may compose a binary with both `cqrs` and `projection` by merging both routers via `Router::merge`. The shared `middleware` module costs one-time wiring but eliminates duplicated security-header, correlation, and error-envelope code.

## Rejected Alternatives

**Built-in auth middleware** — No CHE rule mandates in-crate auth, and shipping default auth would either be default-permissive (violating CHE-0001 P2 Security ranking) or force an opinionated auth scheme that constrains consumers unnecessarily. The extra_routes merge point provides a typed extension surface.

**Dynamic state / trait-object ports** — A `Box<dyn EventStore>` or `Box<dyn CommandGateway>` state container would violate CHE-0005:R1 which forbids dynamic dispatch over infrastructure ports. The generic typed state satisfies the same ergonomic needs without heap allocation per dispatch.

**WebSocket in v0.1** — The donor's WebSocket implementation serves static-page-cache notification (PageUpdateEvent), not event-stream replay. Adapting it for cherry-pit-web would require designing checkpoint and replay-on-reconnect logic (CHE-0024:R3/R4) in the web layer, adding material surface area with no v0.1 consumer need identified.

**Static-content cache surface (arbitrary content)** — CachedPage and the donor's general static-site-server infrastructure remain out of scope per R8; including arbitrary static-content serving in cherry-pit-web would blur the crate's responsibility boundary and balloon the public API. Projection-published HTML is a different category and ships under `feature = "projection"` per R11–R12: it is the read-side of a typed projection state per CHE-0048, not arbitrary file serving.

**Auto-generated idempotency keys** — CHE-0046:R3 requires stable idempotency keys for boundary-crossing retries. Auto-generating a key when the client omits the header would produce a fresh key per retry attempt, defeating the deduplication purpose.

**Unversioned DTO paths** — Omitting the `/v1/` prefix would make future breaking changes to request/response shapes require either silent incompatibility or an ad-hoc versioning retrofit. The prefix cost is negligible and establishes the convention early.

## Amendment 2026-05-16

R12 extended: `build_projection_router<P>` takes a second parameter `extra_routes: Router` (stateless) merged onto the projection surface after `with_state`. Mirrors the CHE-0049:R2 / CQRS `build_router` extension-point convention so consumers attach auth probes, status pages, or any non-projection routes without re-wrapping the router. The merge happens after state is bound, so `extra_routes` cannot widen the projection state's type surface. Callers with no extras pass `Router::new()`. Additive change, no behaviour change for existing routes.

## Amendment 2026-05-16 (Part 2)

Per CHE-0062 (Library Attaches Availability Layers via Per-Layer Limits), `build_projection_router<P>` gains a `limits: LayerLimits` parameter positioned between `state` and `extra_routes`. The library now unconditionally attaches three SEC-0003 R1/R3 availability layers: `tower_http::limit::RequestBodyLimitLayer` for body cap (413 on exceed), the `http_concurrency_limit` middleware with 503-shedding-not-queueing semantics for in-flight cap, and an `Extension<Arc<tokio::sync::Semaphore>>` extracted by `ws_handler` for the WS connection cap (503 on `try_acquire_owned` failure). `LayerLimits` is a library-owned value type carrying three `usize` fields; the consumer constructs it from any source, the library never sees the consumer's config type. Reverses CHE-0056's layer-prohibitions while preserving the no-`&ValidatedConfig`-in-public-signature stance. Falsifier sites: gh-report's existing SEC-0003 tests at `crates/gh-report/src/infra/server/server.rs:1236,:2164,:2209,:3144,:3559` target the cherry-pit-web router after Track 4.3 migration; a regression in any one falsifies CHE-0062. Signature change is semver-major; cherry-pit-web is internal and `Cargo.lock` is committed, so the workspace tolerates this.

## Amendment 2026-06-10 (SEC-0012)

Per SEC-0012 (WebSocket Origin Validation Policy), `build_projection_router<P>` gains a `ws_auth: WsAuthLimits` parameter positioned between `limits` and `extra_routes`. The library now defaults Origin validation to Strict (absent Origin rejected with 403 FORBIDDEN). Consumers elect `WebSocketOriginPolicy::AllowAbsent` to preserve non-browser-client compatibility, accepting CWE-346 risk explicitly per SEC-0012:R3. Signature change is semver-major; precedent set by CHE-0049 Amendment 2 (CHE-0062 ratification).
