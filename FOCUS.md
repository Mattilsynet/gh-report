# FOCUS.md — Architectural Refinement Phase

**Genre**: Refinement recipe
**Status**: Draft
**Phase**: Architectural Refinement (successor to Cherry-Pit Construction)
**Reader**: AI agent — moltke decomposing into hopper missions, or hopper directly
**Predecessor**: `FOCUS-cherry-pit-construction.md` (archived 2026-05-13 at EVAL-GATE PASS)

---

## 0. How to Read This Document

You are an agent. The previous phase asked **"does cherry-pit-* compile, test, and
load-bear inside gh-report?"**— answered YES. The refinement phase prescribes: "Generalize
the cherry-pit-* architecture such that it is idiomatic with DDD; EDA and Hexagonal
architectural concepts and fit for a wide range of applications"

```rust
struct RefinementRecipe {
    objective: Objective,                  // §1 — what "refined" means
    invariants: Vec<Invariant>,            // §2 — inherited from construction; still binding
    starting_state: StartingState,         // §3 — snapshot at EVAL-GATE PASS
    refinement_axes: Vec<RefinementAxis>,  // §4 — orthogonal dimensions of refinement
    sequencing: Option<Dag>,               // §5 — if axes have dependencies
    escalation_policy: EscalationRules,    // §6
    out_of_scope: Vec<Boundary>,           // §7
    verification: VerifyCommands,          // §8
    revision_history: Vec<Revision>,       // §9
}
```

Doctrine: low-risk decisions get the most reversible interpretation, named
explicitly; medium+ risk escalates; surprises halt and re-loop to
copernicus or feynman; ADRs at `docs/adr/cherry/CHE-####-*.md` and
`docs/adr/adr-fmt/AFM-####-*.md` are SSOT.

---

## 1. Objective

Generalize and fortify the architecture to help agents build correct applications
of a wide variety on top of cherry-pit-* crates.

---

## 2. Invariants

The architectural invariants are ADR-backed; refinement is **not** a license to
weaken them. The full table is reproduced here for at-hand reference; the
authoritative source is the CHE ADR corpus at `docs/adr/cherry`.

| Invariant | ADR |
|-----------|-----|
| Design priority: Correctness > Security > Energy efficiency > Response time | CHE-0001 |
| Make illegal states unrepresentable | CHE-0001:P1, CHE-0002 |
| EDA + DDD + hexagonal | CHE-0004 |
| Single aggregate per port instance | CHE-0005:R1 |
| Single-writer per aggregate | CHE-0006 |
| `#![forbid(unsafe_code)]` in every crate | CHE-0007 |
| Pure command handling (no I/O in `handle`) | CHE-0008 |
| Infallible `apply` | CHE-0009 |
| `AggregateId` is `NonZeroU64`, infrastructure-assigned | CHE-0011, CHE-0020 |
| `Aggregate::default()` = zero state | CHE-0012 |
| Commands not serializable (intent ≠ wire data) | CHE-0014 |
| Sync domain, async infrastructure | CHE-0018 |
| Termination is a domain event, not a framework concern | CHE-0023 |
| Persist-then-publish; publication is non-fatal | CHE-0024 |
| RPITIT over `async_trait` | CHE-0025 |
| Cargo workspace with acyclic crate DAG | CHE-0029 |
| Flat public API via `pub use` re-exports | CHE-0030 |
| Append-only event schema | CHE-0022 |

Strengthening an invariant in refinement (e.g. adding a falsifier, narrowing
a trait bound) is in scope. Weakening one requires an ADR amendment + user
ratification (§6).

---

## 3. Starting State

Live state — point-in-time snapshots have been removed; query the SSOTs.

| Item | Pointer |
|------|---------|
| Active phase | Phase 2 v2 (Generalization by Construction) |
| Remaining Phase 2 v2 tracks | Track 3 (adr-srv), Track 4.4 (validate.rs migration), Track 5 (SEC-0003 bind-in-library) |
| Closed-track / closed-task history | bd, labels `phase:1-cleanup` / `phase:2-generalize` |
| Live track-level dashboard | `docs/c4/roadmap.md` |
| ADR corpus state | `cargo run -p adr-fmt -- --tree CHE` (and PAR / GEN / AFM) |
| Workspace members | `Cargo.toml [workspace] members` (SSOT) |

