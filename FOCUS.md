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

Prerequisite reading before working any refinement mission:

- `docs/STORY.md` — strategic intent (apex over ADR corpus on *why* and
  *where to play*; see § 10 below for the document hierarchy).
- `docs/CLOSURE.md` — v0.1 exit gate (terminal milestone document;
  archives to `docs/stale/` on green).

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
| Remaining Phase 2 v2 tracks | Track 3 (adr-srv, read-only re-scope), Track 4.4 (validate.rs migration), Track 5 (SEC-0003 bind-in-library), Track 6 (pardosa-genome file format; PAR-0021 + F9 atomic-ship), Track 7 (gh-report → pardosa hard cut), Track 8 (C3 idiomatic audit) |
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

Cross-cutting language doctrine for Phase 3 is captured as an ideas
register for future RST ADRs (numbering reserved-not-assigned, no
decisions taken). Phase-3 task #13 (`docs/c4/roadmap.md` §F) reviews
the register against in-flight work and promotes candidates only
where Phase-3 tasks have already created concrete pain or
worked-example evidence. Drafting any RST ADR from the register
remains user-ratified per §6.

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
- **Edits to `docs/STORY.md`, and any ADR amendments they entail.**
  STORY is apex over the ADR corpus on *why* and *where to play*; on
  disagreement, the ADR is rewritten or superseded. STORY edits and
  the consequent ADR edits land as one user-ratified commit-set.
  Unratified disagreement is a release blocker — file `story-override`
  beads per defected ADR; never act on the unresolved gap. See
  STORY.md § 9.
- **Edits to `docs/CLOSURE.md` that change the v0.1 exit gate**
  composition, the closure inventory, or the in-scope / out-of-scope
  boundary. Recording a closed-gate tick is routine, not escalation.
  Declaring v0.1 shipped (annotating `Status: Discharged` and
  archiving to `docs/stale/`) is always-escalate.

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
- **First persisted pardosa event in any consumer.** Gated on Track 6
  atomic-ship complete (PAR-0021 F2 chain + F9 type-surface together,
  `FORMAT_VERSION = 3` in tree). Roadmap Tracks 3.A (adr-srv ADR scrape)
  and 7 (gh-report hard cut) carry this gate explicitly. "Parallel" in
  the roadmap means concurrent agents on disjoint crate trees, not
  concurrent first-writes onto a still-changing wire format. Phase 2
  v2 only; lifts once Track 6 closes.
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

# Track 3: adr-srv read-only (Phase 2 v2 re-scope 2026-05-17)
# C1 — adr-srv operational; ADR corpus scraped, served via GraphQL Query.
cargo test -p adr-srv
cargo test -p adr-srv --test scrape_pipeline      # Track 3.A: idempotent re-scrape
cargo test -p adr-srv --test graphql_read_e2e     # Track 3.3: Query schema + projection
cargo run  -p adr-srv &                           # smoke: server starts
# { adr(id: "AFM-0001") { title, references { id } } } returns scraped + projected ADR
# Mutations (ratifyAdr / supersede) and metacircular lint_integration RETIRED
# to Phase 3 (roadmap §G items 16 + 17, formerly 3 + 4 before v0.11 renumber).

# Track 4 mechanical LOC gate retracted v0.7 (2026-05-16); substance
# evidence lives in CHE-0062 + CHE-0049-Amendment-Part-2 + SMI-1..SMI-5.

# Track 5: SEC-0003 bind-in-library integration test (test name TBD per chosen mechanism)
cargo test -p cherry-pit-web --test sec_0003_enforced_at_library_surface

# Track 6: pardosa-genome file-format atomic-ship (Epic 6.A PAR-0021 + Epic 6.B F9).
# Gate for any first persisted event in adr-srv (Track 3.A) or gh-report (Track 7).
rg -n 'FORMAT_VERSION *= *3' crates/pardosa-genome/src/format.rs
cargo test -p pardosa-genome
cargo test -p pardosa-genome --test tamper_injection   # F2f

# Track 7: gh-report → pardosa hard cut. C2 discharged here.
cargo test -p gh-report                           # green on pardosa-genome backend
rg -n 'msgpack.*store|MsgpackEventStore' crates/gh-report/src/   # zero hits
cargo run -p adr-fmt -- --refs CHE-0031           # supersession ADR cites CHE-0031

# Track 4.0 SMI invariant maintained across Track 7 cut-over:
rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/ && exit 1 || true
cargo test -p gh-report --test smi_replay_equivalence

