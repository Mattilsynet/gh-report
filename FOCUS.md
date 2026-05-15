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

## 3. Starting State (2026-05-13, EVAL-GATE PASS)

Captured at moment of phase transition. Sources cited from the readiness
packet (`.ooda/eval-gate-readiness-1778665592.md` and its prep artefacts).

| Item | State at EVAL-GATE PASS |
|------|--------------------------|
| `cherry-pit-core` | Live 0.1.0; load-bearing in gh-report. |
| `cherry-pit-gateway` | Live 0.1.0; `MsgpackFileStore` only; CHE-0047 R5 implemented. |
| `cherry-pit-projection` | Live 0.1.0; `FileProjectionStore`, `InMemoryProjection`, `ProjectionDriver`. |
| `cherry-pit-web` | Live 0.1.0; axum bridge over `CommandGateway`. |
| `cherry-pit-agent` | Live 0.1.0; composition root + EventBus. |
| `gh-report` | Worked example; 8 `HandleCommand` impls across 3 services; all 6 §6.2 anti-patterns clear. |
| ADR corpus | 55 ratified CHE ADRs (CHE-0001..CHE-0055; CHE-0055 supersedes CHE-0052 per commit `868abfe`). |
| §6.3 baseline | 11/11 exit 0 (build, test, clippy `-D warnings`, fmt, `adr-fmt --lint`, `--tree CHE`, `--context × 5`). |
| Open bd evidence beads | 15 retained-open under active arch-review epics (post-gardener 2026-05-13); ~38 archival beads closed. |
| Commit pins | `868abfe` (CHE-0055 ratification trio: CHE-0055 + cmd-queue-target Ratified + master-architecture-review promoted to tracked); `31b3bf2` (FOCUS §0/§1 finalize). |
| WU-7 epic | `adr-fmt-cli6` closed 2026-05-13. |
| `.ooda/` size | 28 MB post-gardener (reclaimed 5.12 MB / 325 files in aggressive sweep). |
| Phase 2 v1 → v2 transition | 2026-05-14: v1 declared exit on ceremony (6/10 tasks discharged by ADR text only; framework crates circularly counted as "≥2 worked examples"). v2 supersedes per `docs/c4/roadmap.md` v0.3 with mechanical CI-verifiable exit criteria + Track 0.5 (Pardosa research) gate before Track 2 (pardosa wrap). v1 task closures retained for audit; do not re-open. |

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
# (Phase 2 v1 verify additions retained for historical task closures;
# Phase 2 v2 supersedes with the mechanical exit-gate suite below.)
cargo test -p <crate> --features trybuild           # v1 T6 compile-fail fixtures (retained)

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

# Track 4: gh-report consolidation LOC gate
test "$(wc -l < crates/gh-report/src/infra/server/server.rs)" -lt 2500

# Track 5: SEC-0003 bind-in-library integration test (test name TBD per chosen mechanism)
cargo test -p cherry-pit-web --test sec_0003_enforced_at_library_surface
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
| 0.1     | 2026-05-13 | acje + agent | Initial template. Predecessor `FOCUS-cherry-pit-construction.md` v0.4 archived in place via `git mv`. EVAL-GATE cleared 2026-05-13 (verdicts §6.1/§6.2/§6.3 = PASS at `.ooda/eval-gate-2026-05-13.md` once finalized from DRAFT). This file is a barebones template — §1 objective, §3 starting-state additions, §4 axis selection, §5 sequencing, §6 medium-risk escalation list, §7 guardrail confirmations, §8 axis-specific verify commands all `[TO BE FILLED IN]` and await user direction. |
| 0.2     | 2026-05-13 | acje + agent | Adopted 3-phase model (Cleanup / Generalize / Harden). §3 starting state filled in (commit pins `868abfe` / `31b3bf2`, post-gardener state, `adr-fmt-cli6` closed). §4 reshaped: phases replace ungrouped axes; full per-axis detail moved to `docs/c4/roadmap.md` (split for stability — FOCUS.md is the recipe, roadmap.md is the live dashboard). §5 sequencing diagram with cross-phase injection. §6 publication-prep policy = Phase 3 only; medium-risk list populated. §7 deferred-items reframed (Phase 3 review for CHE-0044 / Pardosa / NATS; donor refactor N/A post-Axis-B). §8 per-phase verify additions. `.ooda/refinement-roadmap-draft.md` deleted (lifecycle expired per its own §"What this draft does NOT do" — adoption supersedes draft). All Round-1 / Round-2 grilling decisions committed. |
| 0.3     | 2026-05-13 | acje + agent | Ceremony stripped from all phases (C4 doc refreshes, master-review scheduled-refresh, README quickstart, public-surface audit tasks, CHANGELOG, MSRV declaration, semver docs, license-header audit, docs.rs metadata, crates.io publication actions removed — these were ceremony-shaped tasks, not substance). Phase 3 reframed: correctness + error-withstanding + adversarial-input hardening, not publication-prep. Phase 3 gains Smithy contract models and TLA+ specifications for temporal invariants (scope and tool details deferred to task activation). Axis J (perf/energy) and Publication-prep dropped. Roadmap.md restructured from axis-detail blocks to ordered high-level task lists. §4 condensed (axis labels removed; task list lives in roadmap.md); §6 publication-prep escalation replaced with "crates.io publication = refinement does not publish"; §7 publication guardrail rewritten. |
| 0.4     | 2026-05-14 | acje + agent | Phase 2 v2 (Generalization by Construction) ratified after ceremony-vs-substance review: v1 declared exit on ceremony for 6/10 tasks (cherry-pit-agent + cherry-pit-web circularly counted as "≥2 worked examples"; T2/T4/T5/T7/T8/T9/T10 discharged by ADR text only). §3 starting-state addendum records the v1→v2 transition. §7 amendment: Pardosa/NATS deferral lifted (constrained — tests-only NATS via embedded `nats-server`; production NATS, SEC-0010 TLS, SEC-0011 hash-chain stay Phase 3); CHE-0044 object_store still Phase-3-deferred, decoupled from pardosa activation. §8 Phase-2 verify additions rewritten with the mechanical Phase-2 v2 exit-gate suite (conformance harness, RPITIT audit, pardosa workspace activation, adr-srv full-stack, gh-report LOC gate, SEC-0003 library-surface integration test). Track 0.5 (Pardosa research) prepended per user request: gap analysis surfaced model mismatches (Purged state ↔ Aggregate lifecycle; DomainId↔AggregateId identity; correlation/causation propagation in EventBus) requiring prior-art survey (EventStoreDB / Marten / Axon / Rust crates / NATS / Kafka) + verdict ratification before Track 2 (pardosa wrap). §2 invariants unchanged; any CHE amendment recommended by Track 0.5 is its own user-ratification round. Companion: `docs/c4/roadmap.md` v0.3. |
