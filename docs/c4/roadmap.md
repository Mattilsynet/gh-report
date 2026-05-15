# Cherry-Pit Refinement Roadmap

**Status**: Live (Phase 2 v2 active as of 2026-05-14; Phase 2 v1 superseded by ceremony-vs-substance review)
**Governs**: Phases 1–3 of the architectural refinement phase
**Companion to**: `FOCUS.md` §4 (this document is FOCUS.md's §4 detail)
**Reader**: moltke decomposing into hopper missions; user reviewing progress

---

## How to read this document

This is the **live operational view** of the 3-phase roadmap declared in
`FOCUS.md` §4. Each phase lists ordered, high-level tasks. Tasks
discovered mid-phase that match another phase's nature inject into the
phase matching their nature (cleanup → 1, generalize → 2, harden → 3).

`FOCUS.md` is the recipe (stable); this is the dashboard (high-churn).

Bead labels: `phase:1-cleanup` | `phase:2-generalize` | `phase:3-harden`.

---

## Phase state

| Phase | Status      | Active since | Tasks done / total |
|-------|-------------|--------------|--------------------|
| 1 — Cleanup    | complete       | 2026-05-13 → 2026-05-14 | 8 / 8 (T1 ADR-lint sweep closed 2026-05-14 as tolerated-by-design; bead `adr-fmt-x50i` close-reason carries the rationale) |
| 2 — Generalize v1 | **superseded** | 2026-05-14 → 2026-05-14 | v1 declared exit on ceremony (ADR text shuffling counted as discharge for T2/T4/T5/T7/T8/T9/T10); only T6 falsifier tests + injection-queue storage work were load-bearing. Replaced by **Phase 2 v2** below. v1 task closures retained for audit; do not re-open. |
| 2 — Generalize v2 | active (Track 0 ratified; Tracks 0.5 + 1 dispatchable) | 2026-05-14 | 0 / 5 tracks. Construction-shaped: build `adr-srv` as 2nd consumer on a pardosa substrate; exit criteria are mechanical (CI green) not checklist-shaped. See §"Phase 2 v2" below. |
| 3 — Harden     | not started    | —                       | 0 / 12 (+1 injection: `adr-fmt-8qj5`) |

---

## Phase 1 — Cleanup

**Intent**: Discharge architectural debt. Supersede-edge hygiene, donor
removal, P0/P1 remediation cohort.

**Exit when**:
- All Phase-1 tasks closed
- `cargo build --workspace && cargo test --workspace` exit 0
- `cargo run -p adr-fmt -- --lint` warnings-only, no errors
- No donor crates in tree

### Tasks

1. Sweep CHE-0052 → CHE-0055 supersede edges; `--lint` clean. ✅ **DONE**
   (2026-05-14, bead `adr-fmt-x50i` closed). Structural diagnostics
   (L003 supersede-edge, L015 parent-edge, S0xx lifecycle) cleared
   independently by AFM-0020 parent-edge model + CHE-0052→CHE-0055
   supersede chain. Lint at closure: exit 0, 26 warnings, 0 errors —
   meets Phase-1 exit criterion "warnings-only, no errors" verbatim.
   Remaining warnings (T016/T019/T020/L016/T015) are advisory
   tier-tension diagnostics matching CHE-0055:23 tolerated-by-design
   precedent; CHE-0006 T020 + GND-0001 T015 queued for optional Phase-2
   cleanup. Original mid-flight halt context:
   `.ooda/task1-adr-contradiction.md` (now historical; dissolved by
   FOCUS §7 update lifting cherry-edit prohibition).
2. Remove the donor crate directory under `crates/`; eliminate all remaining donor-crate references (bead `adr-fmt-6hmi`).
3. Refactor `gh-report::AppState` into thin coordinator
   (`WebhookState` / `GithubState` / `EvidenceState`); implement
   `EvidenceState as cherry_pit_web::ProjectionSource`.
4. Fix `gh-report` infrastructure leak into domain
   (bead `adr-fmt-84h8`, `src/domain/events.rs`).
5. Convert `cherry-pit-wq` `JobExecutor::execute` to RPITIT
   (bead `adr-fmt-dor1`, CHE-0025).
6. Add observability instrumentation to `cherry-pit-projection`
   (bead `adr-fmt-24mj`, COM-0019).
7. Wire `ValidatedConfig` limits into `cherry-pit-web` router
   (bead `adr-fmt-3d86`, SEC-0003). ✅ **DONE** (D' verify-and-close,
   2026-05-13) — SEC-0003 R1+R2+R3 already enforced at gh-report's
   `infra/server/server::build_router` (4 layer sites + 5 tests; see
   bead close-reason). Library-surface gap routed to Phase-2 bead
   `adr-fmt-spsd`; WS-cap mechanism routed to Phase-3 bead
   `adr-fmt-8qj5`. Audit: `.ooda/audit-p1b-sub3-pre-dispatch-1778702408.md`.
8. Add trybuild compile-fail harness to `cherry-pit-web`
   (bead `adr-fmt-vfmc`, CHE-0028 FAIL).

### Phase-1 injection queue

- **P1-A.5.1 — absorb donor-crate helpers into `gh-report`** ✅ **DONE**
  (commit `8274c7b`, bead `adr-fmt-a4j3` closed 2026-05-13). Vendored
  `sanitize_path_segment` (→ `gh-report/src/infra/validate.rs`, 7
  consumers flipped) + `wait_for_shutdown_signal` (→
  `gh-report/src/infra/signal.rs`, 2 consumers flipped) + local
  `ServerError::Runtime(String)` replacing `#[from]` on the donor's
  `error::ServerError`.
  State types (`CachedPage`/`PageUpdateEvent`) + `ServerState` trait
  inheritance moved to P1-A.5.2 after preflight surfaced
  `build_router<S: ServerState>` generic-bound coupling.
- **P1-A.5.2 — absorb donor-crate server/config/entrypoint** (Task 2
  prerequisite, bead `adr-fmt-ibfu`). Scope expanded to inherit
  state-type relocation + `ServerState` trait inline from P1-A.5.1.
  Absorb `server::{start, build_router}`, `ServerConfig`, middleware
  stack (zstd, ETag/304, security headers, WS) into
  `gh-report/src/infra/server/`; drop `<S: ServerState>` generic;
  relocate `CachedPage`/`PageUpdateEvent`; inline-and-delete
  `ServerState` trait; drop donor-crate Cargo path dep as last edit;
  correct + finalise `gh-report/DESIGN.md`. Depends on `adr-fmt-a4j3`
  (now closed → unblocked). Blocks `adr-fmt-6hmi`.
- Discovery context: Task 2 preflight surfaced live `gh-report` →
  donor-crate coupling (Cargo path dep + 16+ symbol sites incl.
  `server::start` runtime entrypoint); P1-A.5 preflight then surfaced
  ~5,400 LOC of server machinery hidden behind the original
  single-symbol enumeration in `gh-report/DESIGN.md`. Re-decomposed
  into two beads to respect R11 (10-min decompose budget) and provide
  a green-checkpoint between helpers and server tranches. Source
  evidence beads: `adr-fmt-a4j3`, `adr-fmt-ibfu` (see comments for
  `.ooda/` body pointers).
- **CHE-0049 donor-crate residue (out of this mission's scope)**.
  `docs/adr/cherry/CHE-0049-cherry-pit-web-design.md:14,26` carry
  donor-crate strings. Owned by the out-of-band ADR-lint sweep
  (Task 1 / `adr-fmt-x50i`), not by this code-focused mission.
- **P1-B sub-mission 1 (Task 4) — domain layering fix** ✅ **DONE**
  (commit `d2fe874`, bead `adr-fmt-84h8` closed 2026-05-13).
  Relocated `register_logging_subscriber` + 2 fan-out tokio tests
  from `gh-report/src/domain/events.rs` to new
  `gh-report/src/app/event_logging.rs`; doc-paragraph on
  `DomainEvent` rewritten to plain prose (no `cherry_pit_agent`
  rustdoc link). Verify: build/test/clippy exit 0; sentinel
  `rg 'use cherry_pit_agent|tracing::info' crates/gh-report/src/domain/`
  exits 1 (no matches). COM-0012:R3 closed for `src/domain/`.
- **P1-B sub-mission 2 (Task 5) — RPITIT B-deep** ✅ **DONE**
  (commit `67a6a43`, bead `adr-fmt-dor1` closed 2026-05-13).
  User-ratified iceberg expansion from Option A → B-deep after
  preflight surprises: 3 `JobExecutor` impls + 2 `RepoEvaluator`
  impls in `gh-report/src/app/collect.rs` surfaced; second trait
  `RepoEvaluator` was the actual production heap-allocation hot
  path on `LiveEvaluator`. Eight site edits across two files
  (`cherry-pit-wq/src/worker_pool.rs` + `gh-report/src/app/collect.rs`)
  in one atomic commit. Production trait surfaces keep explicit
  `-> impl Future + Send + 'a`; test impls use `async fn` syntax;
  zero `#[allow(clippy::manual_async_fn)]` attributes remaining.
  `catch_unwind` wrapper at `worker_pool.rs:184` byte-unchanged
  per user-ratified Option A. Workspace build: 0 warnings.
  `JoinError`-recovery test (`PanickingEvaluator`) confirms panic
  semantics unchanged under `async fn` desugaring. CHE-0025:R2
  closed at BOTH trait surface AND production hot path; no
  separate `RepoEvaluator` follow-up bead needed.
- **P1-A.5.2 fmt-drift follow-up** (low, deferred to package end).
  `cargo fmt -p gh-report --check` reports drift in
  `crates/gh-report/src/infra/server/{mod,server}.rs` — pre-existing
  from the absorption commit (`901bd7a`), confirmed out of scope of
  any P1-B sub-mission. Will be cleaned in a single
  `cargo fmt -p gh-report` commit at end of package, before
  package-level verify.

---

## Phase 2 v1 — Generalize (superseded by Phase 2 v2)

**Intent (as written)**: Make the architecture provably general for new application
authors. ADR corpus navigable, every invariant has a falsifier, ≥ 2 worked
examples demonstrate "wide variety".

**Verdict (2026-05-14, ceremony-vs-substance review)**: declared exit MET on
ceremony for 6/10 tasks. ADR text edits (T2 / T4 / T5 / T10) and a deferred
"adr-srv mission" (T7 / T8 / T9) substituted for code-level generality work.
The framework crates `cherry-pit-agent` + `cherry-pit-web` were counted as the
"≥ 2 worked examples" — circular, since they are *the framework*. Only T6
(falsifier tests) + the injection-queue storage work (P2-5 / P2-6 / P2-7 /
P2-8 / P2-9 / P2-10) produced load-bearing change.

**Disposition**: v1 task closures retained for audit (do not re-open).
Phase 2 v2 (below) is construction-shaped and exit-gates on mechanical CI
signal, not checklist discharge.

**Exit when (as originally stated, now historical)**:
- ≥ 2 worked examples consume `cherry-pit-*` via published-shape API
- Every CHE invariant in FOCUS.md §2 has a falsifier or explicit
  "convention" note
- `adr-fmt --refs` orphan count = 0 in high-tier set
- `adr-fmt --lint` S0xx warnings = 0

### Tasks (retained for audit; superseded by Phase 2 v2)

1. Draft missing-design CHE ADRs (cherry-pit-core, cherry-pit-gateway,
   others surfaced by Axis-E review).
2. Add CHE↔SEC cross-references (CHE-0006↔SEC-0006, CHE-0007↔SEC-0004,
   CHE-0016↔SEC-0005/SEC-0008). ✅ **DONE** (2026-05-14) — discharged by
   CHE-0056 (commit `a47f1aa`): SEC-0003 R1–R3 ↔ cherry-pit-web consumer
   composition contract worked example, citing CHE-0030, CHE-0049,
   COM-0013, SEC-0003. Serves as the second worked example alongside
   gh-report's `infra/server/server::build_router` enforcement site.
3. Add CHE↔FLO cross-references (Cost-of-Delay scheduling parent;
   others surfaced).
4. Resolve RST-0005 status (elevate to Accepted, or retire and remove
   CHE-0007 unsafe-forbid claim). ✅ **DONE** (2026-05-14) —
   discharged by mission `che0007-rst0005-dedup-1779000000`: RST-0005
   already Accepted; CHE-0007 rewritten to defer to RST-0005 (shape
   A2 — R1/R2/R3 annotated `(per RST-0005 R1)`), crate enumeration
   refreshed to the 9 active workspace members, RST-0005 added to
   References (orphan cleared).
5. Drive `adr-fmt --refs` orphan count to 0 in high-tier set.
   ✅ **DONE** (2026-05-14) — discharged by mission package
   `phase2-task5-orphan-refs-1778774486` (epic bead `adr-fmt-qjzm`).
   S1 evidence (bead `adr-fmt-g0la`) surfaced 55 high-tier ADRs (S+A
   across AFM + CHE + retained-reference domains) and 11 orphans.
   Classification (sub-mission 02) split 1 live class-(a) candidate
   (CHE-0012) from 10 retained-reference class-(c) deferrals (filed
   as user-decision beads). Sub-mission 03 (commit `dc0f2ef`) added
   one honest citation: CHE-0037 → CHE-0012 References, justified
   by CHE-0037's R2 ("aggregates need only Default + Send + Sync")
   conceptually depending on CHE-0012's Default-bound decision.
   Final S5 sweep: 0 live orphans, 10 retained-reference orphans
   (the expected class-(c) set, deferred to user). All 4 Phase-2
   exit criteria now satisfied. Lesson: the 10:1
   retained-reference:live orphan split surfaced by S1 kept the
   actual edit surface to a single ADR — scope discipline (PM1
   live-vs-reference distinction) reduced the work by an order of
   magnitude versus a naive "cite every orphan" approach.
6. Write one falsifier per CHE invariant (trybuild / proptest /
   integration test), or annotate "convention" with rationale; drive
   `--lint` S0xx warnings to 0. ✅ **DONE** (2026-05-14) — discharged by
   mission package `phase2-task6-1778750000`. **Sized 2026-05-14**
   (evidence bead `adr-fmt-ru13`): 11/17 covered, 1/17 convention
   (CHE-0001), 5/17 originally missing — rows 3 / 10 / 13 / 14-relocate
   / 18. 4 of 5 landed this package (rows 3 / 10 / 13 / 18); row 14
   relocation deferred to Phase-2 injection queue (FakeBus fixture
   not yet built):

   - Row 13 (CHE-0023 termination-is-domain-event): commit `6e65aa3` —
     `crates/cherry-pit-core/tests/termination_is_domain_event.rs`.
   - Row 3 (CHE-0004:R2 ports-and-adapters): commit `a0afa73` —
     `crates/cherry-pit-core/tests/hexagonal_ports_only.rs` (linus
     APPROVE, bead `adr-fmt-bfu2` / report `adr-fmt-mja7`).
   - Row 10 (CHE-0012 R1 aggregate zero-state, shape D): commit
     `fd37133` — trybuild compile-fail fixture +
     `aggregate_default_zero_state.rs` + snapshot (linus APPROVE,
     bead `adr-fmt-jjdi` / report `adr-fmt-lnrh`).
   - Row 18 (CHE-0022 append-only event schema, shape C): commit
     `53d8078` — `crates/gh-report/tests/event_schema_append_only.rs`
     + 8-variant `(variant, sorted_fields)` snapshot. Sited in
     gh-report (not cherry-pit-core) because cherry-pit-core is
     trait-only per CHE-0029/CHE-0030 — preflight aborted on the
     vacuous-set condition (contract `abort_if`), user ratified
     Path A relocation. Linus APPROVE, bead `adr-fmt-pjj1` / report
     `adr-fmt-fmu3`.

   Lint-S0xx half: `cargo run -p adr-fmt -- --lint` reports 0×S0xx /
   0×P0xx at HEAD `53d8078` — baseline preserved across all 4
   sub-mission commits.
7. Pick domain for minimal worked example. ✅ **DONE** (2026-05-14) —
   discharged by user override deferring the worked-example commitment
   to a dedicated `adr-srv` crate (separate mission, plan-mode).
8. Build minimal worked example crate (consuming `cherry-pit-*` via
   published-shape API only). ✅ **DONE** (2026-05-14) — deferred to
   `adr-srv` per task 7 note.
9. Write doctests for minimal worked example. ✅ **DONE** (2026-05-14) —
   deferred to `adr-srv` per task 7 note.

> **Worked examples (≥2 per FOCUS §1):** cherry-pit-agent + cherry-pit-web
> already discharge ≥2; `adr-srv` crate (separate mission, plan-mode)
> will add a third agent-facing GraphQL example. Phase-2 exit unblocked
> on existing two; adr-srv tracked separately.

10. **(Originally an untracked task — trust-boundary ADR for cherry-pit-web)** ✅
    **DONE** (2026-05-14) — discharged by CHE-0056 (commit `a47f1aa`,
    bead `adr-fmt-spsd` closed). Same artefact serves task 2 and task
    10: the consumer-side composition contract is itself the
    trust-boundary ADR.

### Phase-2 v1 injection queue (historical)

1. **SEC-0003 ↔ `cherry-pit-web` library-surface ADR-binding gap**
   (bead `adr-fmt-spsd`). Discovered during P1-B sub-3 audit
   (`adr-fmt-3d86`, D' verify-and-close). gh-report's binary surface
   enforces SEC-0003 R1/R2/R3 at `infra/server/server::build_router`;
   cherry-pit-web's public `build_router<G, S, R>` exposes an
   un-enforced router to hypothetical future consumers. Oracle survey
   (bead `adr-fmt-js8l`) found no CHE ADR binds SEC-0003 to
   cherry-pit-web. Decision: bind SEC-0003 inside the library via new
   CHE ADR + layer wiring (requires CHE-0049 / CHE-0050 amendment), or
   document consumer-side composition as the architectural contract in
   a new CHE ADR. Resolution unblocks WS-cap mechanism work (Phase-3
   bead `adr-fmt-8qj5`). Full audit:
   `.ooda/audit-p1b-sub3-pre-dispatch-1778702408.md`.

2. **Row 14 — CHE-0024 persist-then-publish falsifier relocation to a
   CHE crate**. Discovered during Phase-2 task 6 sizing (evidence bead
   `adr-fmt-ru13`). CHE-0024 is *covered* today by
   `crates/gh-report/tests/publish_or_trace_emits_per_envelope_fields.rs`;
   the row-14 annotation flags a quality improvement (relocate the
   falsifier from gh-report to a CHE crate so it pins the contract at
   the architecture layer where CHE-0024 lives). Relocation requires
   `cherry_pit_core::testing::FakeBus` (or equivalent test-side fixture)
   which is not yet built — the present 8-task list does not commission
   it. Surface as a deferred task once cherry-pit-core gains a
   `testing` submodule (likely Phase-3 territory under harden /
   fixture-as-API work). Not blocking task 6 closure: row 14 has
   falsifier coverage today, relocation is hygiene.

3. **Row 18 sibling — extend CHE-0022 falsifier to additional concrete
   event surfaces as they emerge**. Same template as `gh-report/tests/
   event_schema_append_only.rs` (commit `53d8078`); applies whenever a
   new crate ships a concrete `DomainEvent` enum (likely
   cherry-pit-agent or cherry-pit-gateway when concrete events ship).
   No bead yet; surface when the first downstream crate adds a concrete
   event.

4. **CHE-0006 T020 + GND-0001 T015 editorial cleanup** (from Phase-1
   task 1 closure, 2026-05-14). ✅ **DONE** (2026-05-14) — discharged by
   mission `phase2-prereqs-cleanup`: CHE-0006 References trimmed
   4→3 (dropped PAR-0004, reference-only domain); GND-0001 Context
   trimmed 184→<180 words (Bungay sentence tightened). CHE-0054
   editorial sweep also folded into the same commit (References
   trimmed 20→11 by dropping 9 zero-body-mention refs; T020 still
   fires at 11 vs B-tier limit 7 — structural-scope signal tracked
   separately as bead `adr-fmt-6pvi`).

5. **cherry-pit-storage atomic lock-file claim via `persist_noclobber`**
   (bead `adr-fmt-i6nf`, P2 defensive). ✅ **DONE** (2026-05-14) —
   discharged by commit `45f9ce3` (cherry-pit-storage: defensive
   atomicity rewrite of lock primitive). 30/30 stress runs green at
   default thread count (mission `phase2-prereqs-cleanup` S1 verify).
   Originally flagged 2026-05-14
   during CI run 25863765280 triage; that CI failure resolved itself
   on the next push (`e2edc1f`, run 25863959831, all 6 jobs green) and
   the suspected TOCTOU race was not reproducible at 100 threads × 30
   stress iterations on the current `create_lock_exclusive` impl
   (`crates/cherry-pit-storage/src/lock.rs:304-316`). Re-scoped to a
   **defensive** atomicity improvement: rewrite the primitive as
   `tempfile::NamedTempFile::new_in` → `write_all` → `sync_all` →
   `persist_noclobber` (single atomic `link(2)` on Linux/macOS).
   Closes the theoretical partial-write window between `create_new` and
   `sync_all` even though it does not surface in tests today.
   Side-benefit: the empty-file arm in `acquire` (lock.rs:240-250)
   becomes truly unreachable post-rewrite and is deleted; the
   corrupt-replacement arm (lock.rs:255-269) stays — external
   corruption (covered by `corrupt_lock_file_is_replaced` at lock.rs:513)
   still reaches it. Priority dropped P0 → P2 (no observable defect).

6. **CHE-0043 amendment: reconcile flock-mandate vs CHE-0053 TTL-file
   mechanism** (bead `adr-fmt-9b4n`). Discovered 2026-05-14 during
   Mission-1 root-cause analysis. CHE-0043:R1 mandates `File::try_lock`
   (flock) but CHE-0053:R67 rejected flock in favour of TTL-file +
   atomic-rename — and the shipping implementation follows CHE-0053.
   ADR drift; CHE-0043 needs amendment to either supersede R1 or cite
   CHE-0053 as the binding mechanism. No code change.

7. **New ADR: parent-directory fsync contract for cherry-pit-storage
   lock** (bead `adr-fmt-1iy4`). Discovered 2026-05-14 reviewing
   CHE-0053:R6 against shipping `create_lock_exclusive`. Lock does not
   currently `fsync` the parent directory after `persist_noclobber`;
   adding it is a SemVer-major durability contract change and warrants
   its own CHE ADR covering rationale, OS coverage, perf cost, and
   rollout.

8. **PID-liveness check on stale-lock reclaim** (bead `adr-fmt-uwx2`).
   Discovered 2026-05-14 cataloguing latent failure modes (M4) around
   the lock primitive. `acquire()` (lock.rs:176-278) currently reclaims
   any lock whose TTL has expired; add `kill(pid, 0)` on Unix before
   reclaim so a long-paused but still-live holder is not stolen from.
   Windows: document as future (no Windows CI runner).

9. **Fault-injection harness via `fail` crate for cherry-pit-storage
   lock** (bead `adr-fmt-vbzg`). Discovered 2026-05-14 alongside item
   8; ordering note: harness lands before P2-8 so the PID-liveness work
   can reuse it. Inject between `create_new`, `write_all`, `flush`,
   `sync_all`, `persist` so adversarial tests can deterministically
   drive corrupt-file, partial-write, crash-mid-write, and
   parent-dir-unflushed scenarios.

10. **AGENTS.md: document CI exists and pre-push verify_commands**
    (bead `adr-fmt-3caa`). Discovered 2026-05-14. AGENTS.md currently
    claims "There is no CI; verification is purely local" — stale;
    `.github/workflows/ci.yml` runs `cargo test --workspace
    --all-features` plus `cargo clippy --workspace --all-targets --
    -D warnings` and `cargo fmt --check`. Update `## Commands` section
    to add a pre-push mirror so local verification anticipates CI.
    Executed inline as part of Mission 1 (one-line addition piggy-backs
    on the lock fix); bead exists for traceability.

### Phase-2 v1 lessons (historical; retained for v2 application)

- **`cargo test -p <crate> <name>` is a *function-name filter*, not a
  file target.** A `verify_commands` entry like `cargo test -p
  cherry-pit-core dep_tree` silently runs 0 tests if no `#[test] fn`
  matches "dep_tree" — and exits 0. The integration-test file is run
  via `cargo test -p <crate> --test <file_stem>`. Surfaced by hopper
  during Phase-2 task 6 sub-01 verify; subsequent sub-missions used
  the `--test <file>` form. Future mission contracts targeting an
  integration test should prefer `--test <file>` over bare-name to
  avoid 0-tests-ran false-greens.
- **Pre-existing fmt-baseline drift trap**: `crates/gh-report/src/
  infra/server/{mod.rs,server.rs}` fail workspace `cargo fmt --check`
  on HEAD `fd37133` independent of any current mission. Surfaced by
  hopper during Phase-2 task 6 sub-04. Any future mission running
  workspace-wide `cargo fmt` (vs `cargo fmt -p <crate>`) will silently
  sweep these into its diff. Candidate for a `tidy:` follow-up commit;
  or a gardener/hopper preflight assertion "fmt-baseline-clean".

---

## Phase 2 v2 — Generalization by Construction

**Intent**: Prove cherry-pit-* is general by *constructing* a second non-trivial
consumer (`adr-srv` — GraphQL over async-graphql + axum) on a fundamentally
different storage substrate (`pardosa`), then consolidating gh-report onto the
same library surface. If the cherry-pit-* traits survive two consumers + two
EventStore impls, generality is demonstrated mechanically. If they don't, the
gaps surface as code-level friction (not ADR commentary).

**Status**: Track 0 (FOCUS amendments) ratified by user 2026-05-14. Tracks 0.5
+ 1 dispatchable by moltke. Tracks 2–5 sequenced, each gated on prior + on
Track 0.5 verdict for Track 2.

**Exit when (mechanical, all in CI)**:

1. `cargo build --workspace` exit 0 with pardosa* activated as workspace members.
2. `cargo test --workspace --all-features` exit 0.
3. `cargo test --workspace --test '*_conformance'` exit 0 with **≥ 2 EventStore
   impls** (file-store + pardosa-adapter) registered.
4. `cargo run -p adr-srv` starts a GraphQL server; smoke test posts a
   `ratifyAdr` mutation and queries the result back through the projection.
5. `cargo test -p adr-srv --test lint_integration` exit 0 — metacircular
   adr-fmt-as-projection works (lint rules re-run on every event, surfaced
   via GraphQL).
6. `wc -l crates/gh-report/src/infra/server/server.rs` < 2500.
7. `cargo tree` shows **no `async-trait`** anywhere in cherry-pit-* dep trees.
8. `cargo run -p adr-fmt -- --lint` warnings-only, no errors (baseline preserved).
9. Bead `adr-fmt-spsd` closed with code reference, not text deferral.
10. **gh-report SMI landed** (Track 4.0). All of:
    - `rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/`
      returns zero hits.
    - `rg -n 'EventStore' crates/gh-report/src/` shows write-side use confined
      to the `Merger` module.
    - New regression test `crates/gh-report/tests/smi_replay_equivalence.rs`
      loads a pre-SMI msgpack event log (captured in-tree) and asserts
      projection final-state byte-equivalence to the baseline.
    - Sweep audit trail preserved: `Run` event variants
      (`SweepStarted/Progress/Completed/Failed`, `EvidencePublished`) still
      emitted; projection sweep-history fields unchanged.

No checklist item ("orphan count = 0", "every invariant has a falsifier") that
is dischargeable by editing an ADR file. v1's ceremony pattern is structurally
prevented.

### Out of scope (retained for Phase 3)

- NATS in production deployments of adr-srv or gh-report (tests-only is enough).
- SEC-0010 (NATS TLS) — Phase 3.
- SEC-0011 (tamper-evident hash-chain logs) — Phase 3. PAR-0021 frontier hash
  may be surfaced via opt-in trait extension per Track 0.5 verdict; full
  SEC-0011 contract is Phase 3.
- Adversarial-input fuzz harnesses — Phase 3 tasks 2 / 3.
- TLA+ / Smithy specs — Phase 3 tasks 9 / 10.
- CHE-0044 object_store backend — Phase 3 review.
- crates.io publication — separate concern.

### Track 0 — Ratification gate (complete on user-ratification 2026-05-14)

FOCUS.md §3 / §7 / §8 amendments per `.ooda/focus-amendment-phase2-v2-draft.md`.
Ratification unblocks all subsequent tracks. (Track 0 is administrative — no
code, no bd mission.)

### Track 0.5 — Pardosa research (gate before Track 2)

Goal: produce a written conclusion, ratified by the user, that answers three
integration questions and surveys prior art before any pardosa-wrapping code
is written. Read-only. No code.

**Why a separate track**: gap analysis surfaced real model mismatches between
`pardosa` and `cherry-pit-core` (purged-state ↔ aggregate lifecycle, identity
model, correlation/causation propagation). Wrapping pardosa in `EventStore`
without resolving these first repeats the construction-phase mistake — code
that compiles but doesn't honour either side's invariants. The research output
may recommend CHE ADR amendment(s), which are themselves §6 high-risk
decisions requiring user ratification before Track 2.1.

**Q1 — Purged state ↔ Aggregate lifecycle**

Pardosa has `FiberState::Purged` (`crates/pardosa/src/fiber_state.rs:11`) with
ID-reuse semantics (`Purged → Defined` transition; `purged_ids: HashSet<DomainId>`
in `dragline.rs:38`). cherry-pit-core CHE-0023 lifecycle has no Purged variant;
CHE-0011 `AggregateId` is `NonZeroU64` (infrastructure-owned per CHE-0020);
ID-reuse-after-termination is unspecified.

- Q1.1: Does CHE-0023 allow a Terminated aggregate's id to be re-issued?
- Q1.2: Does Pardosa's `Purged → Defined` round-trip preserve the precursor
  chain (PAR-0012) or break it?
- Q1.3: When pardosa-store implements `EventStore::load`, what is the response
  for a domain_id whose fiber is currently `Purged` — empty (per CHE-0019,
  losing audit visibility) or historical events?

Decision branches: **A** reject Purged ops at adapter (simple, loses pardosa
capability) / **B** extend cherry-pit lifecycle with Purged or `Reusable`
marker — CHE-0023 amendment (most general, widest blast radius) / **C** split
trait surface: `EventStore` + extension `PurgeableEventStore` (likely default
recommendation; preserves both sides).

**Q2 — Aggregate identity model ↔ Pardosa DomainId**

Pardosa `DomainId(u64)` (zero allowed; `event.rs:102`) vs cherry-pit
`AggregateId(NonZeroU64)` per CHE-0011. Pardosa `event_id: u64` monotonic per
domain (PAR-0007) vs cherry-pit `event_id: Uuid` v7 (CHE-0033). PAR-0021
frontier hash + per-fiber hash chain has no cherry-pit analogue.

- Q2.1: AggregateId↔DomainId mapping (forward trivial; reverse must reject 0).
- Q2.2: Can the adapter generate UUIDv7 envelope ids deterministically from
  `(domain_id, event_id)` without breaking PAR-0007's monotonic-per-domain
  ordering or CHE-0033's monotonic-global ordering?
- Q2.3: Surface pardosa's hash chain (PAR-0021) via opt-in extension trait, or
  hide it and let SEC-0011 re-add it in Phase 3?

Decision branches: **A** hide hash chain, SEC-0011 re-adds Phase 3 (simplest;
strength wasted) / **B** opt-in `HashChainedEventStore` extension (likely
default; SEC-0011 head-start) / **C** bind into core `EventStore` (forces
file-store to implement; biggest ripple to Track 4).

**Q3 — Correlation + causation in EventBus integration**

cherry-pit `EventEnvelope::{correlation_id, causation_id}: Option<Uuid>` per
CHE-0016 / CHE-0039. Pardosa `Event<T>` has neither — only `precursor: Index`
(per-fiber parent pointer; `event.rs:194`) and `detached: bool` (`event.rs:189`).
PAR-0017 introduces a state-machine bus that may conflict with `cherry-pit-core::EventBus`
+ CHE-0024 persist-then-publish.

- Q3.1: Compute `causation_id: Uuid` from `(domain_id, precursor)` at
  envelope-emit time so the cherry-pit DAG is faithful even though pardosa
  stores it as a chain?
- Q3.2: Where does `correlation_id` live? (a) envelope-wraps-pardosa-event
  (likely default; verify PAR-0006 genome serialization survives) / (b) extend
  `Event<T>` (breaks PAR-0003 immutability+non_exhaustive) / (c) sidecar table.
- Q3.3: Semantic of pardosa `detached: bool` events on the cherry-pit side —
  saga compensation (CHE-0040)? policy-emitted? administrative?
- Q3.4: Does PAR-0017 state-machine bus compose with cherry-pit EventBus
  (CHE-0024 persist-then-publish, publication non-fatal), or replace it when
  pardosa is the substrate?

Decision branches: **A** envelope-around-event, pardosa bus hidden (simplest;
cherry-pit EventBus canonical) / **B** sidecar table for correlation/causation
(two-write atomicity story needed) / **C** pardosa-bus replaces cherry-pit-bus
when pardosa is the substrate (most aggressive; risks divergent CHE-0024
semantics).

**Prior-art survey** (read first, decide second):

| Project | Why |
|---|---|
| **EventStoreDB** / Kurrent | Canonical single-writer-per-stream EDA store; `$purged` stream-deletion + soft/hard delete distinction |
| **Marten** (PostgreSQL, .NET) | Single-writer per stream; archival semantics; documented metadata model |
| **Axon Framework** (Java/Kotlin) | First-class aggregate lifecycle + replay; ID reuse policy |
| **eventsourcing** (Rust) + **disintegrate** (Rust) | Closest in-language prior art; trait shapes |
| **NATS JetStream + KV** | Substrate pardosa references (PAR-0013, PAR-0019, PAR-0020); what NATS provides vs what pardosa adds |
| **Kafka Streams + KTables** | Adversarial — log-compaction + tombstones is Q1 in different shape |

Survey output: 1-table-per-target summary in the research artefact, rows =
Q1/Q2/Q3, cells = how that project answers + citation.

**Deliverables**: single artefact `.ooda/pardosa-research-<ts>.md` (evidence
bead, label `evidence` + `mission:phase2-v2-pardosa-research`) with sections:

1. Summary (5 lines: leading recommendation per Q1/Q2/Q3 + rollup verdict).
2. Gap analysis (concrete file:line citations expanded from this section).
3. Prior-art table (six targets × three questions).
4. Three decision matrices (Q1/Q2/Q3): options × cost × reversibility ×
   ADR-amendment cost × operational risk × pardosa-capability preserved.
   Per-option note: does the option preserve a **single-writer-friendly**
   trait shape? gh-report's Track 4.0 SMI refactor reveals gh-report's
   actual usage is single-writer-per-process; if a `SingleWriterEventStore`
   capability marker (orthogonal to `HashChainedEventStore` from Q2 and
   the correlation/causation strategy from Q3) emerges as natural,
   recommend its inclusion in Track 2.2 adapter scope. Observational
   input only — not a Track 0.5 mandate.
5. Recommendation per question (explicit choice + 2-sentence rationale).
6. Rollup verdict, one of:
   - **Proceed**: pardosa wraps cleanly; Track 2 starts as planned.
   - **Proceed-with-amendment**: pardosa wraps after specified CHE ADR
     amendment(s); enumerate them — user ratifies each before Track 2.1.
   - **Re-scope**: pardosa not viable as Phase-2 second EventStore;
     fall back to in-memory + sqlite; §3 amendment A withdrawn.
   - **Abandon**: stop; re-orient.
7. Open questions for the user, batched at the end, each with a recommended
   default (per AGENTS.md autonomy rule).

**Exit gate**: user reads artefact, ratifies rollup verdict + any recommended
CHE ADR amendments. Only then Track 2 opens.

**Dispatch shape (plan-mode)**: copernicus × 3 (one per Q1/Q2/Q3) + copernicus
× 1 (prior art) + oracle × 1 (ADR-binding constraints for cherry-pit-core
{Aggregate, EventStore, EventBus} + PAR-0001..PAR-0023 + CHE-0011/0012/0016/0020/0023/0024/0039)
+ feynman × 1 (option ranking, falsifiers) + moltke × 1 (synthesise verdict,
draft ratification batch). No hopper involvement.

**Not in Track 0.5**: writing pardosa-store code (Track 2.2); drafting CHE
ADR amendments (only if research recommends + user ratifies — separate
amendment mission before Track 2); editing pardosa source (Track 2.1 territory);
touching FOCUS.md further.

### Track 1 — Foundations (no external deps; may run parallel to Track 0.5)

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 1.1 | Fill `cherry-pit-projection` | Move `FileProjectionStore`, `InMemoryProjection`, `ProjectionDriver` out of `cherry-pit-agent` + `gh-report` into the dedicated crate. Public surface flat per CHE-0030. | `cargo test -p cherry-pit-projection`; gh-report + cherry-pit-agent still build + tests pass; `cargo run -p adr-fmt -- --context cherry-pit-projection` exit 0. |
| 1.2 | `cherry-pit-core::testing` module | Add `FakeBus`, `InMemoryEventStore`, `InMemoryProjectionStore`. Behind `#[cfg(any(test, feature = "testing"))]`. Pure sync, no I/O — CHE-0018:R3 + CHE-0029:R4 still binding. | `cargo test -p cherry-pit-core --features testing`; new fixture types appear in `--context` output. |
| 1.3 | Trait-conformance test harness | One test file per trait (`event_store_conformance.rs`, `aggregate_conformance.rs`, `projection_conformance.rs`) parameterised on impl. Defines invariants every impl MUST satisfy (append-only, replay equivalence, idempotency-key uniqueness, etc.). Invoked from each impl crate's tests. | `cargo test --workspace --test '*_conformance'` exit 0 with only one impl today (file-store); ready for second impl. |
| 1.4 | RPITIT audit | Audit every `async fn` / `async_trait` in cherry-pit-* public surfaces. Convert to `-> impl Future + Send + 'a` per CHE-0025. Drop `async-trait` dep. | `cargo tree` shows no `async-trait` in cherry-pit-* dep trees; `cargo test --workspace`. |

**Checkpoint**: workspace builds; conformance tests green for one impl; no `async-trait`.

### Track 2 — Pardosa as second EventStore (gated on Tracks 0.5 + 1)

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 2.1 | Activate pardosa* as workspace members | Add `pardosa`, `pardosa-genome`, `pardosa-derive` to `Cargo.toml` `members`. Reconcile MSRV / edition / lints. Update `adr-fmt.toml` PAR/GEN domain `crates = [...]` so `--context pardosa` keeps working. | `cargo build --workspace`; `cargo run -p adr-fmt -- --tree PAR`; `cargo run -p adr-fmt -- --context pardosa` exit 0. |
| 2.2 | `PardosaEventStore` adapter | New crate (or module within cherry-pit-gateway per CHE-0029 layering) wrapping pardosa's fiber/dragline as `cherry_pit_core::EventStore` per the Track 0.5 verdict (Q1/Q2/Q3 recommendations). Local fiber storage only in this sub-mission — no NATS. | Trait-conformance harness from 1.3 runs and passes against `PardosaEventStore`. **Load-bearing exit signal for Track 2.** |
| 2.3 | NATS substrate — tests only | Embedded `nats-server` test fixture. Pardosa publish-then-apply (PAR-0008) flows over NATS in tests. Feature-gated for adr-srv Track-3 use; production deploy still local fibers. | `cargo test -p <pardosa-store-crate> --features nats` exit 0; CI workflow installs nats-server binary. |

**Checkpoint**: two impls behind one trait, both green on conformance harness. Generality mechanically falsifiable.

### Track 3 — adr-srv (gated on Tracks 1 + 2)

Goal: the second real consumer. Read + write + projection drives adr-fmt's lint output.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 3.1 | adr-fmt library surface | Extract `crates/adr-fmt-core` (or expose `crates/adr-fmt` as lib+bin). Expose `parser`, `model`, `rules::{template, links, naming}`, `containment`, `nav` as public lib API. Binary thin wrapper. Frozen CLI per AFM-0001 unchanged. | Existing `cargo test -p adr-fmt` still green; `adr-srv` can `use adr_fmt_core::Diagnostic`. |
| 3.2 | adr-srv crate skeleton | New `crates/adr-srv`. axum + async-graphql. Aggregate = `AdrDocument`; events = `Drafted` / `Ratified` / `Superseded` / `Retired`. Commands NOT serializable per CHE-0014. EventStore = `PardosaEventStore`. | `cargo build -p adr-srv`; `cargo test -p adr-srv` (skeleton tests green). |
| 3.3 | GraphQL read schema + projection | Query types over `Projection` of `AdrDocument`. Surface mirrors `adr-fmt --tree` / `--refs` / `--context`. Projection driven by `cherry-pit-projection` (Track 1.1). | `cargo test -p adr-srv --test graphql_read_e2e`; spawn server, `{ adr(id: "AFM-0001") { title, references { id } } }`, assert shape. |
| 3.4 | GraphQL mutations | Mutation types map to commands via `cherry-pit-web::CommandRouter`. `ratifyAdr(id)` / `supersede(old, new)`. Persist via PardosaEventStore, project via Track 1.1. | `cargo test -p adr-srv --test graphql_write_e2e`; mutation → event → projection visible in next query. |
| 3.5 | Projection-driven adr-fmt integration | adr-srv's projection re-runs adr-fmt's lint rules on every event; output surfaced via `{ lint { diagnostics { id, severity, ... } } }`. Closes the metacircular loop. | `cargo test -p adr-srv --test lint_integration`; introduce a synthetic L0xx-violating ADR via mutation, assert diagnostic appears in query. |

**Checkpoint**: adr-srv works end-to-end on pardosa + cherry-pit-projection + adr-fmt-as-lib. **Generality claim load-bearing.**

### Track 4 — gh-report consolidation (gated on Tracks 1 + 3)

Goal: shrink `gh-report::server.rs` (3850 LOC) by consuming `cherry-pit-web` the way adr-srv does. Surfaces every "general but not really" gap.

Track-4 internal order: **4.0 (SMI) → 4.1 (router diff) → 4.2 (push upstream) → 4.3 (migrate + LOC gate)**. 4.0 lands first so the router diff in 4.1 sees post-SMI gh-report; the LOC gate at 4.3 absorbs SMI deletions.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 4.0 | gh-report SMI refactor | Declare the **Serial Merge Invariant** (SMI): exactly one task (`Merger`) holds the `EventStore` write handle within a gh-report process. Collapse `RunService` + `RepoService` + `WebhookService` write paths onto a single `mpsc::Sender<MergerCommand>`. Webhook handler funnels through the same channel (no second writer path). Aggregate `handle()` runs inside the merger against in-memory state; no load-before-append, no CAS, no per-service sequence-tracker maps. Delete `run_index` / `repo_index` / `delivery_index` from `AppState` (become plain `HashMap` fields owned by the merger task). On-disk msgpack event format unchanged — pre-SMI event logs replay byte-identically. All 8 `DomainEvent` variants retained; sweep history audit preserved in projection per FOCUS §3 audit constraint. ApplicationService public APIs become thin channel-send wrappers (call-sites in `collect.rs` and `webhook/mod.rs` do not move). Closes I1 TOCTOU structurally. | `cargo test -p gh-report --workspace`; `cargo test -p gh-report --test smi_replay_equivalence` (new regression test); `rg -n 'sequence_tracker\|run_index\|repo_index\|delivery_index' crates/gh-report/src/` → zero hits; `rg -n 'EventStore' crates/gh-report/src/` → write-side use confined to `Merger` module. |
| 4.1 | Diff gh-report router vs adr-srv router | Read-only analysis sub-mission. `.ooda/gh-report-cherry-pit-web-gap.md`: every line of `infra/server/server.rs` not present in `cherry-pit-web::build_router` is either (a) reusable upstream, (b) gh-report-specific composition (acceptable), or (c) duplicated logic (must consolidate). | Artefact registered as evidence bead. Inventory drives 4.2. |
| 4.2 | Push category (a) into cherry-pit-web | Each reusable layer becomes part of cherry-pit-web's public composition. cherry-pit-web's tests grow to cover them. | `cargo test -p cherry-pit-web`; `cargo test -p gh-report`. |
| 4.3 | Migrate gh-report onto consolidated cherry-pit-web | Delete duplicated logic. `wc -l server.rs < 2500` (exit-criterion gate per §5). | LOC gate hits; full gh-report integration tests green. |

**Checkpoint**: gh-report thinner; cherry-pit-web provably richer because two consumers shaped it.

### Track 5 — SEC-0003 bind-in-library (gated on Track 4)

Goal: discharge `adr-fmt-spsd` with code, not another ADR like CHE-0056. Track 4's consolidation forces the question — adr-srv + gh-report both need SEC-0003 R1/R2/R3 and should not each re-implement it.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 5.1 | Pick mechanism with evidence in hand | Two consumers exist; pick: (a) bind layers inside `cherry-pit-web::build_router` with a `SecurityPosture` parameter; (b) type-state builder that consumers MUST close. Decision driven by Track 4 diff, not speculation. | Brief ADR amendment (supersede CHE-0056 or new CHE) backed by referenced code lines. |
| 5.2 | Implement chosen mechanism | Both adr-srv + gh-report use library-level enforcement; bead `adr-fmt-spsd` closes. | Both apps green; integration test asserts SEC-0003 R1/R2/R3 enforced from library (e.g. compile error in posture (b) or correct defaults in (a)). |

### Phase 2 v2 sequencing

```
Track 0 (Ratification: §3 FOCUS amendments)
    │ user ratifies amendments
    ▼
Track 0.5 (Pardosa research, read-only)
    │ research artefact → user ratifies verdict
    │  ├─ "Re-scope" → redesign Track 2 (in-memory + sqlite fallback)
    │  └─ "Proceed-with-amendment" → CHE amendment mission → user ratifies
    ▼
Track 1 (Foundations) ── parallel-able with 0.5 once Track 0 ratified
    (1.1 projection / 1.2 testing fixtures / 1.3 conformance / 1.4 RPITIT)
    ▼
Track 2 (Pardosa as 2nd EventStore) ── gated on 0.5 verdict + 1.3
    ▼
Track 3 (adr-srv) ── gated on 1 + 2
    ▼
Track 4 (gh-report consolidation) ── gated on 1 + 3
    ▼
Track 5 (SEC-0003 bind-in-library) ── gated on 4
```

### Phase 2 v2 risk register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| pardosa MSRV / edition mismatch on workspace activation | M | M | Track 2.1 fixes inline; >1 day blocker → escalate. |
| Pardosa frontier-hash + precursor-chain model doesn't fit `EventStore` cleanly | M | H | Track 0.5 surfaces this *before* code. Conformance harness (1.3) lands FIRST. If pardosa-as-EventStore needs trait surface changes, that **is** Phase 2 work — goal is to generalize the trait, not protect it. |
| async-graphql + cherry-pit-web composition gap | M | M | Spike at start of 3.2; if hostile, drop async-graphql for axum-only POST handler. User notified before re-scope. |
| adr-fmt library extraction breaks current binary | L | H | Track 3.1 is an internal-refactor; existing tests cover binary surface. Run `cargo test -p adr-fmt` after every step. |
| Track 4 reveals gh-report needs cherry-pit-web features that conflict with adr-srv's needs | M | M | This is the point — surface in `.ooda/gh-report-cherry-pit-web-gap.md` evidence, decide before coding. Halt-and-handback if conflict implies CHE-0049 / CHE-0050 amendment. |
| Embedded nats-server unavailable in CI | L | M | `async-nats` in-memory shim; if absent, install nats-server in CI workflow. |
| Track 0.5 concludes "Re-scope" (pardosa not viable) | M | M | Fallback: in-memory + sqlite as second impl. FOCUS §3 amendment A partially withdrawn. No code wasted — 0.5 is read-only. |
| Track 0.5 recommends multiple CHE amendments | M | M | Each is its own §6 high-risk decision the user ratifies. moltke batches them in one ratification round. |
| Scope creep ("while we're at it…") | H | M | Strict track boundaries; injection queue for discovered work; gardener pass between tracks. |
| gh-report SMI (Track 4.0) replay-equivalence test fails — pre-SMI event log produces divergent post-projection state | M | H | Halt-and-handback per FOCUS abort criteria. Likely a fold-order bug in the in-memory aggregate-state path or a missed event variant in the merger's routing. Bisect via feynman. Do not commit a partial SMI; on-disk format is contractually unchanged, so partial rollout would silently corrupt audit trail equivalence. |

**Pre-mortem worst case**: Track 2.2 reveals the `EventStore` trait can't
accommodate pardosa without breaking existing file-store consumers. **Response**:
this is itself a Phase-2 win (generality gap found by construction). Either
widen the trait with an optional capability (RPITIT-friendly) or accept that
pardosa-store wraps a stricter sub-trait. Document via ADR amendment + commit.
Do **not** revert.

**Abort criteria**: if Track 2 cannot reach conformance-green within ~3 hopper
missions, halt and re-orient via feynman. Maybe pardosa is incompatible in
principle; fall back to in-memory + sqlite as second impl, notify user.

### Phase 2 v2 injection queue

(Inherits all v1-injection items 1–10 above that remain unresolved; they
remain Phase-2 in nature. New discoveries during v2 land here with bead
labels `phase:2-generalize`.)

---

## Phase 3 — Harden

**Intent**: Correctness under stress; withstanding errors; adversarial
behaviour on interfaces. **Not** publication-prep.

**Exit when**:
- All Phase-3 tasks closed
- Fuzz and property suites green on adversarial inputs
- Invariants verified by formal specs (TLA+) and contract specs (Smithy)
  agree with implementation

### Tasks

**Interface trust boundaries (adversarial-input)**

1. Wire `gh-report` webhook trust-boundary validation (SEC-0002 R1–R3:
   signature verification, replay protection, request size caps).
2. Adversarial-input fuzz harness for `cherry-pit-web` HTTP surface
   (malformed bodies, oversize headers, slow-loris, encoding tricks).
3. Adversarial-input fuzz harness for `cherry-pit-gateway` event-decode
   surface (corrupt msgpack frames, truncated streams, mis-typed
   discriminants).
4. Webhook signature-verification negative tests (wrong secret, tampered
   payload, missing header, timing-attack resistance).

**Error-path correctness**

5. Error-path property tests for `cherry-pit-projection` and
   `cherry-pit-gateway` file-store (store failures, partial reads,
   disk-full, permission-denied, concurrent open, fsync failure, torn
   writes).
6. Resource-bound enforcement tests across `cherry-pit-web` router and
   `cherry-pit-wq` execution (concurrent-connection / body-size /
   header-count / max-concurrent-job / timeout / panic-isolation limits
   actually enforced).
7. Error-propagation audit: every public-surface `Result` chain
   preserves enough context for the caller to act.

**Invariant correctness under stress**

8. CHE-0024 (persist-then-publish) failure-mode tests + CHE-0006
   (single-writer) concurrent-command tests + CHE-0022 (append-only)
   in-place-mutation rejection tests.

**Formal specifications**

9. Smithy contract models for `gh-report` webhook ingress,
   `cherry-pit-web` projection-router API, and `cherry-pit` event-envelope
   shape (`specs/smithy/`); validation harness wired into ingress paths.
10. TLA+ specifications for the load-bearing temporal invariants
    (`specs/tla/`); TLC pass; counter-examples become failing tests.
    *(Scope and tool details — PlusCal vs raw TLA+, which invariants —
    decided at task activation.)*

**Security ADR closure**

11. Resolve SEC-0010 (Transport Security / NATS TLS) and SEC-0011
    (Tamper-Evident Logs / hash-chain): elevate to Accepted with
    implementation citation, or retire with rationale. Coupled to
    CHE-0044 / Pardosa deferral disposition.
12. Draft new CHE ADR for secret isolation (per SEC-0007).

### Phase-3 injection queue

1. **WS connection cap mechanism for `cherry-pit-web`** (bead
   `adr-fmt-8qj5`, SEC-0003 R2). Deferred from P1-B sub-mission 3
   (`adr-fmt-3d86`). Copernicus survey (evidence bead `adr-fmt-2k3x`,
   `.ooda/observations-p1b-sub3-layers-1778702556.md`) confirmed at
   pinned versions (axum 0.8.9, tower 0.5.3, tower-http 0.6.10) that
   no stock held-open-socket cap exists; `ConcurrencyLimitLayer` caps
   in-flight upgrade rate, not held sockets. Three candidate
   mechanisms enumerated in surprise artefact
   `.ooda/surprise-p1b-sub3-1778699612.md`. Decision requires oracle
   orient on `cherry-pit-web` public-API surface (CHE-0049:R1 +
   CHE-0050:R2). Vacuous under default features per CHE-0049:R3+R11.

2. **Adversarial-input gap inventory for cherry-pit-storage lock**
   (bead `adr-fmt-htyk`). Discovered 2026-05-14 cataloguing latent
   failure modes (M7) during Phase-2 P2-5 root-cause work. Enumerate
   adversarial inputs the lock primitive does not yet defend against
   (oversized PID, malformed UTF-8 in lockfile, symlink races on the
   lockfile path, etc); informational checklist that defers actual
   harness/fuzz work to existing Phase-3 task 5 (file-store error-path
   property tests).

---

## Injection log

Cross-phase discovery audit trail. Blockers execute inline; non-blockers
queue with `phase:N-<name>` bead label for next phase sweep.

| Date | Discovered during | Routed to | Bead | Reason | Blocker? |
|------|-------------------|-----------|------|--------|----------|
| 2026-05-13 | P1-B sub-3 router-limits orient (Option D selected) | phase-3 | `adr-fmt-8qj5` | WS held-open-socket cap has no stock layer at pinned versions; architectural decision deferred. | no |
| 2026-05-13 | P1-B sub-3 pre-dispatch audit (D' verify-and-close) | phase-2 | `adr-fmt-spsd` | SEC-0003 obligations enforced at gh-report binary surface; cherry-pit-web library surface has no CHE ADR binding — decide bind-in-library vs consumer-side-contract. | no |
| 2026-05-14 | CI run 25863765280 triage (premise: `concurrent_acquire_exactly_one_wins` flake) | phase-2 inline (Mission 1B) | `adr-fmt-i6nf` | Original triage hypothesised a partial-write TOCTOU window in `create_lock_exclusive`. Premise invalidated: HEAD `e2edc1f` CI green (run 25863959831), race not reproducible at 100×30 stress. Re-scoped to defensive atomicity rewrite (`tempfile::NamedTempFile::persist_noclobber`) — independent soundness improvement, no observable defect. Priority P0 → P2. | no |
| 2026-05-14 | Mission-1 latent-failure inventory (M2/M4/M7, ADR drift) | phase-2 + phase-3 | `adr-fmt-9b4n`, `adr-fmt-1iy4`, `adr-fmt-uwx2`, `adr-fmt-vbzg`, `adr-fmt-3caa`, `adr-fmt-htyk` | Five P2 + one P3 follow-ups: CHE-0043↔CHE-0053 drift; parent-dir fsync ADR; PID-liveness on stale reclaim; fault-injection harness; AGENTS.md CI doc (executed inline with Mission 1); adversarial-input inventory. | no |

---

## Revision history

| Version | Date       | Changes |
|---------|------------|---------|
| 0.1     | 2026-05-13 | Initial; per-axis detail blocks lifted from `.ooda/refinement-roadmap-draft.md`. |
| 0.2     | 2026-05-13 | Restructured to high-level ordered task lists. Ceremony stripped from all phases (C4 doc refreshes, CHANGELOG, MSRV declaration, semver docs, license-header audit, docs.rs metadata, crates.io publication actions removed). Phase 3 reframed: correctness + error-withstanding + adversarial-input hardening, not publication-prep. Phase 3 gains Smithy contract models and TLA+ specifications (details deferred to task activation). Axis J (perf/energy) and Publication-prep removed. |
| 0.3     | 2026-05-14 | Phase 2 superseded by Phase 2 v2 (Generalization by Construction). v1 declared exit on ceremony for 6/10 tasks (ADR text shuffling counted as discharge for T2/T4/T5/T7/T8/T9/T10; cherry-pit-agent + cherry-pit-web circularly counted as "≥2 worked examples"; only T6 falsifier tests + injection-queue storage work were load-bearing). v1 task closures retained for audit; v2 layers in 5 tracks (0 Ratification → 0.5 Pardosa research → 1 Foundations → 2 Pardosa as 2nd EventStore → 3 adr-srv → 4 gh-report consolidation → 5 SEC-0003 bind-in-library) with mechanical CI-verifiable exit criteria. Track 0.5 (Pardosa research) prepended at user request: gap analysis surfaced model mismatches (Purged state ↔ Aggregate lifecycle; DomainId↔AggregateId identity; correlation/causation propagation in EventBus) and prior-art survey (EventStoreDB / Marten / Axon / Rust crates / NATS / Kafka) required before any pardosa-wrapping code. FOCUS.md §3/§7/§8 amendment draft at `.ooda/focus-amendment-phase2-v2-draft.md` for user ratification. |
| 0.4     | 2026-05-15 | gh-report **Serial Merge Invariant (SMI)** refactor added as **Track 4.0** (single-writer merger; collapses three-service write coordination; webhook funnels through same merger as sweeps; on-disk msgpack format unchanged; audit trail preserved in projection). Promoted to **mechanical Phase 2 v2 exit criterion #10** (rg checks + replay-equivalence regression test). **Track 0.5 deliverables §4** gains a per-option callout asking whether each Q1/Q2/Q3 decision preserves a single-writer-friendly trait shape (`SingleWriterEventStore` capability marker as potential factoring; observational input, not mandate). **Risk register** gains replay-equivalence-regression row. **Track-4 internal order** documented as 4.0 → 4.1 → 4.2 → 4.3 so the router diff sees post-SMI gh-report. No new track. No §3 FOCUS amendment required. **Sequencing unchanged** — Track 4 remains gated on Tracks 1 + 3. Discovery origin: plan-mode session 2026-05-15 reading gh-report write path; named invariants (SMI, job-queue regenerability, pure-worker, append-or-reject, post-append publish) documented on the injection bead. Companion: `FOCUS.md` v0.5 (§3 audit-trail entry + §8 Phase-2 v2 verify additions for SMI exit gate). |
