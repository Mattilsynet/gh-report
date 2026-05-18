# Cherry-Pit Refinement Roadmap

**Status**: Live (Phase 2 v2 — remaining tracks: 3 (read-only re-scope), 4.4, 5, 6, 7, 8)
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
| 2 — Generalize v2 | active | Tracks 3 (read-only re-scope) + 4.4 + 5 + 6 + 7 + 8 |
| 3 — Harden       | not started | 12 tasks (+1 injection, +2 from Track 3 retirement) |

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
consumer (`adr-srv` — GraphQL over async-graphql + axum, **read-only in Phase 2**)
on a fundamentally different storage substrate (`pardosa`), and by migrating
`gh-report`'s persistence onto the same substrate (hard cut; no prod deployments).
The Phase 2 exit also requires an **idiomatic architectural organization audit**
across `adr-srv`, `gh-report`, `cherry-pit-*`, and `pardosa-*` crates. If the
cherry-pit-* traits survive two consumers + two EventStore impls + a workspace-
wide idiomaticity audit, generality is demonstrated mechanically. If they don't,
the gaps surface as code-level friction (not ADR commentary).

**Completion criteria (user-ratified 2026-05-17)**:

- **C1** — `adr-srv` operational in **read-only mode**: scrape every ADR in
  `docs/adr/**`, store in `pardosa-genome` files, serve all ADRs and their
  relations through a GraphQL `Query` interface. No `Mutation` surface in
  Phase 2.
- **C2** — `gh-report` stores its internal state in `pardosa-genome` files.
  Hard cut (no prod deployments); first post-cut run re-scrapes the GitHub API.
- **C3** — Idiomatic architectural organization of `adr-srv`, `gh-report`,
  `cherry-pit-*`, and `pardosa-*` crates, operationalised as a checklist
  derived from existing CHE ADRs.

**Status**: Tracks 0, 0.5, 1, 2, 4 closed. Tracks 3 (read-only re-scope),
4.4, 5, 6, 7, 8 remaining.

**Exit when (mechanical, all in CI)**:

1. `cargo build --workspace` exit 0 with pardosa* activated as workspace members.
2. `cargo test --workspace --all-features` exit 0.
3. `cargo test --workspace --test '*_conformance'` exit 0 with **≥ 2 EventStore
   impls** (file-store + pardosa-adapter) registered.
4. **(C1)** `cargo run -p adr-srv` starts; GraphQL query
   `{ adr(id: "AFM-0001") { title, references { id } } }` returns the
   scraped + projected ADR with its `References` edges.
5. **(C1)** adr-srv's pardosa-genome store on disk contains ≥ 1 event per ADR
   file under `docs/adr/**`; re-running the scrape is idempotent (body_hash skip).
6. **(C2)** gh-report's persistence directory contains pardosa-genome files
   only; no msgpack store on disk; `cargo test -p gh-report` green; first
   post-cut run re-scrapes GitHub API.
7. `cargo tree` shows **no `async-trait`** anywhere in cherry-pit-* dep trees.
8. `cargo run -p adr-fmt -- --lint` warnings-only, no errors (baseline preserved).
9. Bead `adr-fmt-spsd` closed with code reference, not text deferral (Track 5).
10. SMI maintained after Track 7 cut-over (Track 4.0 closed 2026-05-16):
    - `rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/`
      returns zero hits.
    - `rg -n 'EventStore' crates/gh-report/src/` shows write-side use confined
      to the `Merger` module.
    - `cargo test -p gh-report --test smi_replay_equivalence` exit 0.
11. **(Track 6)** Atomic-ship verified: F2f tamper-injection test green,
    F9 wrappers all green, `FORMAT_VERSION = 3` in tree.
12. **(C3)** Track 8 architectural audit checklist committed; every crate
    (`adr-srv`, `gh-report`, `cherry-pit-*`, `pardosa-*`, `adr-fmt`) has a
    checked-or-deferred-to-Phase-3 row with rationale.

**Retired from Phase 2 v2 exit (moved to Phase 3 backlog)**:

- GraphQL mutations smoke test (was old criterion 4: `ratifyAdr`) — write-side
  surface deferred per C1 read-only scope.
- `lint_integration` metacircular adr-fmt-as-projection (was old criterion 5)
  — depends on the mutation surface; deferred.

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

### Track 3 — adr-srv read-only (re-scoped 2026-05-17; gated on Tracks 1 + 2 closed → dispatchable)

