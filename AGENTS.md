# AGENTS.md — adr-fmt

Multi-crate Rust workspace. The foundational product is `adr-fmt`, an ADR
template + link-integrity validator — read-only at runtime, never writes
into the corpus. The workspace also contains the cherry-pit family
(`cherry-pit-{core,gateway,web,projection,agent,wq,storage-primitives}`),
`gh-report`, and the pardosa family
(`pardosa`, `pardosa-derive`, `pardosa-encoding`, `pardosa-genome`,
`pardosa-traits`). See `Cargo.toml` `members` for the SSOT. adr-fmt
governance is scoped to the `adr-fmt` crate and its `AFM-*` ADRs; other
crates have their own ADR domains (CHE, PAR, GEN, etc.).

## Commands

Always scope to the package; the workspace is multi-crate and `-p adr-fmt`
is the load-bearing form used throughout the docs.

```
cargo build -p adr-fmt
cargo test  -p adr-fmt          # ~84 integration tests, all in crates/adr-fmt/tests/integration.rs
cargo test  -p adr-fmt <name>   # filter; tests are flat, names are the index
cargo test --workspace --all-features   # mirrors CI; run before push
cargo run   -p adr-fmt -- --lint
cargo clippy --workspace --all-targets
cargo fmt --check
```

Toolchain is pinned: `rust-toolchain.toml` selects 1.95 + clippy + rustfmt.
Edition 2024, MSRV 1.95, resolver 3. Workspace lints set
`clippy::pedantic = warn` (see `Cargo.toml [workspace.lints.clippy]`).
Pre-existing pedantic warnings live in `crates/adr-fmt/tests/integration.rs`;
treat them as known noise unless the change is in that file. CI runs
`cargo test --workspace --all-features`, `cargo clippy --workspace
--all-targets -- -D warnings`, and `cargo fmt --check`
(`.github/workflows/ci.yml`). Mirror these locally before push.

**Verify-command gotchas.** (a) `cargo test -p <crate> <name>` is a
function-name *filter*, not a file target — `<name>` matching no
`#[test] fn` exits 0 with 0 tests run (silent false-green). Use
`cargo test -p <crate> --test <file_stem>` to target an integration-test
file. (b) Workspace-wide `cargo fmt --check` may pick up pre-existing
baseline drift in `crates/gh-report/src/infra/server/{mod,server}.rs`;
prefer `cargo fmt -p <crate>` while that drift is unreconciled.

## CLI surface (frozen for v0.1)

```
adr-fmt                     # default: print governance / setup guide
adr-fmt --lint              # lint corpus
adr-fmt --refs <ADR_ID>     # inbound References + Supersedes
adr-fmt --context <CRATE>   # decision rules for a crate
adr-fmt --tree [DOMAIN]     # domain tree
```

Exit codes: `0` = analysis complete (warnings allowed, AFM-0003), `1` =
infra error or structural lint error. Warnings are advisory by design —
do not "fix" by promoting them to errors.

## Configuration discovery (AFM-0001)

`adr-fmt.toml` at the workspace root is the SSOT discovery marker. The
binary walks up from CWD until it finds one with a valid `[corpus]` table.
There is **no CLI override** for the corpus path — this is intentional.
Tests that need an isolated corpus use `tempfile` + a synthetic
`adr-fmt.toml`; copy that pattern from `tests/integration.rs` rather than
inventing flags.

Rule defaults are hardcoded in the binary; `[[rules]]` entries in
`adr-fmt.toml` only override parameters (per AFM-0004). Adding a rule
means new code, not new config.

## Layout

```
crates/adr-fmt/src/
  main.rs        CLI dispatch
  config.rs      adr-fmt.toml schema + discovery walk
  parser.rs      regex-based markdown parsing (AFM-0006); large but stable
  model.rs       AdrRecord, DomainDir, ID parsing
  rules/
    mod.rs       module aggregator
    template.rs  T0xx — per-file template rules
    links.rs     L0xx — cross-file link / lifecycle rules (S0xx live here too)
    naming.rs    filename ↔ ADR ID conformance
  guidelines.rs  default-mode governance + setup output
  context.rs     --context resolution (largest module after rules)
  output.rs      formatted reporting
  containment.rs path safety (AFM-0016)
  refs.rs        --refs implementation
  nav.rs         --tree implementation
  report.rs      Diagnostic struct
crates/adr-fmt/tests/integration.rs   single flat file; one test per scenario
docs/adr/        ADR corpus this tool lints; subdirs are domains
docs/adr/stale/  retired ADRs (AFM lifecycle; see `[stale]` in adr-fmt.toml)
adr-fmt.toml     workspace-root config (also the discovery marker)
```

`parser.rs` and `output.rs` are deliberately large and not to be refactored
during v0.1. Touch only when fixing a behavioural defect.

## Diagnostic ID conventions

- `T0xx` — template rules (`rules/template.rs`)
- `L0xx` — link rules (`rules/links.rs`)
- `S0xx` — lifecycle / supersedes rules (also in `rules/links.rs`)
- `P0xx` — parser-stage diagnostics (AFM-0017)

