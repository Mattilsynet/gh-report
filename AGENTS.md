# AGENTS.md — gh-report

Repo-specific operational notes. General agent/OODA doctrine, bd/beads
conventions, bash hygiene, and the Rust no-`//`-comments rule live in the
global `~/.config/opencode/AGENTS.md` (auto-loaded) — not repeated here.

Cross-repository operational authority is [trunk delivery](docs/trunk-delivery.md).
Use it for canonical library adoption, exact-head release, and deployed consumer
acceptance; this repository retains its source and gate authority.

## Section 1: Canonical Fleet Doctrine

### OODA Loop Roles
- **Copernicus** (Observe): Raw evidence gathering from environment, code, and external specs. Pure sensor; produces no hypotheses.
- **Feynman** (Orient): Produces ranked hypotheses with falsifiers; stress-tests against concrete examples.
- **Moltke** (Decide): Standing mission commander. Emits executable mission contracts, sets intent, boundaries, and abort criteria.
- **Hopper** (Act): Executes missions using Kent Beck TDD (red-green-refactor) with verify-before-claim discipline.
- **Linus** (Review): Mandatory pre-merge Rust reviewer for idiom conformance, type safety, unsafe soundness, and supply chain.
- **Hamilton** (Assurance): Architectural alignment and assurance reviewer running during CI wait windows.
- **Gardener** (GC): Post-mission cleanup specialist; reclaims transient scaffolding and closes completed mission beads.

### Priority Hierarchy
Tradeoffs strictly resolve in this five-tier priority order:
1. **Maintainability**: Pure trunk development, small deployable increments, minimal cognitive overhead, low complexity.
2. **Correctness by design**: Make illegal states unrepresentable via types, explicit state machines, and private invariant constructors.
3. **Response times**: Latency-sensitive read paths and prompt fact propagation across boundaries.
4. **Energy efficiency in code**: Minimize redundant polling, hot loops, unnecessary serialization, and idle CPU/memory burn.
5. **Features**: New functionality ranks last and must never compromise the higher tiers.

### Non-Interactive Shell Commands & Bash Hygiene
Subagents execute non-interactively. Commands that prompt for user confirmation stall execution indefinitely.
- Always use non-interactive and force flags: `cp -f`, `rm -f`, `rm -rf`.
- Streaming and batch mode: use `--batch`, `-y`, or `--quiet` where available.
- Stream separation: machine-readable findings route to `stdout`; diagnostics and logs route to `stderr`.

### Zero Plain Comments
In Rust source (`*.rs`), plain comments (`//` or `/* */`) are forbidden.
- Rationale belongs in commit messages, ADRs, or bead descriptions.
- Use `///` or `//!` contract doc-comments only when defining public API documentation (with required `# Errors`, `# Panics`, `# Safety` sections).
- Suppress lints with `#[expect(lint, reason = "...")]` rather than plain comments.

### Doctrine: "Make tools fast to iterate fast"
Developer and verification tooling must be compiled, ultra-fast Rust binaries operating directly on ASTs and files rather than slow interpreted wrappers or token-heavy in-context simulation. Fast tools enable high-frequency local feedback loops (INNER cadence) without friction.

### Doctrine: "Zero compliance theatre"
High-assurance testing techniques—such as property-based testing (proptest), fuzzing (cargo-fuzz), formal model checking, or fault injection—must be applied purposefully at critical serialization, concurrency, and storage boundaries (high-risk seams), not sprayed ubiquitously as box-ticking ceremony. Where type invariants and deterministic unit tests suffice, do not add compliance overhead.

## Section 2: Target-Specific Profile

### Target Classification & Entrypoint
- Target class: `service-unattended` (as mapped in `sf-sdlc.toml`).
- Real entrypoint: `gh-report` (GitHub org evidence collector + HTML reporter daemon).
- Canonical verification entrypoint: `scripts/verify.sh`
- Rust workspace (edition 2024, MSRV 1.98, resolver 3, 3 member crates) shipping one
  binary, web client and checker, consuming an external ADR-governed library family:
  - `adr-fmt` (ADR validator, read-only) from `Mattilsynet/adr-fmt`.
  - `comment-free` (doc-lint tool) from `acje/comment-free`.
  - `cherry-pit-*` — external event-sourcing substrate from `acje/cherry-pit`,
    all eight crates pinned to canonical main `ffffa0ddae206a322da9b9b4a94a3ad5e191da6e`.
  - `pardosa*` — `.pgno` event-store substrate + a NATS/JetStream backend
    (`pardosa-nats`). External git dependencies from `acje/pardosa`.

