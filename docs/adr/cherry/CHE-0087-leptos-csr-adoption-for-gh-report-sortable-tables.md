# CHE-0087. Leptos CSR Adoption for gh-report Sortable Tables

Date: 2026-07-09
Last-reviewed: 2026-07-09
Tier: B
Status: Accepted
Crates: gh-report, gh-report-web-client

## Related

References: CHE-0007, CHE-0086, RST-0005, SEC-0004, RST-0004, RST-0002, SEC-0009

## Context

gh-report's HTML tables are server-rendered and static; users cannot re-sort a column without a full page reload and a server-side query change. Leptos 0.8.20 (Rust→WASM CSR) via wasm-bindgen 0.2.126 is MSRV-compatible with the pinned 1.96 toolchain (Leptos MSRV 1.88), needs no nightly, and sorts client-side with no server round-trip. The tension is RST-0005/CHE-0007's workspace `#![forbid(unsafe_code)]`: wasm-bindgen's FFI glue is generated `unsafe`. The current pins compile clean under `forbid`, but `forbid` cannot be `#[allow]`-overridden should a future wasm-bindgen emit `unsafe` the lint rejects — so this crate uses `#![deny(unsafe_code)]`, banning hand-authored `unsafe` while tolerating generated FFI glue across version drift. RST-0005 R2 and SEC-0004 R4 require a dedicated ADR before a crate omits the workspace default; this is that ADR.

## Decision

Adopt Leptos CSR (feature `csr`, no nightly) as gh-report's client-rendering stack for sortable tables, ship it in a new crate granted a scoped, documented exception to the workspace's forbid(unsafe_code) default, and serve the compiled bundle through the existing read-serve pipeline as a progressive enhancement.

R1 [5]: Adopt Leptos 0.8.20 (feature = "csr" only, no "nightly") compiled via wasm-bindgen 0.2.126 to `wasm32-unknown-unknown` as the client-side rendering stack for gh-report's interactive sortable-table enhancement; this is the workspace's first client-render precedent.

R2 [5]: `gh-report-web-client` omits `#![forbid(unsafe_code)]` and uses `#![deny(unsafe_code)]` instead: `deny` bans hand-authored `unsafe` while tolerating wasm-bindgen's generated FFI glue across version drift, whereas `forbid` cannot be `#[allow]`-overridden (per CHE-0007's Consequences). Upon acceptance this ADR amends CHE-0007's member list and RST-0005 R1 to exclude that one named crate, never workspace-wide.

R3 [5]: No hand-authored `unsafe` is permitted in `gh-report-web-client`; the only unsafe present originates from wasm-bindgen's macro-GENERATED FFI glue marshalling values across the JS/WASM boundary — the exact FFI case RST-0005 R2 anticipates — and `cargo-geiger` (SEC-0009 R3) characterizes that generated-plus-transitive surface as an ADR-cited artefact on each dependency review.

R4 [5]: All Leptos/wasm-bindgen/web-sys/js-sys dependencies are declared in `[workspace.dependencies]` per RST-0004 R1, with `default-features = false` and only CSR-required features enabled (RST-0004 R2); the addition lands as its own dedicated, reviewable `Cargo.lock` diff PR per RST-0002 R2, and introduces no dependency edge into any `cherry-pit-*` crate.

R5 [9]: The `.wasm` binary and its JS glue are built out-of-band (`cargo build --target wasm32-unknown-unknown --release` + `wasm-bindgen --target web`), committed to the repository, and embedded into `gh-report` via `include_bytes!`/`include_str!` into `LazyLock<CachedPage>` statics mirroring the existing `style.css`/`ws.js` pattern (`crates/gh-report/src/app/state.rs`); regeneration is a Dockerfile/CI concern, never a host `build.rs`.

R6 [9]: `gh-report-web-client` is excluded from the host workspace's default build set (`cargo build --workspace`) so the host toolchain never compiles wasm-only dependencies; `cargo build --workspace --all-features --locked` MUST stay green throughout, and the crate builds only under an explicit `--target wasm32-unknown-unknown` invocation or CI's dedicated wasm step.

R7 [5]: The compiled bundle is served through gh-report's existing generic read-serve surface (CHE-0086) as an additional `CachedPage` value, the same already-sanctioned path serving `style.css`/`ws.js`; this is not a new arbitrary-static-file carve-out and CHE-0049:R8's exclusion remains otherwise intact.

R8 [5]: gh-report's own served Content-Security-Policy, set via `ServerConfig::builder().csp_override(...)` at its serve-construction sites, adds ONLY `'wasm-unsafe-eval'` to `script-src` versus the shared baseline, because that token is strictly narrower than `'unsafe-eval'` (WASM-compile only, required by `WebAssembly.instantiateStreaming`); `cherry-pit-web`'s shared `DEFAULT_CSP` (`serve/runtime.rs`) is unchanged.

R9 [5]: Server-rendered HTML remains pre-sorted and fully readable with WASM absent, disabled, or failed to load; the Leptos client only progressively enhances already-correct markup and never becomes a rendering requirement.

R10 [5]: Adding the `wasm32-unknown-unknown` compilation target is a target-add under RST-0001, not a toolchain channel bump; the pinned 1.96 channel and MSRV stay unchanged, since Leptos 0.8.20's own MSRV (1.88) already sits below that floor.

## Consequences

+ becomes easier: users sort large tables client-side with no page reload or extra query parameters; the read-serve pipeline gains a reusable pattern for client-rendered enhancements.
− becomes harder: `gh-report-web-client` sits outside the workspace's uniform forbid(unsafe_code) guarantee, requiring cargo-geiger review each dependency bump (SEC-0009 R3); the build gains a wasm32 leg regenerated and re-committed on source changes; gh-report's CSP is no longer one shared constant.
risks/migration: this ADR amends CHE-0007's enumerated list and RST-0005 R1 only upon acceptance — no CHE-0007 file edit happens while Proposed, mirroring how CHE-0086 amends CHE-0049:R8 without editing CHE-0049. Release-profile (CHE-0026) bundle-size tuning is deferred to sub-mission 2; wasm-bindgen-cli/library version drift is an open operational risk.

## Rejected Alternatives

**Server-side sorting via query parameters.** Rejected because it requires a full page reload per sort action and adds server-side query complexity for a purely presentational concern; the mission is interactive client rendering.

**`#[allow(unsafe_code)]` inside a `#![forbid(unsafe_code)]` crate.** Rejected because `forbid` cannot be locally overridden by an inner `#[allow]` (CHE-0007 Consequences); the only mechanism is crate-level omission of `forbid`, scoped and documented here.

**Relaxing `forbid(unsafe_code)` workspace-wide.** Rejected outright — RST-0005/CHE-0007 stay in force for every other crate; this ADR's exception is scoped to exactly one named crate.

**A new static-asset-hosting ADR reversing CHE-0049:R8 wholesale.** Rejected because the compiled bundle fits inside CHE-0086's already-sanctioned `CachedPage` pattern; no fresh static-file carve-out is needed.