Every rule should have ≥1 integration test.

## Governance

ADRs about this tool live under `docs/adr/adr-fmt/` with prefix `AFM`.
Before changing architectural surface (CLI shape, discovery, rule schema,
diagnostic namespace, config schema), read the relevant `AFM-*` ADR. The
oracle agent enumerates these via `adr-fmt --tree AFM` (use the freshly
built binary; the tool dogfoods itself).

Other domains (`common`, `ground`, `rust`, `security`, `flow`) are a
**retained reference corpus** for self-host validation. Do not edit those
ADRs; do not add or remove domains in `adr-fmt.toml` during v0.1. The
`pardosa` and `genome` domains are **live**: the pardosa family
(`pardosa`, `pardosa-derive`, `pardosa-encoding`, `pardosa-genome`,
`pardosa-traits`) ships in `Cargo.toml` `members`, and `adr-fmt --context
pardosa` (etc.) resolves to those crates per the `[domains.PAR]` /
`[domains.GEN]` mapping in `adr-fmt.toml`. PAR/GEN ADRs may be edited
per normal ADR process.

The `cherry` domain (prefix `CHE`) is **live**: the workspace ships
`cherry-pit-core`, `cherry-pit-gateway`, `cherry-pit-web`,
`cherry-pit-projection`, `cherry-pit-agent`, `cherry-pit-wq`, and
`cherry-pit-storage`, plus `gh-report` (also CHE-governed
per `adr-fmt.toml`). Cherry ADRs govern those crates and may be edited
per normal ADR process.

`FOCUS.md` at the workspace root is the current architectural-refinement
recipe (successor to the cherry-pit construction phase, which passed its
EVAL-GATE). Read it before taking on cherry-pit refinement work; it
defines the work-unit grain, escalation policy, and out-of-scope
guardrails. It does NOT govern adr-fmt itself; adr-fmt work is governed
by the `AFM-*` ADRs.

## Conventions worth knowing

- `Cargo.lock` is committed deliberately (binary crate; see crate README §
  "Cargo.lock policy"). Do not gitignore it.
- `.ooda/` is gitignored except `.gitkeep`; it holds OODA scratch and
  trace data. Treat as ephemeral.
- `#![forbid(unsafe_code)]` on the binary — keep it.
- License is dual `Apache-2.0 OR MIT`; preserve in any new crate.
- Tests use `assert_cmd` + `predicates` + `tempfile` — no external services,
  no fixtures directory. Self-contained per test.

## Path hygiene (shorter paths = fewer footguns)

Long absolute paths are the leading cause of malformed-path errors in tool
calls (silent `&&` short-circuits, truncated arguments, copy-paste drift).
Two cheap habits keep paths short:

1. **Per-developer short-root symlink.** A symlink such as
   `~/w/solon → ~/Documents/github/acje/solon` collapses the absolute
   prefix from ~55 chars to ~13. Invoke opencode from the short path.
   Caveat: tools that resolve symlinks (`git rev-parse --show-toplevel`,
   `realpath`) return the canonical path; the saving applies to paths
   *agents type*, not paths the OS reports back.
2. **`workdir`-first, relative-body in bash calls.** AGENTS.md § Bash
   hygiene already requires the bash tool's `workdir` parameter over
   `cd <path> && …`. Once `workdir` is set, **prefer relative paths in the
   command body**: `rg foo src/rules/` with `workdir=crates/adr-fmt`
   beats `rg foo crates/adr-fmt/src/rules/` with workdir unset. Shorter
   commands, less room for typos, and the failure mode if `workdir` is
   wrong is a loud "no such file" instead of a silent miss.

Read / Edit / Write tools still require absolute paths by contract — the
saving is bash-side only. That's fine; bash is where malformed paths bite
hardest.

<!-- BEGIN BEADS INTEGRATION v:1 profile:full hash:f65d5d33 -->
## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Dolt-powered version control with native sync
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Check for ready work:**

```bash
bd ready --json
```

**Create new issues:**

```bash
bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
```

**Claim and update:**

```bash
bd update <id> --claim --json
bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `bd ready` shows unblocked issues
2. **Claim your task atomically**: `bd update <id> --claim`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `bd close <id> --reason "Done"`

### Quality
- Use `--acceptance` and `--design` fields when creating issues
- Use `--validate` to check description completeness

### Lifecycle
- `bd defer <id>` / `bd supersede <id>` for issue management
- `bd stale` / `bd orphans` / `bd lint` for hygiene
- `bd human <id>` to flag for human decisions
- `bd formula list` / `bd mol pour <name>` for structured workflows

### Auto-Sync

bd automatically syncs via Dolt:

- Each write auto-commits to Dolt history
- Use `bd dolt push`/`bd dolt pull` for remote sync
- No manual export/import needed!

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `bd ready` before asking "what should I work on?"
- ✅ Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- END BEADS INTEGRATION -->