Goal: the second real consumer, **read-only in Phase 2**. Scrape every ADR
under `docs/adr/**` into a pardosa-genome event log, project into an
`AdrDocument` read model, and serve via GraphQL `Query`. Mutations and the
metacircular adr-fmt-as-projection loop (old 3.4 + 3.5) move to Phase 3.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 3.1 | adr-fmt library surface | Extract `crates/adr-fmt-core` (or expose `crates/adr-fmt` as lib+bin; lib+bin default for reversibility). Expose `parser`, `model`, `rules::{template, links, naming}`, `containment`, `nav` as public lib API. Binary thin wrapper. Frozen CLI per AFM-0001 unchanged. No pardosa dependency. | Existing `cargo test -p adr-fmt` still green; `adr-srv` can `use adr_fmt_core::Diagnostic`. |
| 3.2 | adr-srv crate skeleton | New `crates/adr-srv`. axum + async-graphql. Aggregate = `AdrDocument`; events = `AdrIngested` (+ later `Drafted`/`Ratified`/`Superseded`/`Retired` in Phase 3). Commands NOT serializable per CHE-0014. EventStore = `PardosaEventStore`. Compiles against pardosa types but does NOT yet write events. | `cargo build -p adr-srv`; `cargo test -p adr-srv` (skeleton tests green). |
| 3.A | ADR scrape pipeline (NEW) | Filesystem walker over `docs/adr/**` via `adr_fmt_core`. Emit one `AdrIngested { id, frontmatter, body_hash, references }` event per file into `PardosaEventStore`. Idempotent: re-scrape compares `body_hash`, skips unchanged. **First persisted event — gated on Track 6 atomic-ship complete.** | `cargo test -p adr-srv --test scrape_pipeline`; re-scrape produces zero new events on unchanged corpus. |
| 3.3 | GraphQL read schema + projection | `Query` types over `Projection` of `AdrDocument`. Surface mirrors `adr-fmt --tree` / `--refs` / `--context`. Projection driven by `cherry-pit-projection` (Track 1.1). No `Mutation` types in Phase 2. | `cargo test -p adr-srv --test graphql_read_e2e`; spawn server, `{ adr(id: "AFM-0001") { title, references { id } } }`, assert shape. |

**Checkpoint**: adr-srv reads end-to-end on pardosa + cherry-pit-projection +
adr-fmt-as-lib. **Read-only generality claim load-bearing.**

**Retired from Phase 2 (moved to Phase 3)**:

- **3.4 GraphQL mutations** (`ratifyAdr`, `supersede`) — write-side surface.
- **3.5 Projection-driven adr-fmt integration** (metacircular lint-as-projection)
  — depends on 3.4 mutation surface.

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

### Track 6 — pardosa-genome file-format hardening (parallel to Tracks 3/4.4/5)

Goal: discharge PAR-0021 (BLAKE3 precursor hash + frontier) on the
pardosa-genome wire format and tighten the event-payload type surface
(F9). Wire-format change: bumps `FORMAT_VERSION` 2 → 3 with read-only
migration path for v2 streams. **Independent of the adr-srv chain** —
different crate boundaries (`crates/pardosa*`), different invariants.

Two epics, both open:

#### Epic 6.A — PAR-0021 BLAKE3 precursor hash + frontier (`adr-fmt-il9a`, P0)

Wire-format change. Six sub-tasks; F2a is the root (6 dependents).

| # | Bead | Task | Verify |
|---|------|------|--------|
| 6.A.1 | `adr-fmt-hthf` | F2a: Add `precursor_hash: [u8; 32]` field to `Event<T>`; bump `FORMAT_VERSION` to 3; v2-read migration with zero-hash sentinel | v3 round-trip test; v2 stream reads as v3 with zero-hash; `cargo test -p pardosa --all-features` |
| 6.A.2 | `adr-fmt-jjjd` | F2b: BLAKE3 hash-over-canonical-bytes helper in `pardosa-encoding` (behind `blake3` feature; no_std default preserved) | Determinism test; `cargo deny` + `cargo audit` clean; `cargo build -p pardosa-encoding` with default features still passes |
| 6.A.3 | `adr-fmt-qm6u` | F2c: `Dragline::frontier` field + roll-forward on commit (PAR-0021 R3) | Frontier advances monotonically across commits; depends on 6.A.1 + 6.A.2 |
| 6.A.4 | `adr-fmt-m5e1` | F2d: Extend `verify_precursor_chains` with BLAKE3 verification (PAR-0021 R5) | Verifier rejects tampered history; depends on 6.A.1 + 6.A.2 |
| 6.A.5 | `adr-fmt-issx` | F2e: Frontier publisher to `pardosa.{stream}.frontier` (PAR-0021 R4) | Frontier published over NATS; externally verifiable |
| 6.A.6 | `adr-fmt-eyaz` | F2f: Tamper-injection integration test for PAR-0021 | Integration test detects history rewrite; epic acceptance gate |

