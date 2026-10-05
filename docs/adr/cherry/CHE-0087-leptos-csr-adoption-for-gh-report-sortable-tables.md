# CHE-0087. Leptos CSR Adoption for gh-report Sortable Tables

Date: 2026-07-09
Last-reviewed: 2026-10-04 — refined — align unsafe posture, host/wasm boundary, serving and configured CI with current source; dependency pins remain manifest-owned (mission:stale-doc-repair-20261004)
Tier: B
Status: Accepted
Crates: gh-report, gh-report-web-client

## Related

References: CHE-0007, CHE-0086, RST-0005, SEC-0004, RST-0004, RST-0002, SEC-0009

## Context

Server-rendered tables remain readable without the client; Leptos CSR progressively adds sorting without a server round-trip. Cargo.toml and Cargo.lock own dependency versions. The client root currently uses unconditional `#![forbid(unsafe_code)]`, consistent with RST-0005:R1 and CHE-0007; no deny-based exception is needed. This constrains the local crate, not unsafe inside dependencies. A future exception would require the dedicated justification and safety argument in RST-0005:R2 and SEC-0004:R4.

## Decision

Use Leptos CSR (feature `csr`, no nightly) for sortable-table progressive enhancement, retain the workspace unsafe prohibition, and serve consumer-owned compiled assets through the existing read-serve pipeline.

R1 [5]: Use Leptos with feature `csr`, defaults disabled and no `nightly`, compiled via wasm-bindgen to `wasm32-unknown-unknown` for sortable-table enhancement. Dependency versions are declared in Cargo.toml and resolved in Cargo.lock, not duplicated here.

R2 [5]: `gh-report-web-client` retains `#![forbid(unsafe_code)]` at its crate root under RST-0005:R1. This ADR grants no unsafe exception and does not amend CHE-0007 or RST-0005 to exclude the client. Generated code rejected by that lint requires a separate architectural decision, not an inner `#[allow]`.

R3 [5]: No hand-authored `unsafe` is permitted in `gh-report-web-client`. Dependency review characterizes generated and transitive unsafe exposure with `cargo-geiger` under SEC-0009:R3; the local lint is not a dependency-wide absence-of-unsafe proof. No executed audit or cargo-geiger CI gate is asserted here.

R4 [5]: Leptos/wasm-bindgen/web-sys/js-sys declarations remain in `[workspace.dependencies]` per RST-0004:R1, consumed as browser-target dependencies by the client. Minimize features per RST-0004:R2 and review dependency updates separately per RST-0002:R2; introduce no client dependency edge into any `cherry-pit-*` crate.

R5 [9]: Build the WASM binary and JS glue out-of-band and commit the assets embedded by `include_bytes!`/`include_str!` into consumer-owned `LazyLock<CachedPage>` statics in `crates/gh-report/src/app/state/mod.rs`. Never regenerate them in a host `build.rs`. Docker copies committed assets and builds the host application; it does not regenerate WASM.

R6 [9]: The client is a workspace member but not a default-member: bare builds omit it, while `--workspace` includes its host-compatible pure sort module. Browser dependencies and DOM wiring are wasm-target-only. Host workspace builds must remain green. Configured CI job `build-test-lint` precompiles the host library and wasm tests, builds wasm release, and lints both targets.

R7 [5]: Serve the compiled bundle as gh-report-owned `CachedPage` values through the generic read-serve transport (CHE-0086:R2/R4), like `style.css`/`ws.js`. Keep consumer asset policy outside `cherry-pit-web`; this grants no arbitrary-static-file hosting carve-out.

R8 [5]: gh-report's served Content-Security-Policy uses `ServeOptions::builder().csp_override(...)` in `crates/gh-report/src/server.rs`, adding only `'wasm-unsafe-eval'` to baseline `script-src`, not `'unsafe-eval'`. The shared `cherry-pit-web` default remains unchanged; the consumer owns this WASM-specific override.

R9 [5]: Server-rendered HTML remains pre-sorted and fully readable with WASM absent, disabled, or failed to load; the Leptos client only progressively enhances already-correct markup and never becomes a rendering requirement.

R10 [5]: The `wasm32-unknown-unknown` target is a target addition under RST-0001, not a channel bump. `rust-toolchain.toml` and `[workspace.package].rust-version` own the toolchain and MSRV; this adoption does not change their floor.

R11 [5]: Any no-mount CSR path in gh-report-web-client — i.e. progressive enhancement that never calls mount_to_body/mount_to (R9) — MUST, before constructing any Effect::new or other reactive primitive, establish (a) an initialized async executor via Executor::init_wasm_bindgen() and (b) a page-lifetime reactive Owner that is set as the current owner and retained for the document lifetime (let owner = Owner::new(); owner.set(); std::mem::forget(owner);), mirroring Leptos's own hydrate_islands idiom. Without both, effects are constructed but never run (their driving future is never spawned and no owner context exists), so the enhancement silently no-ops while server HTML stays correct per R9. This is an ADDED runtime-init obligation created by the no-mount choice, consistent with and not a reversal of R9.

R12 [5]: Keep deterministic directed sorting: numeric `N/A`/empty values map to `f64::NEG_INFINITY`, while generic parse failures remain `None`; indeterminate numeric evidence precedes zero ascending and follows it descending, with lexical ties among indeterminate values. Status indeterminates remain last in both directions. Preserve these semantics in the pure comparator, wasm caller and tests.

## Consequences

+ becomes easier: users sort large tables client-side with no page reload or extra query parameters; the read-serve pipeline gains a reusable pattern for client-rendered enhancements.
− becomes harder: dependency unsafe exposure still requires review; source changes require regenerating and committing the browser assets; the consumer owns a CSP override and must bootstrap the no-mount runtime (R11).
risks/migration: CLI/library drift remains an operational risk; wasm tooling must match Cargo.lock. Configured `build-test-lint` runs status vocabulary, directed-sort compiler, headless Chrome and guard-regression checks plus committed import-key validation. These checks are not byte-freshness equivalence: `tools/verify_web_client.py` implements local bundle-byte comparison, not an enabled hosted byte-freshness gate. This source inspection establishes configured behavior, not successful CI, browser execution or bundle freshness.

## Rejected Alternatives

**Server-side sorting via query parameters.** Rejected because it requires a full page reload per sort action and adds server-side query complexity for a purely presentational concern; the mission is interactive client rendering.

**`#[allow(unsafe_code)]` inside a `#![forbid(unsafe_code)]` crate.** Rejected because `forbid` cannot be overridden by an inner `#[allow]` (CHE-0007 Consequences); this ADR grants no crate-level omission either.

**Relaxing `forbid(unsafe_code)`.** Rejected — RST-0005/CHE-0007 remain in force, including for the client; generated FFI does not justify an assumed exception.

**A new static-asset-hosting ADR reversing CHE-0049:R8 wholesale.** Rejected because the compiled bundle fits inside CHE-0086's already-sanctioned `CachedPage` pattern; no fresh static-file carve-out is needed.
