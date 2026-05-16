# Cherry-Pit Refinement Roadmap

**Status**: Live (Phase 2 v2 — remaining tracks: 3, 4.4, 5)
**Governs**: Phases 2–3 of the architectural refinement phase
**Companion to**: `FOCUS.md` §4 (this document is FOCUS.md's §4 detail)
**Reader**: moltke decomposing into hopper missions; user reviewing progress

---

## How to read this document

Live operational view of the 3-phase roadmap declared in `FOCUS.md` §4.
Each phase lists ordered, high-level tasks. Tasks discovered mid-phase
that match another phase's nature inject into the phase matching their
nature (cleanup → 1, generalize → 2, harden → 3).

`FOCUS.md` is the recipe (stable); this is the dashboard.

Closed-task and closed-track history lives in bd
(`bd query --label phase:1-cleanup,phase:2-generalize,phase:3-harden`)
and git log; this file carries only forward work.

Bead labels: `phase:1-cleanup` | `phase:2-generalize` | `phase:3-harden`.

---

## Phase state

Phase 1 (Cleanup) completed 2026-05-14. Phase 2 v1 (Generalize)
superseded by v2 on 2026-05-14.

| Phase | Status | Remaining |
|-------|--------|-----------|
| 2 — Generalize v2 | active | Tracks 3 + 4.4 + 5 |
| 3 — Harden       | not started | 12 tasks (+1 injection) |

---

## Phase 1 — Cleanup (closed 2026-05-14)

Exit criteria met: workspace builds and tests green, lint warnings-only,
no donor crates in tree. Task-level audit in bd under closed beads with
label `phase:1-cleanup`.

---

## Phase 2 v1 — Generalize (superseded 2026-05-14)

Superseded by Phase 2 v2 (ceremony-vs-substance review). Task closures
retained in bd under `phase:2-generalize`. Operational lessons
(`cargo test --test <file_stem>` vs bare-name filter; pre-existing
`gh-report` fmt-baseline drift) migrated to `AGENTS.md § Commands`
"Verify-command gotchas".

---

## Phase 2 v2 — Generalization by Construction

**Intent**: Prove cherry-pit-* is general by *constructing* a second non-trivial
consumer (`adr-srv` — GraphQL over async-graphql + axum) on a fundamentally
different storage substrate (`pardosa`), then consolidating gh-report onto the
same library surface. If the cherry-pit-* traits survive two consumers + two
EventStore impls, generality is demonstrated mechanically. If they don't, the
gaps surface as code-level friction (not ADR commentary).

**Status**: Tracks 0, 0.5, 1, 2, 4 closed. Tracks 3, 4.4, 5 remaining.

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
6. `cargo tree` shows **no `async-trait`** anywhere in cherry-pit-* dep trees.
7. `cargo run -p adr-fmt -- --lint` warnings-only, no errors (baseline preserved).
8. Bead `adr-fmt-spsd` closed with code reference, not text deferral.
9. SMI maintained (Track 4.0 closed 2026-05-16):
   - `rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/`
     returns zero hits.
   - `rg -n 'EventStore' crates/gh-report/src/` shows write-side use confined
     to the `Merger` module.
   - `cargo test -p gh-report --test smi_replay_equivalence` exit 0.

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

### Track 3 — adr-srv (gated on Tracks 1 + 2; both closed → dispatchable)

Goal: the second real consumer. Read + write + projection drives adr-fmt's lint output.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 3.1 | adr-fmt library surface | Extract `crates/adr-fmt-core` (or expose `crates/adr-fmt` as lib+bin). Expose `parser`, `model`, `rules::{template, links, naming}`, `containment`, `nav` as public lib API. Binary thin wrapper. Frozen CLI per AFM-0001 unchanged. | Existing `cargo test -p adr-fmt` still green; `adr-srv` can `use adr_fmt_core::Diagnostic`. |
| 3.2 | adr-srv crate skeleton | New `crates/adr-srv`. axum + async-graphql. Aggregate = `AdrDocument`; events = `Drafted` / `Ratified` / `Superseded` / `Retired`. Commands NOT serializable per CHE-0014. EventStore = `PardosaEventStore`. | `cargo build -p adr-srv`; `cargo test -p adr-srv` (skeleton tests green). |
| 3.3 | GraphQL read schema + projection | Query types over `Projection` of `AdrDocument`. Surface mirrors `adr-fmt --tree` / `--refs` / `--context`. Projection driven by `cherry-pit-projection` (Track 1.1). | `cargo test -p adr-srv --test graphql_read_e2e`; spawn server, `{ adr(id: "AFM-0001") { title, references { id } } }`, assert shape. |
| 3.4 | GraphQL mutations | Mutation types map to commands via `cherry-pit-web::CommandRouter`. `ratifyAdr(id)` / `supersede(old, new)`. Persist via PardosaEventStore, project via Track 1.1. | `cargo test -p adr-srv --test graphql_write_e2e`; mutation → event → projection visible in next query. |
| 3.5 | Projection-driven adr-fmt integration | adr-srv's projection re-runs adr-fmt's lint rules on every event; output surfaced via `{ lint { diagnostics { id, severity, ... } } }`. Closes the metacircular loop. | `cargo test -p adr-srv --test lint_integration`; introduce a synthetic L0xx-violating ADR via mutation, assert diagnostic appears in query. |

**Checkpoint**: adr-srv works end-to-end on pardosa + cherry-pit-projection + adr-fmt-as-lib. **Generality claim load-bearing.**

### Track 4.4 — validate.rs migration to cherry-pit-web (gated on Track 3)

Split out from closed Track 4 per moltke scope-decision A (2026-05-16).
Migrate gh-report's `infra/validate.rs` request-validation logic into
`cherry-pit-web` so adr-srv reuses it.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 4.4 | validate.rs migration | Lift gh-report's validation surface into a `cherry-pit-web` module; both consumers compose it. | `cargo test -p cherry-pit-web`; `cargo test -p gh-report`; adr-srv consumes the same validation surface in Track 3 follow-up. |

### Track 5 — SEC-0003 bind-in-library (gated on Track 4.4)

Goal: discharge `adr-fmt-spsd` with code, not another ADR like CHE-0056. Track 4's consolidation forces the question — adr-srv + gh-report both need SEC-0003 R1/R2/R3 and should not each re-implement it.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 5.1 | Pick mechanism with evidence in hand | Two consumers exist; pick: (a) bind layers inside `cherry-pit-web::build_router` with a `SecurityPosture` parameter; (b) type-state builder that consumers MUST close. Decision driven by Track 4 diff, not speculation. | Brief ADR amendment (supersede CHE-0056 or new CHE) backed by referenced code lines. |
| 5.2 | Implement chosen mechanism | Both adr-srv + gh-report use library-level enforcement; bead `adr-fmt-spsd` closes. | Both apps green; integration test asserts SEC-0003 R1/R2/R3 enforced from library (e.g. compile error in posture (b) or correct defaults in (a)). |

### Phase 2 v2 sequencing (remaining)

```
Track 3 (adr-srv)
    ▼
Track 4.4 (validate.rs migration)
    ▼
Track 5 (SEC-0003 bind-in-library)
```

### Phase 2 v2 risk register (remaining tracks)

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| async-graphql + cherry-pit-web composition gap | M | M | Spike at start of 3.2; if hostile, drop async-graphql for axum-only POST handler. User notified before re-scope. |
| adr-fmt library extraction breaks current binary | L | H | Track 3.1 is an internal-refactor; existing tests cover binary surface. Run `cargo test -p adr-fmt` after every step. |
| Track 4.4 reveals validate.rs surface needs gh-report-specific bits that conflict with adr-srv | M | M | Surface in evidence artefact, decide before coding. Halt-and-handback if conflict implies CHE-0049 / CHE-0050 amendment. |
| Scope creep ("while we're at it…") | H | M | Strict track boundaries; injection queue for discovered work; gardener pass between tracks. |

**Abort criteria**: if any remaining track cannot reach its verify-green within
~3 hopper missions, halt and re-orient via feynman.

### Phase 2 v2 injection queue

New Phase-2 discoveries land here with bead label `phase:2-generalize`.

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
   (`adr-fmt-3d86`). Three candidate mechanisms enumerated in surprise
   artefact `.ooda/surprise-p1b-sub3-1778699612.md`. Decision requires
   oracle orient on `cherry-pit-web` public-API surface (CHE-0049:R1 +
   CHE-0050:R2). Vacuous under default features per CHE-0049:R3+R11.

2. **Adversarial-input gap inventory for cherry-pit-storage lock**
   (bead `adr-fmt-htyk`). Enumerate adversarial inputs the lock
   primitive does not yet defend against (oversized PID, malformed
   UTF-8 in lockfile, symlink races on the lockfile path, etc);
   informational checklist that defers actual harness/fuzz work to
   existing Phase-3 task 5 (file-store error-path property tests).

---

## Injection log

Cross-phase discovery audit trail lives in bd
(`.beads/interactions.jsonl`, append-only). Query:
`bd query --label phase:1-cleanup,phase:2-generalize,phase:3-harden`.

---

## Revision history

| Version | Date       | Changes |
|---------|------------|---------|
| 0.1–0.5 | 2026-05-13 → 2026-05-16 | Initial axis detail → high-level task list; ceremony strip; Phase 2 v1→v2 supersession; Track 4.0 SMI promoted to mechanical exit criterion; LOC-gate amendment. See git log + bd for detail. |
| 0.6     | 2026-05-16 | v0.5 LOC-gate amendment retracted; `scripts/prod-loc` + `scripts/track4-verify` removed; CI job `track4-gates` deleted; `.gitignore` re-adds `scripts/`. Track 4 substance lives in commit diffs + CHE-0062 + CHE-0049-Amendment-Part-2 + SMI-1..SMI-5. Companion: FOCUS.md v0.7. |
| 0.7     | 2026-05-16 | Pruned historic state: Phase 1 (closed) + Phase 2 v1 (superseded) collapsed to single-paragraph pointers; closed Phase 2 v2 Tracks 0/0.5/1/2/4 sub-sections dropped; injection log replaced with bd query pointer; revision history collapsed v0.1–v0.5. Forward-work content (Tracks 3, 4.4, 5; Phase 3) unchanged. Closed-task audit trail SSOT is bd + git log. |
