# AGENTS.md — solon

Repo-specific operational notes. General agent/OODA doctrine, bd/beads
conventions, bash hygiene, and the Rust no-`//`-comments rule live in the
global `~/.config/opencode/AGENTS.md` (auto-loaded) — not repeated here.

## What this repo is

Rust workspace (edition 2024, MSRV 1.97, resolver 3, 20 crates) shipping three
binaries plus an ADR-governed library family and a large ADR corpus.

- Binaries (real entrypoints): `adr-fmt` (ADR validator, read-only),
  `adr-srv` (axum GraphQL service over an ADR-corpus projection),
  `gh-report` (GitHub org evidence collector + HTML reporter daemon).
  `comment-free` is a doc-lint tool.
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
- `rust-toolchain.toml` pins channel 1.97 (clippy+rustfmt). Use it; don't bump.

## Live-NATS tests need a pinned `nats-server` (common CI/local gotcha)

`crates/pardosa-nats/src/test_support.rs` spawns a real `nats-server` and
**asserts its `--version` matches `tools/.nats-server-version` (currently
2.14.3)** — it panics on mismatch or if the binary is absent. Affected tests
include `pardosa`'s `dragline::runtime::tests::*jetstream*`. To run them:
install `nats-server` v2.14.3 onto `PATH`. CI installs it in the `test` job
(checksum-verified). `async-nats` is pinned to the `server_2_14` feature to
match.

## CI specifics (`.github/workflows/ci.yml`)

- Triggers on push/PR to `main`. Third-party actions are **SHA-pinned**
  (Dependabot updates them); keep that pattern if you edit the workflow.
- Two custom **tripwire** jobs live in `.github/workflows/ci-reusable.yml`
  (called from `ci.yml`) and grep the tree to fail the build — do not break
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
- **Substrate ring purity:** `pardosa-nats` depends only on tokio, async-nats,
  bytes, blake3, and futures-util (plus tempfile behind an optional test
  feature) — no `cherry-pit-*` or `pardosa` adapter-ring edges;
  instrumentation/metrics belong in the `pardosa` adapter ring, never in
  `pardosa-nats`. High-cardinality ids (event_id, ack, stream) go on
  spans/logs, never metric labels (COM-0019:R6).
- **House style:** suppress lints with `#[expect(lint, reason = "…")]`
  (attribute, allowed) — not `//`-comments (forbidden fleet-wide). Use
  `#[allow(.., reason=..)]` only where `#[expect]` would be unfulfilled (e.g. a
  lint that fires under `--test` but not `--all-features`).

## Intent (why this repo exists — the Solon stance)

Solon lays down a *small, observable, ratified* set of enabling constraints —
after Solon the lawgiver — so that correct software is easier to build than
incorrect software. The bet is **subtractive**: remove enough degrees of
freedom that the remaining moves are obviously correct. When the type system
rejects illegal architectures, the search space an agent must explore collapses.

- **The libraries are the product; the binaries are evidence they work.**
  `cherry-pit-*` is the EDA+DDD+hexagonal substrate (illegal compositions —
  multiple writers per aggregate, async in the domain, leaky identity — do not
  type-check). `pardosa*` is the durable event-store substrate. `adr-fmt`/
  `adr-srv` are the governance plane. `gh-report` is the first non-trivial
  consumer, load-bearing proof the substrate carries real work.
- **Niche — where to play:** high-complexity, durable, *intra-org* workloads
  that do **not** need arbitrary horizontal scale-out. Single writer per
  aggregate; sync domain, async edges; linearizable per aggregate; sub-PB,
  single-region; crash-fail with recovery. Webscale is a different game and is
  not played here.
- **Non-goals** (recorded so they aren't rediscovered): multi-region/multi-tenant
  webscale; frameworks/scaffolding/starter-kits (the constraints *are* the
  guidance); runtime-pluggable architectures (composition is compile-time,
  CHE-0005:R1); schema evolution as a runtime feature (cross-major migration is
  a ratified operation); public-API surface freeze before v0.3 (invariants are
  frozen, surface is not yet).
- ADRs are binding for *what*; this section is orientation for *why*. Read the
  ADRs continuously, this once.

## ADR governance (this repo is ADR-driven)

- Corpus under `docs/adr/` (domains: `ground`/GND, `common`/COM, `rust`/RST,
  `security`/SEC, `flow`/FLO, `adr-fmt`/AFM, `cherry`/CHE, `pardosa`/PGN);
  superseded ADRs live in `docs/adr/stale/`. Config: `adr-fmt.toml` (root).
- Before editing code in a crate, check its binding rules:
  `adr-fmt --context <crate>` (or the `adr-context` skill). `adr-fmt --lint`
  validates corpus integrity; `--tree`/`--refs` inspect structure/citations.
- Repo-local skills available: `adr-context`, `adr-lint`, `adr-refs`,
  `adr-tree`, `graphify`.

## Tooling notes

- `graphify-out/graph.json` exists — use `graphify query/explain/affected` for
  structural questions before grepping; refresh with `graphify update .` after
  code changes (or rely on the post-commit hook).
- `.beads/` is an embedded-dolt store (gitignored) — bd mutations do **not**
  produce a git commit; the audit trail is dolt history + `interactions.jsonl`.
  Don't try to `git add` bead state.
- Two bd stores exist for this repo's work, selected by cwd:
  - Repo-local store: `.beads/` here (prefix `adr-fmt`) — the CANONICAL home
    for any mission that describes THIS repo's code.
  - HOME store: `~/.beads` (prefix `anders_jensen`) — for cross-repo / personal
    work with no single repo home.
- Convention (advisory — bd has no mechanism to enforce it; see below):
  - Run `bd` from the gh-report repo root (or any path inside it) for any
    repo-scoped mission, so beads auto-discover the repo-local `.beads/` and
    land with the `adr-fmt` prefix, co-located with the code they describe.
  - Use the HOME store only for genuinely cross-repo or personal-planning work.
  - A `mission:<slug>` label must resolve to an epic IN THE SAME STORE. Never
    use a bead/mission ID as a label value (`mission:anders_jensen-4gt` was such
    a malformed label — a mission-id masquerading as a slug; stripped
    2026-07-03).
- Why this is advisory only: `.beads/` is gitignored and bd mutations produce no
  git commit, so a git pre-commit hook cannot see (and therefore cannot block) a
  bead written to the wrong store. Recurrence-prevention here rests on running
  bd from the right cwd, not on tooling enforcement. Symptom of the failure this
  prevents: repo-scoped evidence beads accumulating in the HOME store with
  `mission:` labels that resolve to no epic in the repo-local store (the exact
  mess reconciled by mission anders_jensen-t0q, 2026-07-03).
- `cherry-pit-*` atomic-write protocol is CHE-0032 (temp → fsync → rename →
  parent-dir fsync); the production path in
  `cherry-pit-gateway/src/event_store/msgpack_file.rs::write_atomic` already
  implements it. `cherry-pit-gateway` genuinely uses MessagePack on disk;
  `gh-report` does not (native pardosa `.pgno`, default backend).