#### Epic 6.B — F9 event-type surface tightening (`adr-fmt-e71p`, P1)

Schema-hash-affecting; wire-format compatible for valid values. **Ships
atomically inside the F2a `FORMAT_VERSION=3` commit-set** so consumers
re-validate once across F2+F9. Depends on Epic 6.A.

| # | Bead | Task | Verify |
|---|------|------|--------|
| 6.B.1 | `adr-fmt-njvo` | F9a: Float-tier wrappers (`FiniteF{32,64}` / `RealF{32,64}` / `OrderedF{32,64}`) | `TryFrom<inner>` + `Validate` with `ValidationCost::Cheap` |
| 6.B.2 | `adr-fmt-3ez0` | F9b: `CharScalar` wrapper + raw `char` retention | Wrapper rejects invalid scalars; raw `char` still serialisable with doctrine note |
| 6.B.3 | `adr-fmt-fwqb` | F9c: Remove `GenomeSafe` for `&str` / `&[u8]` (unconditional) | Lifetime-hazardous borrows no longer encodable into stored events |
| 6.B.4 | `adr-fmt-8paj` | F9d: Remove `GenomeSafe` for `Arc<T>` / `Cow<'_, T>` | Runtime-sharing wrappers no longer encodable |
| 6.B.5 | `adr-fmt-llu4` | F9e: Draft `GEN-xxxx` ADR — Idiomatic types for event payloads | ADR landed; cited from impl-level doc comments |

#### Adjacent loose tasks (Phase-2, genome / encoding, not under either epic)