### Resource Contracts & Bounds (FLEET-RES-01)
Changes to ingestion, buffering, concurrency, retries, recursion, or hot paths
must define and satisfy explicit resource bounds:
- **Items and bytes accounted separately**: A bounded channel alone does not bound
  memory; admit work before unbounded allocation or payload retention.
- **Permit lifetimes & RAII**: Permits and resource charges must stay alive for the
  actual resource lifetime, including error paths and cancellations. Release exactly
  once via RAII.
- **Admission control**: Work admission must be bounded. If admission waits, bound
  the number of waiters and what they retain.
- **Progress, cancellation & shutdown**:
  - Individual work units, retries, and recursion depth must be bounded.
  - Service lifetime loops require reachable, supervised shutdown and bounded work
    between shutdown checks.
  - Dropping task handles does not cancel asynchronous tasks; use explicit cancellation
    tokens and await task completion during shutdown.
- **Atomic file write protocol (CHE-0032)**:
  All durable state written to disk must use the atomic sequence:
  `write temporary file` $\rightarrow$ `fsync file` $\rightarrow$ `atomic rename` $\rightarrow$ `fsync parent directory`.

### Build / test / verify (local cadence; boundary mirrors CI)
Three-tier verify cadence — INNER (per increment), MID (per sub-mission,
ONCE), BOUNDARY (per epic, ONCE). The done-claim is **tier-scoped**: a claim
is backed by the tier whose scope matches the claim's scope — a sub-mission
done-claim is backed by MID; the EPIC done-claim is backed by BOUNDARY.

- **INNER** (every hopper TDD increment AND every per-review-round
  re-verification, changed crate ONLY; exit-code criterion: test + clippy exit 0):
  ```sh
  CARGO_TERM_PROGRESS_WHEN=never cargo test -p <crate> --message-format=short
  CARGO_TERM_PROGRESS_WHEN=never cargo clippy -p <crate> --all-targets --message-format=short -- -D warnings
  ```
  One test: `cargo test -p <crate> <name> --message-format=short`.
  `--workspace` / `--all-features` are FORBIDDEN at this tier.
  `--all-targets` is MANDATORY on the clippy line at this tier and at MID.

- **MID** (ONCE at sub-mission completion, before a sub-mission done-claim;
  changed crates PLUS their reverse-dependent closure; exit-code criterion:
  every listed `-p` package's test + clippy exit 0, with `--all-targets` on
  the clippy line as at INNER). `--workspace` /
  `--all-features` are FORBIDDEN at this tier — MID stays scoped to the
  computed package list, never the whole graph.

  Compute reverse-dependent closure mechanically via:
  ```sh
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
  exit-code criterion: all four commands below exit 0):
  ```sh
  cargo build --workspace --all-features --locked
  timeout 900 cargo test --workspace --all-features --locked --no-fail-fast
  cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
  cargo fmt --all -- --check
  sh scripts/verify.sh
  ```
  `timeout 900` is mandatory on the BOUNDARY test line.
  **Exit 124 is `Outcome::Surprise`, NEVER a test failure.** Investigate the stall;
  do not fold it into a failure count.
  `--no-fail-fast` is mandatory on the BOUNDARY test line.
  Doctests ride the BOUNDARY `cargo test --workspace` line above.

- `clippy::pedantic` is the **standing bar**, not an elevation
  (`[workspace.lints.clippy] pedantic = warn` + CI `-D warnings`). New code must
  pass pedantic with zero warnings.
- `rustfmt` runs on **stable defaults only** (RST-0003:R3); there is no custom
  `rustfmt.toml` style. Don't add format config.
- `rust-toolchain.toml` pins channel 1.98 (clippy+rustfmt). Use it; don't bump.

### Supply Chain Gates
`cargo deny check` and `cargo audit` are the supply-chain controls.

### Rustdoc Budget Gate
Run the same native check from the repository root locally and in CI:
```sh
comment-free --check-doc-budget --doc-advisory-words 80 --doc-max-words 120 --max-warning-files 0 .
```

Requires comment-free 0.2.0 at the canonical revision below:
```sh
cargo +1.98.0 install --git https://github.com/acje/comment-free --rev e45de7ef3b0fcd9a1ec299b9026b14fb5b0cf534 --locked comment-free
```

The read-only native gate recursively scans Rust sources under `.` with the
tool's build/hidden pruning: 80 prose words is advisory; 120 is enforced.
Fenced code is excluded by the tool. Summary-only output retains full totals
while suppressing finding details; diagnostics remain visible.
Native gate exits are 0 for pass, 1 for enforced breach, and 2 for
unknown/error, including undecided payloads or empty scope. Policy and its
implementation/tests/proofs belong upstream; repository checks establish
integration only. No rewrite mode runs.
Macro-generated docs without spelled `doc` tokens remain outside detection;
this is not proof of semantic documentation coverage or process-memory bounds.

### Live-NATS Tests Need Pinned `nats-server`
The consumer native-store harness checks `tools/.nats-server-version`, currently
2.14.5, and prefers `tools/bin/nats-server` (verified v2.14.5 on 2026-09-21).
CI installs the consumer pin as a step in the `build-test-lint` job checksum-verified.
`async-nats` is pinned to the `server_2_14` feature to match.

### CI Specifics (`.github/workflows/ci.yml`)
- Triggers on push/PR to `main`. Third-party actions are **SHA-pinned**.
- Merge gates (RST-0007) live in `.github/workflows/ci-reusable.yml` (called
  from `ci.yml`), invoked via `tools/tripwires.sh <check>`.
- `cargo deny check` and `cargo audit` run as supply-chain gates.

### Architecture Invariants
- **Synchronous public facade.** No `async fn` on the public surface of
  `pardosa::store` / `prelude` (PGN-0010:R5, PGN-0008, PGN-0015:R6). The
  intentional sync-over-async bridge is `pardosa-nats/src/handle.rs::run_op`
  (`block_on`); `std::sync::Mutex` behind the facade is deliberate.
- **Closed error enums are mandated (C4.5/C4.6):**
  Public error enums MUST NOT carry `#[non_exhaustive]`. Variant sets are
  complete within a major line, making unhandled error states
  unrepresentable at compile time. Enforced by `non-exhaustive-check`.