---

## 4. Refinement Phases

The refinement phase runs in three sequential sub-phases. Phases run in
order; **discovered tasks inject into the phase matching their nature**
(cleanup → 1, generalize → 2, harden → 3) regardless of when discovered.

```rust
enum Phase {
    Cleanup    { exit: ExitCriteria },  // debt removal
    Generalize { exit: ExitCriteria },  // make architecture provably general
    Harden     { exit: ExitCriteria },  // correctness + adversarial interface behaviour
}
```

### 4.1 Phase 1 — Cleanup

Discharge architectural debt. Supersede-edge hygiene, donor removal,
P0/P1 remediation cohort.

### 4.2 Phase 2 — Generalize

Make the architecture provably general for new application authors.
ADR corpus navigable, every invariant has a falsifier, ≥ 2 worked
examples demonstrate "wide variety".

### 4.3 Phase 3 — Harden

Correctness under stress; withstanding errors; adversarial behaviour on
interfaces. Fuzz + property suites on trust boundaries; formal
specifications (TLA+ for temporal invariants, Smithy for interface
contracts) agree with implementation. **Not** publication-prep.

### 4.4 Task injection rules

Tasks discovered mid-phase that match an earlier or later phase's nature:

- **Blocker** → execute inline before resuming current work (it is a
  hidden prereq of the originating task).
- **Non-blocker** → file bd bead with the appropriate phase label;
  sweep in the next batch for that phase.

Bead labels:

- `phase:1-cleanup`
- `phase:2-generalize`
- `phase:3-harden`

Injection events are logged in `docs/c4/roadmap.md` "Injection log" with
date, discovered-during phase, routed-to phase, bead id, reason, and
blocker flag.

### 4.5 Task list

The ordered, high-level task list for each phase lives in
`docs/c4/roadmap.md`. Tasks are short and concrete; sub-decomposition
happens at mission-dispatch time.

### 4.6 Phase-state dashboard

`docs/c4/roadmap.md` carries the live phase-state table (which phase is
active, when boundaries cleared, exit-criteria progress). moltke reads
roadmap.md before each mission decomposition; FOCUS.md is the standing
recipe and changes only when the phase model itself changes.

---

## 5. Sequencing

Phases run sequentially; discovery injects across boundaries.

```
Phase 1 (Cleanup) ─► Phase 2 (Generalize) ─► Phase 3 (Harden)
       ▲                    ▲                       │
       │                    │                       │
       └──── inject ◄───────┴─── inject ◄───────────┘
       └──── inject ◄───────────────────────────────┘
                            └──── inject ◄──────────┘
```

- **Within a phase**, tasks may interleave when they don't depend on
  each other. moltke decomposes per directed opportunism.
- **Across phases**, exit criteria gate advancement. Phase 2 cannot open
  until Phase 1 exit criteria all green; Phase 3 cannot open until Phase 2
  exit criteria all green.
- **Injection** happens any time a task is discovered to belong to an
  earlier or later phase. Blockers execute inline; non-blockers queue
  with a `phase:N-<name>` bead label. See `docs/c4/roadmap.md` §Injection
  log for the audit trail.

Per-phase task list and exit criteria: see `docs/c4/roadmap.md`.

---

## 6. Escalation Policy

Same baseline as construction phase: low-risk = act with stated
assumption; medium+ = ask; high-risk = always ask.

**Always escalate** (high risk):

- Drafting a new CHE ADR.
- Editing an existing CHE ADR. (Supersede via new ADR + user ratification.)
- Weakening any §2 invariant.
- **Phase boundary advancement** (declaring Phase N → Phase N+1) — user
  ratifies each transition.
- **crates.io publication** or any equivalent irreversible release
  action. Refinement does not publish.
- Changes to `adr-fmt.toml` corpus configuration.

**Escalate after exhausting cheap evidence** (medium risk):

- Drafting new SEC ↔ CHE cross-references that affect implementation
  surface (Phase 2 / Phase 3).