| Bead | Pri | Task |
|------|-----|------|
| `adr-fmt-o9lp` | P1 | F1: `DeError::SchemaMismatch` widens to u128 to carry full xxh3-128 hash |
| `adr-fmt-mync` | P1 | F3: Encode/Decode parity audit; implement missing `Decode for BTreeSet<T>` |
| `adr-fmt-bbpm` | P2 | F4: Migrate `pardosa-traits` to `no_std + alloc` (foreign-floor parity with `pardosa-encoding`) |
| `adr-fmt-b7lk` | P2 | F5: Add fallible `Index` constructor (GEN-0001 parse-don't-validate) |
| `adr-fmt-ljek` | P2 | F7: Derive macro serde-attribute completeness audit (EVT catalog extension) |
| `adr-fmt-2odp` | P2 | `EventError::CapExceeded` discriminant for decoder-cap surface (post-FH11) |

**Backfill note**: these F-task beads currently lack the
`phase:2-generalize` label. Backfill is a one-shot bd-side action
(`bd label add <id> phase:2-generalize` for each), separate from this
roadmap edit. Until then, the SSOT for Track 6 status is this section
plus `bd query` by F-prefix title.

### Track 7 — gh-report → pardosa hard cut (NEW; gated on Track 6 atomic-ship complete)

Goal: discharge **C2**. Migrate `gh-report`'s persistence from the named
MessagePack EventStore (CHE-0031) onto `pardosa-genome` files. Hard cut: no
dual-write, no importer. First post-cut run re-scrapes the GitHub API and
rebuilds local state from scratch. User confirmed (2026-05-17): no production
deployments exist; data loss in the migration is acceptable cost.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 7.1 | PardosaEventStore adapter completeness | Verify Track 2.2 `PardosaEventStore` impl behind `cherry_pit_core::EventStore` covers everything gh-report uses (append, load, CAS via `expected_sequence`, conformance tests). If partial, close the gap. | `cargo test --workspace --test '*_conformance'` green with PardosaEventStore registered. |
| 7.2 | gh-report EventStore swap | Replace gh-report's msgpack-store wiring with `PardosaEventStore`. Delete msgpack on-disk format code + tests. Update CHE-0031 references in code to point at the supersession ADR (7.3). Atomic commit-set per CHE-0022:R1. | `cargo test -p gh-report` green; `cargo build --workspace` green; no msgpack-store references remain (`rg -n 'msgpack.*store\|MsgpackEventStore' crates/gh-report/src/`). |
| 7.3 | Supersession ADR | Draft new CHE-#### ADR superseding CHE-0031 (named-msgpack → pardosa-genome). Cite Track 7.2 commit-set as implementation evidence. Per AFM-0020:R1, `References:` first target = CHE-0031. Per GND-0007:R4, CHE-0031 transitions to `Superseded` with retirement section. **Always-escalate ADR draft per FOCUS.md §6** — already ratified 2026-05-17. | `cargo run -p adr-fmt -- --lint` warnings-only; `cargo run -p adr-fmt -- --refs CHE-0031` shows the new ADR. |
| 7.4 | SMI replay green on fresh log | First post-cut gh-report run re-scrapes GitHub API and writes a fresh pardosa-genome event log. SMI invariants (Track 4.0) preserved across the cut. | `rg -n 'sequence_tracker\|run_index\|repo_index\|delivery_index' crates/gh-report/src/` zero hits; `cargo test -p gh-report --test smi_replay_equivalence` green. |

### Track 8 — C3 idiomatic architectural organization audit (NEW; final Phase 2 track)

Goal: discharge **C3**. Operationalise "idiomatic architectural organization"
as an observable checklist derived from existing CHE ADRs, then audit every
crate against it. Findings become remediation beads (drained in-track or
deferred-to-Phase-3 with rationale). Subjective in principle; mechanical in
practice via checklist.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 8.1 | Author E1 checklist | Single `.ooda/`-then-bd evidence artefact (label `evidence,track:8`) enumerating C3 criteria: (a) hexagonal layering visible (ports/adapters split per crate); (b) no `async-trait` in cherry-pit-* dep tree (CHE-0025); (c) RPITIT at public trait surfaces; (d) ADR coverage — every public type has an ADR home or inline justification; (e) idiomatic crate naming + `lib`/`bin` split; (f) flat public API via `pub use` re-exports (CHE-0030); (g) aggregate boundaries match CHE-0005:R1 (single aggregate per port); (h) dependency direction respects CHE-0029 (acyclic crate DAG). | Checklist bead created with all criteria + verify-grep where applicable. |
| 8.2 | Per-crate audit | Walk every crate in `Cargo.toml [workspace] members` (currently 14: adr-fmt, adr-srv [new], cherry-pit-{core,gateway,web,projection,agent,wq,storage}, gh-report, pardosa, pardosa-genome, pardosa-encoding, pardosa-derive, pardosa-traits). For each, score against 8.1 criteria; file a remediation bead per failure. | One audit row per crate committed in a single audit-report bead; remediation beads filed with `track:8,remediation` labels. |
| 8.3 | Drain or defer remediations | Each remediation bead either closed in-track or labeled `phase:3-harden` with a rationale comment explaining why it can't reasonably ship in Phase 2. | All `track:8,remediation` beads either closed or `phase:3-harden`-labelled. |
| 8.4 | ADR gap-fill | Any 8.2 finding without an ADR home gets a draft ADR (CHE / GND / AFM domain per topic). Per FOCUS.md §6, new ADR drafts are always-escalate — user ratifies each before merge. | New ADRs land in `docs/adr/<domain>/`; `cargo run -p adr-fmt -- --lint` warnings-only. |

### Phase 2 v2 sequencing (remaining)

```
START
  ├─ Track 3.1   adr-fmt-core lib extraction        [no pardosa dep]
  │      ▼
  │   Track 3.2  adr-srv skeleton                    [no pardosa write]
  │
  └─ Track 6    Epic 6.A + Epic 6.B atomic-ship (FORMAT_VERSION=3 + F9)
                Loose tasks (F1, F3, F4, F5, F7, FH11) drain opportunistically

GATE: Track 6 complete
  ▼
Track 3.A  ADR scrape pipeline (first persisted event)
  ▼
Track 3.3  GraphQL Query schema + projection
  ▼
Track 4.4  validate.rs → cherry-pit-web
  ▼
Track 5    SEC-0003 bind-in-library; adr-fmt-spsd closes
  ▼
Track 7    gh-report → pardosa hard cut; CHE-0031 supersede; SMI green
  ▼
Track 8    C3 idiomatic audit + remediation + ADR gap-fill
  ▼
END  (Phase 2 v2 exit; user ratifies Phase 2 → Phase 3 boundary)
```

Tracks 3.1+3.2 and Track 6 run in parallel (disjoint crate sets: `adr-srv` +
`adr-fmt` vs. `pardosa*`). Track 3.A is the first pardosa-write step and is
gated on Track 6 atomic-ship complete — same gate applies to Track 7.

### Phase 2 v2 risk register (remaining tracks)

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| async-graphql + cherry-pit-web composition gap | M | M | Spike at start of 3.2; if hostile, drop async-graphql for axum-only POST handler. User notified before re-scope. |
| adr-fmt library extraction breaks current binary | L | H | Track 3.1 is an internal-refactor; existing tests cover binary surface. Run `cargo test -p adr-fmt` after every step. |
| Track 4.4 reveals validate.rs surface needs gh-report-specific bits that conflict with adr-srv | M | M | Surface in evidence artefact, decide before coding. Halt-and-handback if conflict implies CHE-0049 / CHE-0050 amendment. |
| Track 3 ↔ Track 6 coupling (parallel start, sequenced first-write) misjudged | M | M | First-write gate enforced via Track 3.A explicit "gated on Track 6 atomic-ship" annotation. If 3.1/3.2 inadvertently introduce a pardosa write path before Track 6 closes, halt and audit. |
| Track 7 hard cut loses unrecoverable state | L | L | No prod deployments per user (2026-05-17). First post-cut run re-scrapes GitHub API; local state rebuilds. Acceptable cost. |
| Track 7 CHE-0031 supersession ADR inbound-ref repointing | L | M | Run `cargo run -p adr-fmt -- --refs CHE-0031` before Track 7.3; repoint each citation per AFM-0020 / GND-0007:R2. |
| Track 8 "idiomatic" subjective, audit becomes bikeshedding | M | M | Authored from existing CHE ADRs only; no new criteria invented in Track 8. Disagreements escalate as ADR drafts (8.4), not as 8.1 churn. |
| Scope creep ("while we're at it…") | H | M | Strict track boundaries; injection queue for discovered work; gardener pass between tracks. |
| Track 6 wire-format change strands v2 readers | L | H | F2a includes read-only migration path (v2 streams decode with zero-hash sentinel); F2f tamper-injection test asserts v2→v3 read still works. Halt-and-handback if migration path proves infeasible. |
| Track 6 atomic-ship coupling (F2a + F9) inflates blast radius | M | M | Epic acceptance criteria require atomic landing; mitigation = small TDD increments behind the `blake3` feature flag until F2f integration test green, then single squash commit. |

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

3. **adr-srv GraphQL mutations** (retired from Phase 2 Track 3.4 on
   2026-05-17). `Mutation` surface — `ratifyAdr(id)` /
   `supersede(old, new)` — mapped to commands via
   `cherry_pit_web::CommandRouter`. Persist via `PardosaEventStore`,
   project via Track 1.1. Verify: `cargo test -p adr-srv --test
   graphql_write_e2e`. Re-scoped here because Phase 2 v2 completion
   criterion C1 is **read-only**; the write-side surface needs separate
   ratification before re-activation. No bead yet (file when Phase 3
   activates).

4. **Projection-driven adr-fmt integration (metacircular)** (retired
   from Phase 2 Track 3.5 on 2026-05-17). adr-srv's projection re-runs
   adr-fmt's lint rules on every event; output surfaced via
   `{ lint { diagnostics { id, severity, … } } }`. Closes the
   metacircular loop. Depends on Phase-3 injection #3 (mutation
   surface). Verify: `cargo test -p adr-srv --test lint_integration`;
   introduce a synthetic L0xx-violating ADR via mutation, assert
   diagnostic appears in query. No bead yet (file when Phase 3
   activates).

