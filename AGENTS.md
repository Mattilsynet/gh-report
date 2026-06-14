# AGENTS.md — solon

Repo-specific operational notes. General agent/OODA doctrine, bd/beads
conventions, bash hygiene, and the Rust no-`//`-comments rule live in the
global `~/.config/opencode/AGENTS.md` (auto-loaded) — not repeated here.

## What this repo is

Rust workspace (edition 2024, MSRV 1.96, resolver 3, 21 crates) shipping three
binaries plus an ADR-governed library family and a large ADR corpus.

- Binaries (real entrypoints): `adr-fmt` (ADR validator, read-only),
  `adr-srv` (axum GraphQL service over an ADR-corpus projection),
  `gh-report` (GitHub org evidence collector + HTML reporter daemon).
  `pardosa-cli` is a fourth bin; `comment-free` is a doc-lint tool.
- `cherry-pit-*` — event-sourcing substrate consumed by `gh-report`.
- `pardosa*` — `.pgno` event-store substrate + a NATS/JetStream backend
  (`pardosa-nats`). `cherry-pit` does **not** depend on `pardosa` (severed per
  CHE-0010); don't reintroduce that edge.

## Build / test / verify (local cadence; boundary mirrors CI)

- **INNER-LOOP** (every TDD increment, changed crate only):
  ```
  CARGO_TERM_PROGRESS_WHEN=never cargo test -p <crate> --message-format=short
  CARGO_TERM_PROGRESS_WHEN=never cargo clippy -p <crate> --message-format=short -- -D warnings
  ```
  One test: `cargo test -p <crate> <name> --message-format=short`.
- **BOUNDARY** (mission/sub-mission completion, before claiming done):
  ```
  cargo build  --workspace --all-features --locked
  cargo test   --workspace --all-features --locked
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  cargo fmt --all -- --check
  ```
  Exit codes from this tier back the done-claim; the cadence relocates controls
  to where they earn their cost, not weakens verify-before-claim.
- **CI-ONLY** (never in the local loop): CI owns deny, audit, and the two
  tripwire jobs.
- `clippy::pedantic` is the **standing bar**, not an elevation
  (`[workspace.lints.clippy] pedantic = warn` + CI `-D warnings`). New code must
  pass pedantic with zero warnings.
- `rustfmt` runs on **stable defaults only** (RST-0003:R3); there is no custom
  `rustfmt.toml` style. Don't add format config.
- `rust-toolchain.toml` pins channel 1.96 (clippy+rustfmt). Use it; don't bump.

## Live-NATS tests need a pinned `nats-server` (common CI/local gotcha)

`crates/pardosa-nats/src/test_support.rs` spawns a real `nats-server` and
**asserts its `--version` matches `tools/.nats-server-version` (currently
2.14.2)** — it panics on mismatch or if the binary is absent. Affected tests
include `pardosa`'s `dragline::runtime::tests::*jetstream*`. To run them:
install `nats-server` v2.14.2 onto `PATH`. CI installs it in the `test` job
(checksum-verified). `async-nats` is pinned to the `server_2_14` feature to
match.

## CI specifics (`.github/workflows/ci.yml`)

- Triggers on push/PR to `main`. Third-party actions are **SHA-pinned**
  (Dependabot updates them); keep that pattern if you edit the workflow.
- Two custom **tripwire** jobs grep the tree and fail the build — do not break
  the invariants they guard:
  - `async-trait-tripwire`: no `async-trait` in any `cherry-pit-*` dep tree
    (RPITIT only; CHE-0025 / CHE-0029:R4).
  - `gh-report-projection-lock-tripwire`: raw `.projection_state.lock(` is
    banned outside `crates/gh-report/src/app/state.rs` — use
    `AppState::lock_projection()` (CHE-0048:R2).
- `cargo-vet` was removed (deferred per SEC-0009); `cargo-deny`/`cargo-audit`
  are the supply-chain controls.

## Architecture invariants an agent will trip over

These are load-bearing; violating them is an abort-class change:

- **Synchronous public facade.** No `async fn` on the public surface of
  `pardosa::store` / `prelude` (PGN-0010:R5, PGN-0008, PGN-0015:R6). The
  intentional sync-over-async bridge is `pardosa-nats/src/handle.rs::run_op`
  (`block_on`); `std::sync::Mutex` behind the facade is deliberate — do **not**
  "fix" it to `tokio::sync::Mutex` (would break single-writer linearizability).
- **`#[non_exhaustive]` on error enums is mandated** (PGN-0006, CHE-0021 scoped
  to error types) — adding variants is non-breaking, don't remove it. It is
  **not** for serde DTOs.
- **Substrate ring purity:** `pardosa-nats` depends only on tokio + async-nats;
  instrumentation/metrics belong in the `pardosa` adapter ring, never in
  `pardosa-nats`. High-cardinality ids (event_id, ack, stream) go on
  spans/logs, never metric labels (COM-0019:R6).
- **House style:** suppress lints with `#[expect(lint, reason = "…")]`
  (attribute, allowed) — not `//`-comments (forbidden fleet-wide). Use
  `#[allow(.., reason=..)]` only where `#[expect]` would be unfulfilled (e.g. a
  lint that fires under `--test` but not `--all-features`).

## ADR governance (this repo is ADR-driven)

- Corpus under `docs/adr/` (domains: `ground`/GND, `common`/COM, `rust`/RST,
  `security`/SEC, `flow`/FLO, `adr-fmt`/AFM, `cherry`/CHE, `pardosa`/PGN);
  superseded ADRs live in `docs/adr/stale/`. Config: `adr-fmt.toml` (root).
- Before editing code in a crate, check its binding rules:
  `adr-fmt --context <crate>` (or the `adr-context` skill). `adr-fmt --lint`
  validates corpus integrity; `--tree`/`--refs` inspect structure/citations.
- Repo-local skills available: `adr-context`, `adr-lint`, `adr-refs`,
  `adr-tree`, `graphify`.
- `docs/STORY.md` is the apex on *why* (strategy); start there for intent.

## Tooling notes

- `graphify-out/graph.json` exists — use `graphify query/explain/affected` for
  structural questions before grepping; refresh with `graphify update .` after
  code changes (or rely on the post-commit hook).
- `.beads/` is an embedded-dolt store (gitignored) — bd mutations do **not**
  produce a git commit; the audit trail is dolt history + `interactions.jsonl`.
  Don't try to `git add` bead state.
- `cherry-pit-*` atomic-write protocol is CHE-0032 (temp → fsync → rename →
  parent-dir fsync); the production path in
  `cherry-pit-gateway/src/event_store/msgpack_file.rs::write_atomic` already
  implements it. `cherry-pit-gateway` genuinely uses MessagePack on disk;
  `gh-report` does not (native pardosa `.pgno`, default backend).