- Picking the domain for the minimal worked example (Phase 2).
- RST-0005 status decision: elevate to Accepted or retire (Phase 2).
- TLA+ / Smithy scoping decisions at Phase 3 task activation.
- Routing a discovered task to a different phase via the injection log
  when blocker-vs-non-blocker classification is non-obvious.

**Do NOT escalate** (low risk — proceed with stated assumption):

- Code formatting, naming within established conventions, doc-comment phrasing.
- Tests strengthening existing invariants.
- Refactors that preserve public API.

When escalating: batch questions, present at one checkpoint, recommend a
default for each.

---

## 7. Out of Scope (Guardrails)

Refinement-phase guardrails. Items deferred to a later phase are marked
with their phase target; items permanently out of scope are marked
"permanent".

- **Pardosa as second EventStore impl** — Phase 2 v2 activates
  `crates/pardosa`, `crates/pardosa-genome`, `crates/pardosa-derive`
  as workspace members and wraps them behind `cherry_pit_core::EventStore`.
  Wrapping shape determined by **Track 0.5 (Pardosa research)** verdict
  (purged-state ↔ aggregate lifecycle, identity model, correlation/causation
  propagation, prior-art survey). NATS / JetStream lights up **for tests
  only** (embedded `nats-server`) — production NATS deploy, SEC-0010 (TLS),
  and SEC-0011 (hash-chain) remain Phase 3.
- **CHE-0044 object_store backend.** Still deferred to Phase 3 review;
  decoupled from pardosa activation. Phase 3 may lift the deferral or
  confirm it.
- **Add async runtime dependencies to `cherry-pit-core`.** Permanent.
  Invariants CHE-0018:R3 / CHE-0029:R4.
- **Introduce `Box<dyn EventStore>`.** Permanent. Invariant CHE-0005:R1.
- **Run `git push`** or any irreversible network operation without
  explicit user instruction. Permanent.
- **Refactor donor source in place.** N/A after Phase-1 task 2
  (`quics-web` is the last donor; removal closes the surface).
- **Publication / release work** during refinement. Refinement is
  Cleanup → Generalize → Harden; publication is a separate concern
  handled outside this recipe.

---

## 8. Verification Commands

For a hopper executing any refinement mission, the standard verify suite:

```
# Per-crate
cargo test   -p <crate>            # all tests including doctests
cargo doc    --no-deps -p <crate>  # rustdoc clean
cargo clippy -p <crate> --all-targets -- -D warnings

# Workspace-wide
cargo build  --workspace
cargo test   --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt    --check
cargo run    -p adr-fmt -- --lint
cargo run    -p adr-fmt -- --tree CHE
```

ADR navigation while refining:

```
cargo run -p adr-fmt -- --refs CHE-####     # what cites this ADR
cargo run -p adr-fmt -- --context <crate>   # decision rules for crate
cargo run -p adr-fmt -- --tree CHE          # full cherry domain tree
```

### Per-phase verify additions

**Phase 1 (Cleanup)** — baseline above is sufficient. Plus:

```
rg 'quics-web|quics_web' crates/            # must return 0 after task 2
cargo run -p adr-fmt -- --refs CHE-0052     # task 1: only historical refs
```

**Phase 2 (Generalize)** — baseline plus:

```
# Phase 2 v2 — Generalization by Construction
# Track 1.3: trait-conformance suite (any-impl-must-pass)
cargo test --workspace --test '*_conformance'

# Track 1.4: RPITIT audit (no async-trait in cherry-pit-* dep trees)
for c in cherry-pit-core cherry-pit-gateway cherry-pit-web cherry-pit-agent \
         cherry-pit-projection cherry-pit-wq cherry-pit-storage; do
  cargo tree -p "$c" -e features 2>&1 | rg -q async-trait && exit 1 || true
done

# Track 2.1: pardosa workspace activation
cargo build --workspace                     # pardosa* now workspace members
cargo run -p adr-fmt -- --tree PAR          # PAR domain reachable
cargo run -p adr-fmt -- --context pardosa   # crate-resolved context

# Track 2.2: pardosa as second EventStore (load-bearing)
cargo test -p <pardosa-store-crate> --test '*_conformance'

# Track 2.3: NATS substrate, tests-only
cargo test -p <pardosa-store-crate> --features nats

# Track 3: adr-srv full stack
cargo test -p adr-srv
cargo test -p adr-srv --test graphql_read_e2e
cargo test -p adr-srv --test graphql_write_e2e
cargo test -p adr-srv --test lint_integration   # metacircular adr-fmt-as-projection

# Track 4 mechanical LOC gate retracted v0.7 (2026-05-16); substance
# evidence lives in CHE-0062 + CHE-0049-Amendment-Part-2 + SMI-1..SMI-5.

# Track 5: SEC-0003 bind-in-library integration test (test name TBD per chosen mechanism)
cargo test -p cherry-pit-web --test sec_0003_enforced_at_library_surface

# Track 4.0 SMI invariant maintained (Track 4.0 closed 2026-05-16):
rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/ && exit 1 || true
cargo test -p gh-report --test smi_replay_equivalence
```

