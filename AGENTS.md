# AGENTS.md — gh-report

Repo-specific operational notes. General agent/OODA doctrine, bd/beads
conventions, bash hygiene, and the Rust no-`//`-comments rule live in the
global `~/.config/opencode/AGENTS.md` (auto-loaded) — not repeated here.

Cross-repository operational authority is [trunk delivery](docs/trunk-delivery.md).
Use it for canonical library adoption, exact-head release, and deployed consumer
acceptance; this repository retains its source and gate authority.

## What this repo is

Rust workspace (edition 2024, MSRV 1.98, resolver 3, 3 member crates) shipping one
binary, web client and checker, consuming an external ADR-governed library family.

- Binary (real entrypoint): `gh-report` (GitHub org evidence collector + HTML
  reporter daemon). Two tools are **not** built here and are consumed as
  installed binaries from their canonical repos: `adr-fmt` (ADR validator,
  read-only) from `Mattilsynet/adr-fmt`, which has no library consumers here
  and therefore no workspace-dependency pin; and `comment-free`
  (doc-lint tool) from `acje/comment-free`, which has no library consumers
  here and therefore no workspace-dependency pin.
- `cherry-pit-*` — external event-sourcing substrate from `acje/cherry-pit`,
  all eight crates pinned to canonical main `ffffa0ddae206a322da9b9b4a94a3ad5e191da6e`.
  The outer `pardosa-cherry-pit-test-support` uses the same revision as a
  consumer dev-dependency. Library tests/fixtures are owned and run upstream;
  local workspace tests do not run git dependencies' test targets.
- `pardosa*` — `.pgno` event-store substrate + a NATS/JetStream backend
  (`pardosa-nats`). **External, not workspace members**: consumed as git
  dependencies from `acje/pardosa` at the rev pinned in
  `[workspace.dependencies]`, currently canonical main
  `49ea5ec42a782aa9303b0e6eab2659d59d181e69`. No `pardosa`-named workspace member
  remains. Neutral Cherry normal/build closures exclude Pardosa per CHE-0084
  and canonical `acje/cherry-pit` CPP-0001; outer Pardosa-family adapters are
  cohosted there, and dev/test bridges are permitted. gh-report does not link
  the outer projection adapter. Because `pardosa` / `pardosa-nats` are dependencies rather than
  members, their **test targets are not part of this workspace's test set** —
  `cargo test -p pardosa` and `cargo test -p pardosa-nats` have no test target
  to run here.

## Build / test / verify (local cadence; boundary mirrors CI)

Three-tier verify cadence — INNER (per increment), MID (per sub-mission,
ONCE), BOUNDARY (per epic, ONCE). This supersedes the prior two-tier text:
the earlier wording bound BOUNDARY to "mission/sub-mission completion" with
"exit codes from this tier back the done-claim" (ratified, adr-fmt-8whg7 /
adr-fmt-xdlw9 O1); that bound the done-claim to the wrong granularity and is
overwritten here, cited explicitly per adr-fmt-8whg7 and adr-fmt-xdlw9. The
done-claim is now **tier-scoped**: a claim is backed by the tier whose scope
matches the claim's scope — a sub-mission done-claim is backed by MID (its
changed crates + reverse dependents); the EPIC done-claim is backed by
BOUNDARY. Verify-before-claim is not weakened at any level; what changes is
that a sub-mission no longer claims workspace-wide correctness it never
established.

Measured driver: 1647 full-workspace invocations, 84.2% at hopper/linus
inner-loop tier, ~23.6h wall-clock, 94.6% same-session repeats, against
0/98 exhaustively-triaged GenuineEscape yield (adr-fmt-bgp65 G2) — the prior
prose-only cadence clause did not bind because mission contracts exposed one
undifferentiated `verify_commands` vector and hopper executes every listed
command (adr-fmt-j5ujb H1). Each tier below declares its own observable
exit-code criterion (GND-0005, no ADR governs verification tiering itself —
this is unconstrained ground, adr-fmt-xdlw9 O4 G1-G3/G7; COM-0024 governs
test KINDS mapped to architectural layers, not runner or cadence choice,
adr-fmt-xdlw9 O3).

