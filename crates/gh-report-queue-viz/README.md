# gh-report-queue-viz

Standalone, client-side, animated discrete-event simulation of gh-report's
runtime queue network (mission adr-fmt-t63uo; topology grounded in
adr-fmt-223sd). No dependency on a running gh-report; no trace capture — the
simulation is entirely self-contained and runs in-browser.

gh-report is NOT one steady conveyor. It is three distinct triggers feeding a
per-packet WRITE side, joined by a barrier, then a per-RUN READ side, plus a
continuous SERVE path:

- **Trigger 1 — scheduled sweep.** `spawn_collection_loop` fires a run;
  `SweepSaga` walks `SweepPhase` (`Init` → `Resumed`/`BaselineReused` →
  `AwaitingBatch` → `BatchDrained` → `Completed`/`Failed`) and batch-enqueues
  many `JobSource::ScheduledBatch` jobs.
- **Trigger 2 — webhook.** `webhook_handler` enqueues a single
  `JobSource::External { id, kind }` job per event, never gated on any batch
  barrier.
- **Trigger 3 — warm start.** `warm_start_from_baseline` renders straight from
  the current `EvidenceProjection`, bypassing the queue/workers entirely.

WRITE side (per job): `WorkQueue` → `worker_loop` / `LiveEvaluator::evaluate`
(GitHub query) → `JobOutcome` → `delivery_loop` → `record_repo` folds an
`EvidenceProjectionEvent` into `EvidenceProjection`.

BARRIER: `BatchTracker` — a scheduled sweep's run blocks until every
`ScheduledBatch` job of that run has drained. Webhook jobs never gate on it.

READ side (per RUN, not per packet): `finalize_and_publish` →
`build_cached_pages` (memoized, zstd `CachedBody`) → `commit_cached_pages`
(atomic `ArcSwap` swap, generation++) → `PageUpdateEvent` broadcast on the WS
channel — fires exactly once when a run finalizes, never once per successful
job.

SERVE path (continuous, per request): `cache_fallback` reads whatever
generation the `ArcSwap` currently holds, independent of any run completing.

- `src/sim.rs` — pure, host-testable discrete-event sim core mirroring the
  real types (`JobSource`, `SweepPhase`, `JobOutcome`, `EnqueueResult`,
  `EvidenceProjectionEvent`, `WorkQueue`, `BatchTracker`, `EvidenceProjection`,
  `CachedPage`/`CachedBody`, `PageUpdateEvent`, `ArcSwap` generation) so a
  future live feed could deserialize real events into the same types. Run its
  invariant tests with `cargo test -p gh-report-queue-viz`.
- `src/view.rs` (`wasm32`-only) — raw `web-sys` + leptos reactive-primitive
  DOM wiring (no `view!` macro) that renders the three triggers, the
  write/barrier/read/serve split, animates packets colored by `JobSource`, and
  exposes tunable arrival-rate buttons plus a one-shot warm-start button.

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