**Phase 3 (Harden)** — baseline plus:

```
cargo test -p <crate> --features fuzz       # adversarial-input harnesses
cargo test -p <crate> --features proptest   # error-path property tests
# Smithy + TLA+ verify commands defined at task activation
```

Phase advancement requires the originating phase's baseline + additions
all green AND `docs/c4/roadmap.md` exit criteria all checked.

---

## 9. Revision History

| Version | Date       | Author | Changes |
|---------|------------|--------|---------|
| 0.1–0.6 | 2026-05-13 → 2026-05-16 | acje + agent | Phase model adoption, ceremony strip, Phase 2 v1→v2 supersession, Track 4.0 SMI injection, LOC-gate amend-then-retract. See git log + bd for detail. |
| 0.7     | 2026-05-16 | acje + agent | **v0.6 amendment retracted; LOC gate sunset entirely; bespoke scripts removed.** Half-day audit of `scripts/prod-loc` (574 LOC syn-AST production-LOC counter) + `scripts/track4-verify` (711 LOC, 13-criterion harness) found: ~10 of the 13 `track4-verify` criteria duplicate existing CI jobs (`build`, `test`, `clippy`, `fmt`); the unique value (LOC non-regression gate, SMI rg checks, audit-trail/alias verifications) collapses to 4-6 inline CI shell steps; the LOC gate the tooling enforced is itself a proxy gate (architectural substance — duplication deleted, libraries consolidated — is observable in commit diffs and ADRs, not in a line count). User verdict (verbatim): *"the gain is too small and the cost of drift and clutter is real. remove"*. Actions: (1) `git rm -r scripts/prod-loc scripts/track4-verify scripts/citation-diff`; (2) `git rm scripts/{adr_agg,adr_rules,loc,loc_agg}.awk` (the "honest historical record" of v0.6 was equally subject to the drift-and-clutter verdict); (3) `.gitignore` re-adds `scripts/` so future throwaway tooling does not silently accumulate — promote any genuinely durable tool to its own `crates/` member with an ADR justifying it; (4) `.github/workflows/ci.yml` job `track4-gates` deleted; (5) §3 row + §8 Track-4 verify block rewritten to reflect retraction. **Doctrine lesson recorded:** v0.6 amended a malformed gate by building tooling to enforce the amendment — substituted measurement infrastructure for the underlying question "is the gate's substance worth measuring at all?". v0.7 answers no. Track 4 epic `adr-fmt-ysaa` remains CLOSED — retraction does not reopen exit criteria, it removes mechanical enforcement that was duplicative of `cargo test` / `cargo clippy` / `cargo fmt` plus the substance evidence already in CHE-0062 / CHE-0049-Amendment-Part-2 / SMI-1..SMI-5. Net code change: −1285 LOC bespoke scripts + −15 LOC `.gitignore` un-ignore + −16 LOC CI job, −1 CI job, +1 `.gitignore` rule. §2 invariants unchanged. §6/§7 guardrails unchanged. SM2 (doc_markdown sweep, also retracted 2026-05-16) is a sibling failure mode — both rounds demonstrated that **building tools to enforce a gate is a higher-order ceremony**: the tool exists, therefore the gate must be real, therefore the proxy is treated as substance. Companion: `docs/c4/roadmap.md` v0.6. |