- **Substrate ring purity:** `pardosa-nats` depends only on tokio, async-nats,
  bytes, blake3, and futures-util.
- **House style:** suppress lints with `#[expect(lint, reason = "…")]`.

### Rustling Review Examples & Construction-Path Inventory
Selective TigerStyle adopt/adapt/reject decisions and the construction-path
inventory are canonical in fleet `~/.config/opencode/AGENTS.md` § Rustling —
selective TigerStyle adaptation. Invariant-bearing domain types must enforce
"illegal states unrepresentable" by design.

Scoped review samples:
| Case | Invariant, route and read-level assessment |
|---|---|
| `ControlCell` (`crates/gh-report/src/report/view_model.rs`; builder in `report/html.rs`) | Exclusion count and formatted text must agree. Reject mutation/struct-literal routes under `illegal-state-representable`; private fields plus deriving constructor/accessors. |
| `UpdatedAt` (`crates/gh-report/src/domain/repository.rs`) | Nonempty spelling, NOT timestamp syntax. Accept `new("") == None`. Private storage and read-only dereference constrain outside callers. |
| `SweepTimeout` (`crates/gh-report/src/config/mod.rs`) | Nonzero seconds fitting `u32`. Accept `new(0) == None`, `new(1)` and the current 7200-second default. |
| Independent repository flags (`crates/gh-report/src/domain/repository.rs`) | `archived`, `has_issues`, `fork`, `is_empty` are independent attributes, not exclusive states. Accept booleans. |

### Intent
This workspace lays down a small, observable, ratified set of enabling
constraints — an ADR corpus enforced by `adr-fmt` — so that correct
software is easier to build than incorrect software.

### ADR Governance
- Corpus under `docs/adr/` (domains: `ground`/GND, `common`/COM, `rust`/RST,
  `security`/SEC, `flow`/FLO, `adr-fmt`/AFM, `cherry`/CHE, `pardosa`/PGN);
  superseded ADRs live in `docs/adr/stale/`. Config: `adr-fmt.toml` (root).
- Before editing code in a crate, check its binding rules:
  `adr-fmt --context <crate>`.

### Tooling Notes & Database Discovery
- `graphify-out/graph.json` exists — use `graphify query/explain/affected` for structural questions.
- `.beads/` contains tracked repository scaffold, while the database itself (`embeddeddolt/`) is ignored.
- Pinned store discovery: Always run bd commands with `bd -C <repo-root>` (or set `BEADS_DIR`).
- Strict fleet invariant: No HOME store at `~/.beads`.
- `cherry-pit-*` atomic-write protocol is CHE-0032 (temp → fsync → rename → parent-dir fsync).
