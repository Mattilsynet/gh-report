# CHE-0086. Generic Read-Serve Surface in Cherry Pit Web

Date: 2026-07-02
Last-reviewed: 2026-07-02
Tier: B
Status: Accepted
Crates: cherry-pit-web, gh-report

## Related

References: CHE-0049, CHE-0084, CHE-0029, CHE-0005, CHE-0050, CHE-0025, CHE-0062

## Context

gh-report contains a generic read-serve HTTP pipeline: path normalization, security headers, ETag and 304 handling, zstd negotiation, cached page responses, and WebSocket live updates behind a ServerState seam. CHE-0084:R1 makes the donor slice extraction-eligible when its public contract avoids org, GitHub API, report, credential, and tenant-policy vocabulary; this slice passes that test. The unresolved question is placement. cherry-pit-web is the axum adapter crate already hosting command-gateway HTTP and typed projection read/WS surfaces, but CHE-0049:R8 excluded the donor's static-content/cache surface from v0.1. This ADR narrows that exclusion: arbitrary static-file serving remains out of scope, while a generic read-serve pipeline with consumer-owned state is in scope.

## Decision

cherry-pit-web is the home for the generic read-serve HTTP pipeline. This amends CHE-0049:R8 without superseding CHE-0049: the R8 rejection continues to forbid arbitrary static-content and static-site cache serving, but it no longer excludes generic HTTP read-serving primitives parameterised by a consumer-owned ServerState implementation. The command-gateway surface, typed projection read/WS surface, and generic read-serve surface are sibling adapter surfaces inside cherry-pit-web.

R1 [5]: The generic read-serve HTTP pipeline whose public contract is path normalization, security headers, ETag and 304 negotiation, zstd response compression, cached response bodies, and WebSocket live-update transport belongs in `cherry-pit-web` when the contract can be named without org, repository, GitHub API, report rendering, credential, or tenant-policy vocabulary per CHE-0084:R1

R2 [5]: This ADR amends CHE-0049:R8 by distinguishing generic read-serve from arbitrary static-file serving: `CachedPage`, cache-fallback policy, file-root resolution, and static-site hosting remain out of scope unless separately ruled, while consumer-supplied page values and read-serving transport mechanics are in scope for `cherry-pit-web`

R3 [5]: The generic read-serve surface is a third sibling adapter surface next to the CHE-0049 command-gateway surface and the CHE-0049:R11/R12 typed projection read/WS surface; it MUST NOT be implemented by folding the three surfaces into one unified builder or by making read-side code depend on command-gateway router state

R4 [5]: The ServerState seam for the generic read-serve surface is defined in `cherry-pit-web` and implemented by downstream consumers such as gh-report, following COM-0012:R1/R5; the implementation remains in the consumer crate and no reverse dependency from `cherry-pit-web` to gh-report is permitted

R5 [5]: ServerState dispatch is static: router builders and handlers are generic over the concrete state type, and no `Box<dyn ServerState>`, erased state registry, runtime backend selector, or object-safe trait-object form is permitted, consistent with CHE-0005:R1, CHE-0049:R1, and CHE-0050:R2/R4

R6 [5]: The lifted handlers and WebSocket/broadcast code MUST use plain `async fn` or RPITIT-style futures only; `#[async_trait]`, boxed futures introduced for trait erasure, and any dependency that pulls `async-trait` into the `cherry-pit-web` feature graph are forbidden per CHE-0025:R1/R2 and CHE-0029:R4

R7 [5]: The placement MUST preserve the CHE-0029 crate DAG: `cherry-pit-core` stays free of axum, tokio, tracing, tower-http, and filesystem-serving dependencies; `cherry-pit-web` may host axum/tower-http adapter mechanics per CHE-0029:R5, and the extraction MUST introduce no cherry-pit to pardosa dependency edge

R8 [5]: Shared transport helpers used by command-gateway, projection, and generic read-serve surfaces live as intra-crate cherry-pit-web plumbing per CHE-0049:R14; public re-exports are limited to deliberately named surface types and reusable value types, while implementation modules remain private unless a later ADR names a public contract

R9 [6]: WebSocket origin validation and availability limits on the generic read-serve surface follow the same obligations as the existing cherry-pit-web WS/read surfaces: SEC-0012 governs Origin policy and CHE-0062 governs library-owned per-layer limits without exposing consumer config types in public signatures

R10 [5]: CHE-0010 remains only contextual for this placement: it governs DomainEvent supertrait bounds and is not the pardosa-severance authority for this slice; because the read-serve pipeline has no pardosa dependency, CHE-0084's pardosa-placement machinery is not engaged

## Consequences

Positive: gh-report can move reusable read-serving transport code into the existing axum adapter crate without adding a fourth home or changing the cherry-pit-core dependency budget.

Negative: cherry-pit-web's charter broadens from command-gateway plus typed projection read/WS to an HTTP adapter family, so its public surface needs deliberate naming and feature discipline.

Open / deferred: arbitrary static file serving, bundled UI hosting, and static-site cache policy remain outside cherry-pit-web until a future ADR explicitly reverses the remaining CHE-0049:R8 exclusion.

## Rejected Alternatives

**Leave the generic read-serve pipeline in gh-report.** Rejected because the slice passes CHE-0084:R1 and the existing cherry-pit-web DAG layer already owns the axum/tower-http transport primitives it consumes.

**Create a new read-serve crate.** Rejected because the code is adapter transport plumbing already inside the cherry-pit-web responsibility band; a new crate would create another governance island without improving the CHE-0029 DAG.

**Supersede CHE-0049 wholesale.** Rejected because CHE-0049's command-gateway, projection, static-dispatch, middleware, and route-version rules remain active. Only the R8 line between arbitrary static-content serving and generic read-serve transport needed amendment.

**Dynamic ServerState or async-trait erasure.** Rejected because it would contradict CHE-0005:R1, CHE-0049:R1, CHE-0050:R2/R4, and CHE-0025 by replacing compile-time typed wiring with runtime dispatch.