5. **Workspace hash-algorithm consolidation** (drafted 2026-05-18; mission
   package preserved as bd evidence bead — see bd query
   `--label hash-consolidation,evidence`). Collapse three hash policies
   (SHA-256, BLAKE3, xxhash) onto a single rule: "BLAKE3 where there's an
   adversary (precursor chain, frontier); HMAC-SHA256 for external protocols
   (GitHub `x-hub-signature-256`); xxh3-family otherwise (file checksums,
   snapshot signatures, ETags)." Six sub-missions: (1) author COM-0039
   umbrella ADR; (2) supersede GEN-0016, update CHE-0053 R11 to xxh3-128;
   (3) pardosa-genome xxh64→xxh3-64 + `FORMAT_VERSION` 3→4 (hard cut);
   (4) cherry-pit-storage snapshot signature SHA-256→xxh3-128 (drop sha2
   dep); (5a) extract three `compute_etag` sites to one shared helper
   (structural; SHA-256 preserved); (5b) swap shared helper to xxh3-128
   (behavioural; one-time RFC 9110 §8.8.3 revalidation); (6) audit gate.
   Gated on Track 6 atomic-ship complete (`FORMAT_VERSION=3` in tree) so
   the 3→4 bump doesn't fight Track 6's atomic-ship. Verify: `rg 'use sha2'
   crates/` returns exactly 1 hit (`gh-report/src/webhook/signature.rs`);
   `cargo tree -p cherry-pit-storage -i sha2` empty; `rg 'fn compute_etag'
   crates/` returns exactly 1 hit; `cargo test --workspace --all-features`
   exit 0. Algorithm choices ratified by user 2026-05-18 (xxh3-128 not
   BLAKE3 for snapshot sig + ETags — right-sized for no-adversary threat
   model; ~10× faster than SHA-256). Mission-package body lives in bd
   evidence bead (queryable via the label-based query above).

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
| 0.8     | 2026-05-17 | Surfaced parallel Phase-2 file-format work stream as Track 6 (pardosa-genome file-format hardening); Epic 6.A = PAR-0021 (`adr-fmt-il9a`, 6 sub-tasks), Epic 6.B = F9 (`adr-fmt-e71p`, 5 sub-tasks), plus 6 adjacent loose tasks. Previously bd-only; roadmap omitted 19 open Phase-2 beads. Sequencing diagram updated; risk register extended with wire-format / atomic-ship rows. F-task `phase:2-generalize` label backfill is a separate bd action. |
| 0.9     | 2026-05-17 | **User-ratified Phase 2 v2 completion criteria** (C1/C2/C3): C1 = adr-srv operational in **read-only mode** (scrape ADRs → pardosa-genome → GraphQL Query); C2 = gh-report stores internal state in pardosa-genome files (hard cut, re-scrape GitHub API); C3 = idiomatic architectural organization audit across adr-srv / gh-report / cherry-pit-* / pardosa-*. **Track 3 re-scoped** to read-only: 3.4 (GraphQL mutations) and 3.5 (metacircular adr-fmt-as-lint loop) retired to Phase 3 backlog; new 3.A scrape-pipeline sub-task added. **Track 7 added** (gh-report → pardosa hard cut; supersedes CHE-0031; 4 sub-tasks). **Track 8 added** (C3 idiomatic audit, 4 sub-tasks). Exit criteria rewritten: 12 criteria, indexed to C1/C2/C3/Track 6. Sequencing updated: Track 3.1/3.2 + Track 6 parallel at start; first pardosa write (3.A) gated on Track 6 atomic-ship; Track 7/8 sequenced after Track 5. Risk register extended with Track 3↔6 coupling, Track 7 hard-cut, Track 8 audit-bikeshedding rows. **Original v0.8 Track 6 content preserved unchanged.** PAR-0021 sequencing decision: F2 chain lands before any consumer write (Track 3.A + Track 7), per user direction; "parallel" in roadmap means concurrent agents on disjoint crate trees, not concurrent first-writes. Track 6 atomic-ship (Epic 6.A + 6.B together) preserved per user direction. Track 4.4 + Track 5 placement preserved (sequenced after Track 3.3 per user "as early as possible in Phase 2" direction). Companion: FOCUS.md sync to v0.8 (this commit). |
| 1.0     | 2026-05-18 | **Phase 3 injection queue extended**: item #5 — workspace hash-algorithm consolidation. Drafted as mission package `hash-consolidation-1779148800`; promoted to bd evidence bead (label `evidence,hash-consolidation,phase:3-harden,roadmap-deferred`). Collapses three hash policies onto one rule ("adversary → BLAKE3; external protocol → HMAC-SHA256; otherwise → xxh3"). Six sub-missions; gated on Phase 2 Track 6 atomic-ship complete. Algorithm choices user-ratified 2026-05-18 (xxh3-128 for snapshot signature + ETags; right-sized for no-adversary threat model). Phase 2 v2 forward-work unchanged. |
