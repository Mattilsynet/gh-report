# gh-report-queue-viz

Standalone, client-side, animated discrete-event simulation of gh-report's
runtime queue network (mission adr-fmt-21eo5; topology grounded in
adr-fmt-223sd). No dependency on a running gh-report; no trace capture — the
simulation is entirely self-contained and runs in-browser.

- `src/sim.rs` — pure, host-testable discrete-event sim core: bounded FIFO
  `WorkQueue` with dedup on `domain_key`, `BatchTracker` join-barrier, a
  16-worker M/M/c station, and a departure tail (`EvidenceProjection` fold →
  `DeliveryTail` publish). Run its invariant tests with
  `cargo test -p gh-report-queue-viz`.
- `src/view.rs` (`wasm32`-only) — raw `web-sys` + leptos reactive-primitive
  DOM wiring (no `view!` macro) that renders the network, animates packets
  colored by `JobSource`, and exposes tunable arrival-rate buttons.

## Run it in a browser

```sh
cd crates/gh-report-queue-viz
wasm-pack build --target web --out-dir pkg --dev
python3 -m http.server 8787
# open http://localhost:8787/index.html
```

Equivalent `wasm-bindgen-cli` form (no `wasm-pack` install required, matches
the exact CI-verified build command):

```sh
cd crates/gh-report-queue-viz
cargo build --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir pkg \
  ../../target/wasm32-unknown-unknown/debug/gh_report_queue_viz.wasm
python3 -m http.server 8787
# open http://localhost:8787/index.html
```

`bootstrap.js` is an external ES module (CSP `script-src 'self'
'wasm-unsafe-eval'`, no inline script) that `init()`s the wasm module, then
drives the simulation clock via `setInterval(() => tick(), 80)` — the
animation cadence lives in JS so the crate's `web-sys` feature surface stays
limited to what `gh-report-web-client` already declares in the workspace
`Cargo.toml` (no `requestAnimationFrame`/`Performance` gating needed).