# Track 8: C3 idiomatic architectural-organization audit.
# Audit-report bead + per-crate remediation beads under labels track:8,remediation.
bd query --label 'track:8,remediation' --json
# Every Phase-2 remediation bead either closed or labelled phase:3-harden with rationale.
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

## 9. Document Hierarchy

The Solon governance plane has six load-bearing documents plus the ADR
corpus. Their relationship is fixed; an agent should not have to
reconstruct it.

```
docs/STORY.md   — strategic intent. Apex over the ADR corpus on *why*
                  and *where to play*. Read once for orientation.
docs/adr/       — binding doctrine. The catalogue of constraints; the
                  authority for *what* invariants hold. Read continuously
                  via `adr-fmt --context <crate>`.
FOCUS.md        — refinement recipe. The standing *how we work* during
                  the refinement phase. Read by moltke at every mission
                  decomposition.
docs/c4/roadmap.md
                — live operational dashboard. Per-track status, per-task
                  detail. Read by moltke + hopper at mission dispatch.
docs/CLOSURE.md — v0.1 exit gate. Terminal milestone; archives to
                  `docs/stale/` on green. Indexes roadmap.md; never
                  duplicates it.
AGENTS.md       — agent collaboration doctrine. Orthogonal to the above;
                  governs *how agents work together*, not *what they
                  build*.
```

Disagreement-resolution rule (cross-references STORY.md § 9):

- **STORY ↔ ADR.** STORY is apex. On disagreement, the ADR is in
  defect: rewrite or supersede the ADR. Never silent — file
  `story-override` beads per defected ADR, blocker on dependent work,
  user ratifies STORY edit + ADR amendments as one commit-set.
- **STORY ↔ FOCUS / roadmap / CLOSURE.** STORY overrides; the
  operational document amends to match.
- **ADR ↔ ADR.** Existing supersedes mechanism (S0xx, AFM-0020 parent
  edges). No change.
- **FOCUS ↔ roadmap ↔ CLOSURE.** FOCUS is the recipe; roadmap is the
  live state; CLOSURE indexes roadmap at the v0.1-relevant grain.
  CLOSURE never duplicates roadmap content — it points at it.
- **AGENTS.md ↔ all of the above.** AGENTS.md governs agent behaviour
  only. It is not consulted for product / architectural decisions and
  does not override product / architectural documents.

This section is itself FOCUS-class doctrine; edits are always-escalate
per § 6.

---

## 10. Revision History