- **INNER** (every hopper TDD increment AND every per-review-round
  re-verification, changed crate ONLY; exit-code criterion: the changed
  crate's test + clippy exit 0):
  ```
  CARGO_TERM_PROGRESS_WHEN=never cargo test -p <crate> --message-format=short
  CARGO_TERM_PROGRESS_WHEN=never cargo clippy -p <crate> --all-targets --message-format=short -- -D warnings
  ```
  One test: `cargo test -p <crate> <name> --message-format=short`.
  `--workspace` / `--all-features` are FORBIDDEN at this tier.

  `--all-targets` is MANDATORY on the clippy line at this tier and at MID.
  Without it, any lint that fires only in a test/bench/example target is
  invisible to INNER, to MID, and to linus's per-round re-verification, and
  surfaces only at BOUNDARY (once per epic) or in CI. Live instance
  (ghr-gpu84): a constant `assert!` in a `#[cfg(test)]` module passed two
  hopper INNER rounds and one linus round clean, then failed
  `clippy::assertions_on_constants` at exit 101 when linus round 2 ran
  `--all-targets` — two review rounds spent on a one-line fix, exhausting
  the 2-round cap. Unlike the CI-ONLY blind spot below, this one was
  self-inflicted by the tier command and is locally cheap to close.

  Cost, measured 2026-09-06 on `gh-report` (the workspace's largest crate;
  macOS/arm64, 14 cores, warm cargo cache; `touch` on one `src` file then
  re-run, two paired rounds, identical both rounds): plain **1.74s**,
  `--all-targets` **2.35s** — **+0.61s (+35%)** per increment. Re-derive if
  the crate or machine profile changes (iteration-speed rule 2). This
  measurement is the input the deferral in ghr-gpu84 was waiting on; the
  tiering's 1647-invocation over-verification driver concerns `--workspace`
  scope, which is unchanged here — `-p` scoping stays, only the target set
  widens.
- **MID** (ONCE at sub-mission completion, before a sub-mission done-claim;
  changed crates PLUS their reverse-dependent closure; exit-code criterion:
  every listed `-p` package's test + clippy exit 0, with `--all-targets` on
  the clippy line as at INNER). `--workspace` /
  `--all-features` are FORBIDDEN at this tier — MID stays scoped to the
  computed package list, never the whole graph.

  The historical path-only recipe below applies to local member changes only.
  For external Cherry adoption, use full locked metadata's `resolve.nodes`
  reverse edges and intersect the result with `workspace_members`; the current
  affected member is gh-report. Producer test targets stay upstream. See
  README.md "Canonical Cherry gate ownership" for source-gate ownership.

  Reverse-dependent closure is mechanically computable, not a judgement
  call — verified against `cherry-pit-core` (historical run, 8 transitive
  reverse dependents: `cherry-pit-app`, `cherry-pit-gateway`,
  `cherry-pit-merger`, `cherry-pit-projection`, `cherry-pit-web`,
  `cherry-pit-wq`, `gh-report`, `pardosa-cherry-pit-test-support`; exit 0):
  ```
  cargo metadata --format-version 1 --no-deps | jq -r --arg t <changed-crate> '
    (reduce .packages[] as $p ({}; .[$p.name] = [$p.dependencies[]? | select(.path != null) | .name])) as $g
    | [$t]
    | until(
        . as $seen
        | ($g | to_entries | map(select(.value | any(. as $d | $seen | index($d)))) | map(.key)) as $new
        | ($seen + $new | unique) == $seen;
        . as $seen
        | ($g | to_entries | map(select(.value | any(. as $d | $seen | index($d)))) | map(.key)) as $new
        | ($seen + $new | unique)
      )
    | .[]
  '
  ```
  Feed the resulting package names as `-p <name>` to `cargo test` / `cargo
  clippy`, one flag per name including `<changed-crate>` itself.
- **BOUNDARY** (ONCE per EPIC, before the epic done-claim; full workspace;
  measured total ~4.4 min, bead ghr-df35935d (authoritative) / ghr-64e9cdf0
  (orientation) — macOS/arm64, 14 cores, warm cargo cache, uncontended
  machine; not a universal constant, present with these conditions. The
  prior "60-83 min" figure (bead ghr-2f56d34d) is superseded — that run hit
  environment contention (NI=5, competing processes, Defender activity), not
  a code baseline; exit-code criterion: all four commands below exit 0):
  ```
  cargo build  --workspace --all-features --locked                  # 93.7s cold, 678 units
  timeout 900 cargo test --workspace --all-features --locked --no-fail-fast   # ~120s warm (~40s compile + 85.6s exec, incl. doctests)
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings  # 49.0s
  cargo fmt --all -- --check                                        # 1.0s
  ```
  `timeout 900` is mandatory on the BOUNDARY test line, and 900s is derived
  from the ~120s warm measurement above (~7.5x that line, ~3.4x the whole
  ~4.4min four-command tier; authoritative figure in bead ghr-df35935d).
  It is deliberately NOT anchored on the superseded 60-83min figure
  (ghr-2f56d34d), which measured CPU contention rather than a code baseline.
  The wrapper exists because the PR #44 deadlock was not CI-specific — a local
  BOUNDARY would have hung identically, and only per-invocation agent
  discipline stood between it and an indefinite stall, which is not a property
  of the tier. Re-derive the bound if the workspace or machine profile changes
  (iteration-speed rule 2).

  **Exit 124 is `Outcome::Surprise`, NEVER a test failure.** It means the
  suite was killed at the wall clock without reaching a verdict, so it carries
  no information about whether any test passed — reporting it as red is as
  wrong as reporting it as green. Investigate the hang; do not re-run until
  green, and do not fold it into a failure count. Without this rule the tier
  trades a silent hang for a silent miscategorisation.

  `--no-fail-fast` is mandatory on the BOUNDARY test line: plain `cargo test`
  stops at the first failing test binary, so a failing BOUNDARY silently
  verifies only a fraction of the workspace (measured: ~40% covered before
  abort) — violating the coverage-parity intent of this tier. Note the
  direction of the mirror: CI's test step
  (`.github/workflows/ci-reusable.yml:138`) carries **neither**
  `--no-fail-fast` nor a `timeout` wrapper — it is bounded by the job's
  `timeout-minutes: 30` instead. BOUNDARY mirrors CI's *scope*, not its exact
  flags; the two local flags are deliberate local additions, not a claim about
  CI. Changing CI's cadence is out of scope for documentation work. BOUNDARY stays
  on `cargo test` rather than `nextest`: nextest measured slower at workspace
  scope (262s vs ~120s) and does not run doctests. Doctests therefore ride the
  BOUNDARY `cargo test --workspace` line above; there is no separate
  `cargo test --doc` command in this tier.

  Coverage parity (non-negotiable, and free here): CI's C1 doctest
  condition (adr-fmt-cus9e) is a CI/pre-merge gate ONLY — adr-fmt-8whg7 Q2
  already adjudicated that C1 does not obligate `cargo test --doc` in the
  local inner loop. **CI has no dedicated doctest step**: doctests are covered
  by the single `cargo test --workspace --all-features --locked` step at
  `.github/workflows/ci-reusable.yml:138`, exactly as locally. CI is untouched
  by this tiering change: the 7 required
  contexts and stabsec code-owner review still run full verification
  pre-merge. Nothing is deleted, `#[ignore]`d, or feature-gated by moving
  BOUNDARY to epic granularity; coverage is relocated to where it earns its
  cost, never dropped.
- **Cutover traffic guard** (locally runnable entry point, per local-gate
  doctrine): `python3.12 -B tools/test_cutover_guard.py` — mocked regression for
  `tools/cutover_guard.py` and the wiring of `.github/workflows/cutover.yml`.
  It performs no cloud reads or writes. The same suite runs as the workflow's
  first validation step after checkout (`Guard Self-Test`), before Google Cloud
  authentication and before any traffic mutation, so a red guard cannot reach a
  real rollout. The guard
  rejects a `percent` input of `0` outright instead of silently skipping, and
  treats tagged zero-traffic revisions (entries with no `percent`) as serving
  nothing.
- **CI-ONLY** (never in the local loop): CI owns deny, audit, and the
  tripwire jobs (`tools/tripwires.sh --list` for the current check names). No
  agent tier runs these — not INNER, not MID, not BOUNDARY.
- **Not covered by the four BOUNDARY commands** (additional CI steps, *not* an
  added prohibition): the `gh-report-web-client` steps inside the
  `build-test-lint` job (`.github/workflows/ci-reusable.yml:16-46`). They split
  two ways. Host-side: `python3.12 -B tools/verify_web_client.py status --ci`,
  `... compiler --ci`, and the guard-regression harness `python3.12 -B
  tools/test_web_client_verify.py` — these need no wasm target and no browser.
  wasm/browser-side: the `--target wasm32-unknown-unknown ... --no-run`
  precompile and `python3.12 -B tools/verify_web_client.py browser --ci`, which
  needs headless Chrome. A green BOUNDARY says nothing about either group; that
  is a coverage fact, not a ban. Local invocation is explicitly fine — the
  regression harness is already documented as a local command at
  `crates/gh-report-web-client/src/sort.rs:148-149`.
  A green BOUNDARY is therefore NOT a proxy for a green CI; the residual
  defect class that reaches a PR unnoticed by every agent tier is exactly
  "violates a CI-only invariant" — a supply-chain advisory, or a tripwire
  whose grep no longer matches after a rename. Live instance: PR #12's K9
  rename of `app/state.rs` -> `app/state/mod.rs` broke the
  `gh-report-projection-lock-tripwire` whitelist; linus APPROVEd 0-issues and
  BOUNDARY was green, but CI caught the break. Adding a tripwire grep to
  MID/BOUNDARY is under consideration — it is NOT currently an obligation.
- `clippy::pedantic` is the **standing bar**, not an elevation
  (`[workspace.lints.clippy] pedantic = warn` + CI `-D warnings`). New code must
  pass pedantic with zero warnings.
- `rustfmt` runs on **stable defaults only** (RST-0003:R3); there is no custom
  `rustfmt.toml` style. Don't add format config.
- `rust-toolchain.toml` pins channel 1.98 (clippy+rustfmt). Use it; don't bump.

## Live-NATS tests need a pinned `nats-server` (common CI/local gotcha)

The consumer native-store harness checks `tools/.nats-server-version`, currently
2.14.5, and prefers `tools/bin/nats-server` (verified v2.14.5 on 2026-09-21).
The separately installed `/opt/homebrew/bin/nats-server` reports v2.14.7;
that does not change this repository's test pin. Producer harness versions are
owned upstream and must be read from their actual pinned source. Consumer
store/ACL tests do run here; git dependencies' own test targets do not.
CI installs the consumer pin as a step in the
`build-test-lint` job (`.github/workflows/ci-reusable.yml:126-137`,
checksum-verified). `async-nats` is pinned to the `server_2_14` feature to
match.

The consumer `store::tests` harness distinguishes unavailable/version-mismatched
servers (SKIP) from fatal harness failures. Quiet green tests alone do not prove
live assertions executed; use the harness's visible skip output when establishing
that evidence. The currently pinned producer instead exposes
`pardosa_nats::test_support::LiveNatsServer`, which spawns `nats-server` from PATH
and panics on startup failure; its source does not impose the old consumer
version/skip policy. Do not carry historical `LiveNats` API claims across pins.

Related, still open: `ghr-89b05be0`. Do not claim `Outcome::Verified` from a
run that never reached exit 0 — say so explicitly (partial) and cite the
matching bead.

A second, unrelated known-red: `cargo test -p pardosa --test trybuild` is a
false red on **incremental** builds only (reproduced on `main` @ `d7270f5`);
it passes on a clean rebuild. Not a regression. Historical note only — since
`pardosa` is an external git dependency rather than a workspace member, that
test target is not part of this workspace's test set.

## CI specifics (`.github/workflows/ci.yml`)

- Triggers on push/PR to `main`. Third-party actions are **SHA-pinned**
  (Dependabot updates them); keep that pattern if you edit the workflow.
- Merge gates (RST-0007) live in `.github/workflows/ci-reusable.yml` (called
  from `ci.yml`), invoked via `tools/tripwires.sh <check>` — do not break the
  invariants they guard. Run `tools/tripwires.sh --list` for the current check
  names and dispatch on any single check locally (`tools/tripwires.sh
  <check>`), or `tools/tripwires.sh all` for every check. The committed
  regression harness `tools/tripwire-regression.sh` is the locally-runnable
  entry point pinning those gates' behaviour; run it before handing off any
  change to `tools/tripwires.sh`. It is not itself a merge gate — its
  ratification as one is deferred (see ghr-z9cho.6). Governing citations
  (kept current in the script's `::error::` strings, not duplicated here):
  projection-lock is COM-0018 + CHE-0048:R7; async-trait deny is
  CHE-0025:R1+R2; non-exhaustive gate is RST-0006:R1+R3.
  The projection-lock whitelist is exactly
  `crates/gh-report/src/app/state/mod.rs`, NOT the whole `app/state/`
  directory: the sibling modules (`builder.rs`, `baseline.rs`,
  `server_state.rs`) are in scope of the ban.
- `cargo-vet` was removed (deferred per SEC-0009); `cargo-deny`/`cargo-audit`
  are the supply-chain controls.

## Architecture invariants an agent will trip over

These are load-bearing; violating them is an abort-class change:

- **Synchronous public facade.** No `async fn` on the public surface of
  `pardosa::store` / `prelude` (PGN-0010:R5, PGN-0008, PGN-0015:R6). The
  intentional sync-over-async bridge is `pardosa-nats/src/handle.rs::run_op`
  (`block_on`); `std::sync::Mutex` behind the facade is deliberate — do **not**
  "fix" it to `tokio::sync::Mutex` (would break single-writer linearizability).
- **Closed error enums are mandated (C4.5/C4.6, reversing RST-0006/PGN-0006/CHE-0021):**
  Public error enums MUST NOT carry `#[non_exhaustive]`. Variant sets are
  complete within a major line; adding or changing variants is a breaking
  change requiring a major version bump, making unhandled error states
  unrepresentable at compile time. Enforced by `non-exhaustive-check`.
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

## Rustling review examples

Selective TigerStyle adopt/adapt/reject decisions and the construction-path
inventory are canonical in fleet `~/.config/opencode/AGENTS.md` § Rustling —
selective TigerStyle adaptation. Use its existing `illegal-state-representable`
artefact in both Linus and generic review; do not add a repo regex or duplicate
the resource/control-flow doctrine. Joint acceptance: `ghr-jwl9c`, `ghr-2gefa`.

These are scoped review samples, not an exhaustive type audit or an API change:

| Case | Invariant, route and read-level assessment |
|---|---|
| `ControlCell` (`crates/gh-report/src/report/view_model.rs`; builder in `report/html.rs`) | Exclusion count and formatted text must agree. The builder derives a consistent pair, but public fields permit setting `excluded_total = 1` while retaining `excluded_formatted = "0 unmeasured"`. Reject this mutation/struct-literal route under `illegal-state-representable`; private fields plus a deriving constructor/accessors are the separate `ghr-p84jq` remedy. A downstream compile-fail mutation test would check that remedy; none is claimed here. |
| `UpdatedAt` (`crates/gh-report/src/domain/repository.rs`) | Nonempty spelling, NOT timestamp syntax. Accept `new("") == None`, including wire normalization through `deserialize_updated_at`; accept nonempty `"not-a-timestamp"`. Private storage and read-only dereference constrain outside callers. Audit conversions and defining-module construction too; an unchecked deserializer admitting empty would be a reject example, not a claim that one currently exists. Existing constructor/wire/native-roundtrip tests are runtime boundary oracles. |
| `SweepTimeout` (`crates/gh-report/src/config/mod.rs`) | Nonzero seconds fitting `u32`. Accept `new(0) == None`, `new(1)` and the current 7200-second default. `Default` constructs directly, so review its constant separately; private storage is not a global proof of nonzero. Existing zero/default/duration tests are boundary oracles, not evidence that arbitrary defining-module code cannot create zero. |
| Independent repository flags (`crates/gh-report/src/domain/repository.rs`) | `archived`, `has_issues`, `fork`, `is_empty` are independent attributes, not exclusive states. Accept their booleans; no enum conversion is justified merely by their count. |

Refresh these implementations when reviewing a diff. Record actual test exits
if executed; these examples alone are neither compiler nor CI proof.

## Intent (why this repo exists — the gh-report stance)

This workspace lays down a *small, observable, ratified* set of enabling
constraints — an ADR corpus enforced by `adr-fmt` — so that correct
software is easier to build than incorrect software. The bet is
**subtractive**: remove enough degrees of freedom that the remaining moves
are obviously correct. When the type system rejects illegal architectures,
the search space an agent must explore collapses. `gh-report` is the first
non-trivial consumer of that substrate, not the source of the constraints.

- **The libraries are the product; the binaries are evidence they work.**
  `cherry-pit-*` is the EDA+DDD+hexagonal substrate (illegal compositions —
  multiple writers per aggregate, async in the domain, leaky identity — do not
  type-check). `pardosa*` is the durable event-store substrate. `adr-fmt` is
  the governance plane. `gh-report` is the first non-trivial
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
  code changes (or rely on the post-commit hook). Supported commands in 0.9.9:
  `query`, `path`, `explain`, `affected`, `diagnose multigraph`, `update .`, and
  `hook status`. Do NOT run `graphify hook install` or `graphify hook uninstall`
  as this clobbers custom hooks (`post-commit` background rebuild / worktree guard
  / filter tail, and `post-checkout` F7 disable).
- `.beads/` contains tracked repository scaffold (`.gitignore`, `README.md`,
  `config.yaml`, `metadata.json`, and `.beads/hooks/*`), while the database
  itself (`embeddeddolt/`) and runtime files (`interactions.jsonl`, etc.) are
  ignored by `.beads/.gitignore` and `.gitignore`. bd mutations do **not**
  produce a git commit; the audit trail is dolt history + `interactions.jsonl`.
  Don't try to `git add` bead database state.
- Single canonical bd store with pinned discovery:
  - Repo-local store: `.beads/` here (prefix `ghr`, embedded Dolt database
    `gh_report`) — the only canonical store for work on THIS repo.
  - Pinned discovery: Always run bd commands with `bd -C <repo-root>` (or set
    `BEADS_DIR`) to target this repository's store directly. Do not rely on
    ambient cwd discovery or upward directory walks.
  - Store verification: An exit code of 0 alone does not prove the expected
    store answered; defensively verify store resolution via
    `bd -C <repo-root> where` (checking prefix `ghr` and database path) or
    `bd -C <repo-root> --readonly context --json`.
  - No HOME store: Fleet doctrine strictly prohibits a home-level bd store at
    `~/.beads`. Distinguish a HOME issue store (embeddeddolt database) from bd's
    telemetry-only directory (`~/.beads` containing only `eventsData/`); do not
    delete either. All repository beads must reside in the repository-local store.
  - Diagnostic support: Full `bd doctor` validation requires Dolt server mode;
    under embedded Dolt mode running `bd doctor` without flags returns
    `code: embedded_unsupported`. Supported embedded checks are
    `--check=artifacts`, `--check=conventions`, `--check=pollution`, and
    `--check-health`.
- `cherry-pit-*` atomic-write protocol is CHE-0032 (temp → fsync → rename →
  parent-dir fsync). The canonical gateway has no MessagePack backend;
  `gh-report` persists domain events via native pardosa (`.pgno`, default
  backend, CHE-0074), and (per CHE-0099, msgpack-removal-2 Direction A)
  its scheduler + sweep-timeout streams are ephemeral in-process stores
  with no on-disk persistence — `gh-report` is genuinely msgpack-free for
  every prod event store as of msgpack-removal-2, not just for the
  CHE-0074-governed domain-event trio. The prior incidental `rmp-serde`
  uses (in-process timer codec, byte-size metric sample, and three
  serde-compat tests) were removed at the CHE-0074 purge.