| Version | Date       | Author | Changes |
|---------|------------|--------|---------|
| 0.1–0.6 | 2026-05-13 → 2026-05-16 | acje + agent | Phase model adoption, ceremony strip, Phase 2 v1→v2 supersession, Track 4.0 SMI injection, LOC-gate amend-then-retract. See git log + bd for detail. |
| 0.7     | 2026-05-16 | acje + agent | **v0.6 amendment retracted; LOC gate sunset entirely; bespoke scripts removed.** Half-day audit of `scripts/prod-loc` (574 LOC syn-AST production-LOC counter) + `scripts/track4-verify` (711 LOC, 13-criterion harness) found: ~10 of the 13 `track4-verify` criteria duplicate existing CI jobs (`build`, `test`, `clippy`, `fmt`); the unique value (LOC non-regression gate, SMI rg checks, audit-trail/alias verifications) collapses to 4-6 inline CI shell steps; the LOC gate the tooling enforced is itself a proxy gate (architectural substance — duplication deleted, libraries consolidated — is observable in commit diffs and ADRs, not in a line count). User verdict (verbatim): *"the gain is too small and the cost of drift and clutter is real. remove"*. Actions: (1) `git rm -r scripts/prod-loc scripts/track4-verify scripts/citation-diff`; (2) `git rm scripts/{adr_agg,adr_rules,loc,loc_agg}.awk` (the "honest historical record" of v0.6 was equally subject to the drift-and-clutter verdict); (3) `.gitignore` re-adds `scripts/` so future throwaway tooling does not silently accumulate — promote any genuinely durable tool to its own `crates/` member with an ADR justifying it; (4) `.github/workflows/ci.yml` job `track4-gates` deleted; (5) §3 row + §8 Track-4 verify block rewritten to reflect retraction. **Doctrine lesson recorded:** v0.6 amended a malformed gate by building tooling to enforce the amendment — substituted measurement infrastructure for the underlying question "is the gate's substance worth measuring at all?". v0.7 answers no. Track 4 epic `adr-fmt-ysaa` remains CLOSED — retraction does not reopen exit criteria, it removes mechanical enforcement that was duplicative of `cargo test` / `cargo clippy` / `cargo fmt` plus the substance evidence already in CHE-0062 / CHE-0049-Amendment-Part-2 / SMI-1..SMI-5. Net code change: −1285 LOC bespoke scripts + −15 LOC `.gitignore` un-ignore + −16 LOC CI job, −1 CI job, +1 `.gitignore` rule. §2 invariants unchanged. §6/§7 guardrails unchanged. SM2 (doc_markdown sweep, also retracted 2026-05-16) is a sibling failure mode — both rounds demonstrated that **building tools to enforce a gate is a higher-order ceremony**: the tool exists, therefore the gate must be real, therefore the proxy is treated as substance. Companion: `docs/c4/roadmap.md` v0.6. |
| 0.8     | 2026-05-17 | acje + agent | **User-ratified Phase 2 v2 completion criteria** synced from `docs/c4/roadmap.md` v0.9: C1 = adr-srv operational in **read-only mode** (scrape ADRs → pardosa-genome → GraphQL Query); C2 = gh-report stores internal state in pardosa-genome files (hard cut, re-scrape GitHub API; no prod deployments); C3 = idiomatic architectural-organization audit across `adr-srv` / `gh-report` / `cherry-pit-*` / `pardosa-*`. **§3 row updated** to enumerate all remaining tracks (3 read-only, 4.4, 5, 6, 7, 8). **§7 guardrail added**: "First persisted pardosa event in any consumer" gated on Track 6 atomic-ship complete — formalises the user direction that PAR-0021 F2 chain + F9 type-surface (`FORMAT_VERSION = 3`) land before any consumer writes. "Parallel" means concurrent agents on disjoint crate trees, not concurrent first-writes. **§8 verify block rewritten**: Track 3 verify drops `graphql_write_e2e` + `lint_integration` (retired to Phase 3 injection-queue items 3 + 4); adds Track 3.A `scrape_pipeline`; adds Track 6 atomic-ship verify (`FORMAT_VERSION = 3` grep + `tamper_injection` test); adds Track 7 (`gh-report` on pardosa, msgpack-store zero hits, CHE-0031 supersession ADR refs); adds Track 8 (`track:8,remediation` bd query). §2 invariants unchanged. §6 escalation policy unchanged. Track 6 atomic-ship preserved (Epic 6.A + 6.B together, per user direction). Track 4.4 + Track 5 placement preserved (sequenced after Track 3.3, per user "as early as possible in Phase 2" direction). Companion: `docs/c4/roadmap.md` v0.9. |
| 0.9     | 2026-05-18 | acje + agent | **§4.3 (Phase 3 — Harden) extended** with one paragraph pointing at the cross-cutting RST hardening ideas register as the source of Phase-3 language-doctrine candidates (advisory framing; numbering reserved-not-assigned; no decisions taken). Roadmap Phase-3 task #13 (§F) reviews the register against in-flight work; drafting any RST ADR remains user-ratified per §6 (always-escalate: new ADR). §8 verify block cross-reference updated to reflect roadmap §G renumber ("items 3 + 4" → "§G items 16 + 17"). §2 invariants unchanged. §6 escalation policy unchanged. Companion: `docs/c4/roadmap.md` v1.1. |
| 1.0     | 2026-05-19 | acje + agent | **STORY.md + CLOSURE.md anchored into governance.** §0 gains a prerequisite-reading block pointing at the two new documents. §6 always-escalate list gains two entries: (a) STORY.md edits + entailed ADR amendments, ratified as one commit-set (apex-over-ADR per STORY.md §9; `story-override` beads block release while open); (b) CLOSURE.md exit-gate composition / scope-boundary changes (recording a closed-gate tick is routine, not escalation; declaring v0.1 shipped is). New §9 Document Hierarchy fixes the six-document topology (STORY apex, ADR binding, FOCUS recipe, roadmap dashboard, CLOSURE v0.1 gate, AGENTS orthogonal) and codifies disagreement-resolution rules across the layers. Old §9 Revision History renumbered to §10. §2 invariants unchanged. §3 starting state unchanged. Companion: `docs/STORY.md` v0.1, `docs/CLOSURE.md` v0.1. |
