# Cherry-Pit Refinement Roadmap

**Status**: Live (Phase 2 v2 — remaining tracks: 3 (read-only re-scope), 4.4, 5, 8, 10)
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

| Phase | Status | Remaining |
|-------|--------|-----------|
| 2 — Generalize v2 | active | Tracks 3 (read-only re-scope) + 4.4 + 5 + 8 + 10 |
| 3 — Harden       | not started | 12 tasks (+1 injection, +2 from Track 3 retirement) |

---

## Phase 2 v2 — Generalization by Construction

**Intent**: Prove cherry-pit-* is general by *constructing* a second non-trivial
consumer (`adr-srv` — GraphQL over async-graphql + axum, **read-only in Phase 2**)
over the cherry-pit substrate. The Phase 2 exit also requires an
**idiomatic architectural organization audit** across `adr-srv`, `gh-report`,
and `cherry-pit-*` crates. If the cherry-pit-* traits survive two consumers
plus a workspace-wide idiomaticity audit, generality is demonstrated
mechanically. If they don't, the gaps surface as code-level friction
(not ADR commentary).

**Completion criteria (user-ratified 2026-05-17)**:

- **C1** — `adr-srv` operational in **read-only mode**: scrape every ADR in
  `docs/adr/**`, store as cherry-pit events, serve all ADRs and their
  relations through a GraphQL `Query` interface. No `Mutation` surface in
  Phase 2.
- **C2** — `gh-report` DDD tactical alignment (Track 10): Vernon Value
  Objects, per-aggregate event enums, Tension-2 retirement, Merger ADR.
- **C3** — Idiomatic architectural organization of `adr-srv`, `gh-report`,
  and `cherry-pit-*` crates, operationalised as a checklist derived from
  existing CHE ADRs.

**Status**: see "Phase 2 v2 sequencing (remaining)" below.

**Exit when (mechanical, all in CI)**:

1. `cargo build --workspace` exit 0.
2. `cargo test --workspace --all-features` exit 0.
3. `cargo test --workspace --test '*_conformance'` exit 0 with the
   cherry-pit `EventStore` impl(s) registered.
4. **(C1)** `cargo run -p adr-srv` starts; GraphQL query
   `{ adr(id: "AFM-0001") { title, references { id } } }` returns the
   scraped + projected ADR with its `References` edges.
5. **(C1)** adr-srv's event store on disk contains ≥ 1 event per ADR
   file under `docs/adr/**`; re-running the scrape is idempotent (body_hash skip).
6. **(C2)** Track 10 verifies green (see Track 10 below).
7. `cargo tree` shows **no `async-trait`** anywhere in cherry-pit-* dep trees.
8. `cargo run -p adr-fmt -- --lint` warnings-only, no errors (baseline preserved).
9. Bead `adr-fmt-spsd` closed with code reference, not text deferral (Track 5).
10. SMI maintained (Track 4.0 closed 2026-05-16):
    - `rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/`
      returns zero hits.
    - `rg -n 'EventStore' crates/gh-report/src/` shows write-side use confined
      to the `Merger` module.
11. **(C3)** Track 8 architectural audit checklist committed; every crate
    (`adr-srv`, `gh-report`, `cherry-pit-*`, `adr-fmt`) has a
    checked-or-deferred-to-Phase-3 row with rationale.

**Retired from Phase 2 v2 exit (moved to Phase 3 backlog)**:

- GraphQL mutations smoke test (was old criterion 4: `ratifyAdr`) — write-side
  surface deferred per C1 read-only scope.
- `lint_integration` metacircular adr-fmt-as-projection (was old criterion 5)
  — depends on the mutation surface; deferred.

### Out of scope (retained for Phase 3)

- NATS in production deployments of adr-srv or gh-report (tests-only is enough).
- SEC-0010 (NATS TLS) — Phase 3.
- SEC-0011 (tamper-evident hash-chain logs) — Phase 3.
- Adversarial-input fuzz harnesses — Phase 3 tasks 2 / 3.
- TLA+ / Smithy specs — Phase 3 tasks 9 / 10.
- CHE-0044 object_store backend — Phase 3 review.
- crates.io publication — separate concern.

### Track 3 — adr-srv read-only (re-scoped 2026-05-17; gated on Tracks 1 + 2 closed → dispatchable)

Goal: the second real consumer, **read-only in Phase 2**. Scrape every ADR
under `docs/adr/**` into the cherry-pit event store, project into an
`AdrDocument` read model, and serve via GraphQL `Query`. Mutations and the
metacircular adr-fmt-as-projection loop (old 3.4 + 3.5) move to Phase 3.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 3.1 | adr-fmt library surface | Extract `crates/adr-fmt-core` (or expose `crates/adr-fmt` as lib+bin; lib+bin default for reversibility). Expose `parser`, `model`, `rules::{template, links, naming}`, `containment`, `nav` as public lib API. Binary thin wrapper. Frozen CLI per AFM-0001 unchanged. | Existing `cargo test -p adr-fmt` still green; `adr-srv` can `use adr_fmt_core::Diagnostic`. |
| 3.2 | adr-srv crate skeleton | New `crates/adr-srv`. axum + async-graphql. Aggregate = `AdrDocument`; events = `AdrIngested` (+ later `Drafted`/`Ratified`/`Superseded`/`Retired` in Phase 3). Commands NOT serializable per CHE-0014. EventStore = cherry-pit substrate. | `cargo build -p adr-srv`; `cargo test -p adr-srv` (skeleton tests green). |
| 3.A | ADR scrape pipeline | Filesystem walker over `docs/adr/**` via `adr_fmt_core`. Emit one `AdrIngested { id, frontmatter, body_hash, references }` event per file into the cherry-pit `EventStore`. Idempotent: re-scrape compares `body_hash`, skips unchanged. | `cargo test -p adr-srv --test scrape_pipeline`; re-scrape produces zero new events on unchanged corpus. |
| 3.3 | GraphQL read schema + projection | `Query` types over `Projection` of `AdrDocument`. Surface mirrors `adr-fmt --tree` / `--refs` / `--context`. Projection driven by `cherry-pit-projection` (Track 1.1). No `Mutation` types in Phase 2. | `cargo test -p adr-srv --test graphql_read_e2e`; spawn server, `{ adr(id: "AFM-0001") { title, references { id } } }`, assert shape. |

**Checkpoint**: adr-srv reads end-to-end on cherry-pit-projection +
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

### Track 8 — C3 idiomatic architectural organization audit (NEW; final Phase 2 track)

Goal: discharge **C3**. Operationalise "idiomatic architectural organization"
as an observable checklist derived from existing CHE ADRs, then audit every
crate against it. Findings become remediation beads (drained in-track or
deferred-to-Phase-3 with rationale). Subjective in principle; mechanical in
practice via checklist.

| # | Task | Deliverable | Verify |
|---|------|-------------|--------|
| 8.1 | Author E1 checklist | Single bd evidence artefact (label `evidence,track:8`) enumerating C3 criteria: (a) hexagonal layering visible (ports/adapters split per crate); (b) no `async-trait` in cherry-pit-* dep tree (CHE-0025); (c) RPITIT at public trait surfaces; (d) ADR coverage — every public type has an ADR home or inline justification; (e) idiomatic crate naming + `lib`/`bin` split; (f) flat public API via `pub use` re-exports (CHE-0030); (g) aggregate boundaries match CHE-0005:R1 (single aggregate per port); (h) dependency direction respects CHE-0029 (acyclic crate DAG). | Checklist bead created with all criteria + verify-grep where applicable. |
| 8.2 | Per-crate audit | Walk every crate in `Cargo.toml [workspace] members` (currently 10: adr-fmt, adr-srv, cherry-pit-{core,gateway,web,projection,agent,wq,storage}, gh-report). For each, score against 8.1 criteria; file a remediation bead per failure. | One audit row per crate committed in a single audit-report bead; remediation beads filed with `track:8,remediation` labels. |
| 8.3 | Drain or defer remediations | Each remediation bead either closed in-track or labeled `phase:3-harden` with a rationale comment explaining why it can't reasonably ship in Phase 2. | All `track:8,remediation` beads either closed or `phase:3-harden`-labelled. |
| 8.4 | ADR gap-fill | Any 8.2 finding without an ADR home gets a draft ADR (CHE / GND / AFM domain per topic). Per FOCUS.md §6, new ADR drafts are always-escalate — user ratifies each before merge. | New ADRs land in `docs/adr/<domain>/`; `cargo run -p adr-fmt -- --lint` warnings-only. |

### Track 10 — Vernon DDD tactical-pattern alignment (gated on Track 8 complete)

Goal: close the gap between the cherry-pit ADR corpus (which faithfully
translates Vernon's *Implementing DDD* Ch. 5–10 into Rust types at the
substrate level) and the gh-report consumer (where the substrate's DDD
discipline is not consistently expressed). Read-only assessment
2026-05-18 against Vernon's pattern-by-pattern checklist found
cherry-pit-core exemplary on Entities (CHE-0011/0020), Domain Events
(CHE-0010/0016/0042), Aggregates (CHE-0005/0006/0008/0009/0012), and
Repository-reframed-as-EventStore; gaps cluster at the consumer layer:
Value Objects (Ch. 6), Bounded-Context event-type partitioning (Ch. 10
Rule 4), Application Services (Ch. 14 — gh-report realises them today
via the `Merger` task per CHE-0054:R4/R10), and Anti-Corruption Layer
at the GitHub edge (Ch. 3 / Ch. 13). Track scope is
**gh-report + cherry-pit-core + cherry-pit-projection**; adr-srv (Track 3)
inherits VOs + event-enum discipline by construction.

Triggering evidence (assessment artefacts):

- `bd show adr-fmt-2ww8n` — Vernon Ch. 5–10 tactical-pattern assessment
  (re-verified 2026-05-18; 6/7 findings confirmed, 1 delta).
- `bd show adr-fmt-fyrlo` — oracle ADR-binding survey across CHE
  corpus; identifies PAR-0024:R5 misattribution
  (canonical-bytes anchor is **CHE-0064:R2 — hand-rolled `Encode`,
  no `#[derive]`** — not PAR-0024:R5, which is naming-discipline only)
  and CHE-0054:R8/R10 carve-out preserving gh-report's non-consumption
  of `CommandGateway` / `App<...>`.
- `bd show adr-fmt-9kz7p` — feynman orientation: final four-mission
  shape recommendation; option (a) multi-projection over option (b)
  facade enum (user-ratified 2026-05-18 at plan review).

Key file:line evidence: `crates/gh-report/src/domain/events.rs:45-180`
(9-variant umbrella `DomainEvent`); `crates/gh-report/src/domain/aggregates/{run,repo,webhook}.rs`
defensive `debug_assert!(false, "CHE-0054:R5 routing bug")` arms (one
per aggregate) admitting the type-system doesn't enforce CHE-0005:R1;
`crates/gh-report/src/domain/repository.rs:17-65` GitHub-API shape
leaking into domain entity; CHE-0054:R5 names `RepoIdentity` but
implementation uses `String` (zero hits for the type).

**Four missions, sequenced. Mission 10.5 (ACL) deferred to Phase 3
§G #20.** Each mission is a single landing with a mechanical verify.
Mission packages live in bd (epic `adr-fmt-u3pim`); per-mission
contracts under `mission:track10-ddd-vernon-1779131035`.

#### 10.2 — Value Objects + `RepoIdentity` newtype (gh-report)

Deliverable: introduce domain newtypes in `crates/gh-report/src/domain/`:
`BatchId(Uuid)`, `RepoIdentity(String)` (closes the CHE-0054:R5
abstraction gap — the rule names the type but zero implementations
exist), `Org(String)`, `EventTimestamp(jiff::Timestamp)` (CHE-0034:R1).
Validated constructors per CHE-0002:R3 + COM-0020:R1 (private fields,
no `pub fn new_unchecked`). Hand-rolled `Encode` impl per
**CHE-0064:R2** (no `#[derive(Encode)]`); `serde(transparent)` so wire
format is byte-unchanged. Migrate command structs (`StartSweep`,
`RecordProgress`, …) + `DomainEvent` variant payload fields to the
newtypes. `AppState` `DashMap` indices retype to
`DashMap<RepoIdentity, AggregateId>` etc. — CHE-0054:R5 wording amends
in line (key types change; three-index structure preserved).

Verify: `cargo test -p gh-report --all-features` exit 0;
`rg -n 'domain_key: String\|batch_id: String\|repo_name: String\|timestamp: String' crates/gh-report/src/domain/`
returns zero hits inside command + event types (current baseline: 42
hits per assessment `adr-fmt-2ww8n` finding 1); canonical-bytes invariant per
CHE-0064:R2 + existing CHE-0038:R4 golden fixtures re-pass byte-unchanged (transparent
newtype invariant); trybuild compile-fail fixtures land per
CHE-0028:R1/R3 + CHE-0038:R2 for each VO's negative case.

#### 10.3 — Per-aggregate event enums + Tension-2 retirement + multi-projection (gh-report)

Deliverable: largest mission of the track. Three coupled changes
landing as separate-but-sequential sub-missions, each linus-reviewed:

(a) **Partition `DomainEvent`** in `crates/gh-report/src/domain/events.rs`
    into `RunEvent` (6 variants: `SweepStarted`, `SweepCompleted`,
    `SweepFailed`, `SweepProgress`, `EvidencePublished`,
    `PartialEvidenceRendered`), `RepoEvent` (2 variants:
    `RepoEvaluated`, `RepoRemoved`), `WebhookEvent` (1 variant:
    `WebhookReceived`). Each aggregate's `type Event = …` becomes its
    own enum (CHE-0005:R1 restored at the type level). The
    `debug_assert!(false, "CHE-0054:R5 routing bug")` arms in all
    three aggregates (`run.rs:99`, `repo.rs:86`, `webhook.rs:87`) and
    their paired `#[should_panic]` tests delete. Hand-rolled
    `impl Encode` per-aggregate enum **must preserve original
    umbrella discriminant per variant** — RunEvent variants keep
    `0u8, 3u8, 5u8, 6u8, 7u8, 8u8`; RepoEvent keeps `1u8, 2u8`;
    WebhookEvent keeps `4u8` — NOT renumber from 0 within each enum.
    Canonical-bytes anchor: **CHE-0064:R2** (hand-rolled `Encode`,
    no derive). `event_type()` strings are immutable per CHE-0010:R2
    + CHE-0022:R4. Per-variant discriminant bytes unchanged per CHE-0064:R2 (canonical-bytes invariant via hand-rolled `Encode`).

(b) **Tension-2 lock retirement** at
    `crates/gh-report/src/projection.rs:18-26` (single `OrgGovernance`
    aggregate for all 9 events) and singleton `AggregateId` constant
    block at `:55-60`. User-ratified 2026-05-18: **option (a)
    multi-projection** over option (b) facade enum. Decompose
    `EvidenceProjection` into per-aggregate `RunProjection` /
    `RepoProjection` / `WebhookProjection` composed at the
    `ProjectionDriver` boundary; no `AnyDomainEvent` facade.

(c) **Preflight I0** before any code edit: read
    `crates/cherry-pit-projection/src/lib.rs` to verify the
    composition surface admits per-aggregate `Projection` impls under
    one `ProjectionDriver`. CHE-0054:R8 already permits the
    `gh-report → cherry-pit-projection` dep edge for
    `ProjectionDriver` + `FileProjectionStore`; the open question is
    the *shape* of the composition primitive. If closed, halt and
    back-brief moltke (`BriefScope::PackageLevel`) — moltke decides
    between widening the API inside 10.3 within CHE-0054:R8 or
    escalating to user for option (b) re-ratification.

(d) **ADR amendments** (wording-only, mechanical):
    CHE-0054:R1/R2/R3 (replace `DomainEvent::*` with appropriate
    `Aggregate::Event` types); CHE-0063:R6 (cite `RunEvent` as the
    additive home of `PartialEvidenceRendered`); CHE-0048-family
    projection ADR (identified at mission time via
    `adr-fmt --refs CHE-0048`) names projection co-migration across
    per-aggregate event enums. Preflight: if
    `adr-fmt --refs CHE-0054` inbound-ref count exceeds 5,
    back-brief moltke before code work starts (PM4 mitigation).

Verify: `cargo test -p gh-report --all-features` exit 0;
`rg -n 'routing bug\|debug_assert!\(false' crates/gh-report/src/domain/aggregates/`
zero hits;
`rg -n 'Tension-2\|single aggregate.*OrgGovernance' crates/gh-report/src/projection.rs`
zero hits in live code;
canonical bytes per CHE-0064:R2 + existing CHE-0038:R4 serde golden fixtures re-pass
byte-unchanged (per CHE-0022:R2 + CHE-0064:R2 canonical-bytes
invariant); `cargo run -p adr-fmt -- --lint` warnings-only.

#### 10.4 — Doc-only ADR codifying the Merger pattern

Deliverable: a new CHE ADR (`docs/adr/cherry/CHE-NNNN-merger-as-application-service.md`,
NNNN = next free CHE slot resolved at mission time) codifying
gh-report's bespoke `Merger`-task pattern
(`crates/gh-report/src/app/merger.rs:109,347-414`) as the v0.1
realisation of an Application Service for a multi-aggregate consumer.
**This is a doc-only ADR, not a primitive extraction.** No new traits,
no new crate, no amendment to `CommandGateway`/`CommandRouter`
(CHE-0050), no consumption of `cherry-pit-agent::App<...>` by
gh-report. CHE-0054:R8/R10 carve-out preserved exactly as written —
the new ADR cites R8/R10 as parents and *amplifies* (not amends) them
by documenting the multiplexer realisation already in tree.

Wording discipline: each per-aggregate `ApplicationService`
(`RunService`/`RepoService`/`WebhookService`) **delegates** its triad
to a shared Merger task; per-aggregate identity is preserved at the
service-method boundary. This phrasing keeps CHE-0054:R4 (each
aggregate has a dedicated service) intact while documenting the
multiplexer task. Trait-level `ApplicationService<A, S, B>` extraction
is deferred to Phase 3 when a second write-side consumer surfaces
(adr-srv read-only in Phase 2 does not exercise the triad).

Verify: `cargo run -p adr-fmt -- --lint` warnings-only (no errors per
AFM-0003); `cargo run -p adr-fmt -- --refs CHE-0054` lists the new
ADR; `cargo run -p adr-fmt -- --tree CHE` shows the new ADR registered
under the AFM-0020 parent-edge model; linus reviews prose for
amendment-vs-amplification distinction (PM6 mitigation).

**Out of scope for Track 10:**

- **Trait-level `ApplicationService<A, S, B>` extraction** into
  `cherry-pit-agent` or a new crate. Deferred to Phase 3 per user
  decision A (feynman bead `adr-fmt-9kz7p` leader H3). The Merger
  pattern (10.4 ADR) names the v0.1 realisation; trait extraction
  waits on a second write-side consumer.
- **Amending `CommandGateway` / `CommandRouter` (CHE-0050)** to
  express Application Service as a first-class trait. Same rationale
  — out of v0.1 scope; CHE-0054:R8/R10 carve-out preserved.
- **Domain Services vs Application Services trait separation**
  (Vernon Ch. 7). Services already moved Weak→Strong via Track 4.0
  Merger wiring (`adr-fmt-2ww8n` finding 6 delta). No concrete
  violation remains; documentation-only Phase-3 candidate.
- **Versioned event-type naming** (`SweepStartedV1`/`V2` rather than
  additive fields) — CHE-0010:R2 + CHE-0022:R4 forbid renames of
  existing `event_type()` strings. Phase-3 candidate.
- **Strategic DDD** (Bounded Context maps, Context Mapping diagrams).
  Track 10 is tactical-patterns-only; strategic mapping deferred
  until adr-srv + gh-report exercise enough surface to make context
  boundaries observable.
- **Anti-Corruption Layer (`Repository` field removal)** at the
  GitHub edge. Filed as Phase 3 §G #20 with one-paragraph rationale
  citing CHE-0022 silence on event-payload field removal.
- **adr-srv `ApplicationService` consumption** — adr-srv inherits
  VOs + event-enum discipline by construction during Track 3.3; the
  Merger ADR is gh-report-specific.

**Sequencing inside Track 10:**

```
10.1  Scope ratification (roadmap edit; no code)
   ▼
10.2  Value Objects + RepoIdentity        [wire format unchanged]
   ▼
10.3  Per-aggregate event enums + Tension-2 retirement
      + multi-projection (I0 preflight on cherry-pit-projection)
   ▼
10.4  Doc-only ADR codifying Merger pattern (CHE-NNNN)
```

10.2 must precede 10.3 — VOs land before the event-enum split
references them in payload fields. 10.4 must follow 10.3 — the Merger
ADR cites the post-split shape (`RunEvent`/`RepoEvent`/`WebhookEvent`)
in its code citations.

**Risk register additions (8):**

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| 10.3 hand-rolled `impl Encode` per-aggregate enum renumbers discriminants from 0 within each enum, breaking canonical bytes (event identity changes) | M | H | Per-variant discriminant map locked to original umbrella byte assignment (Run: `0,3,5,6,7,8`; Repo: `1,2`; Webhook: `4`) and called out in the commit body. Existing CHE-0038:R4 serde golden fixtures are the byte-identity gate. Abort criterion A1 if golden fixtures break after ≤ 2 hopper sub-mission attempts. |
| 10.3 Tension-2 retirement + multi-projection composition surface in `cherry-pit-projection` is closed against per-aggregate `Projection` impls | M | M | I0 preflight: read `crates/cherry-pit-projection/src/lib.rs` before any code edit; if no public primitive admits the composition, halt and back-brief moltke. CHE-0054:R8 already permits the dep edge; the open question is API shape, not dependency direction. |
| 10.3 ADR amendments on CHE-0054 / CHE-0048 cascade beyond wording-only (`adr-fmt --refs CHE-0054` shows ≥ 8 inbound references) | M | M | Mission 10.3 preflight runs `adr-fmt --refs CHE-0054` and `adr-fmt --refs CHE-0048`; if inbound-ref count exceeds 5 per target, hopper back-briefs moltke before code work starts (PM4). |
| 10.2 VO migration breaks event payload `#[derive(Serialize, Deserialize)]` interaction with `serde(transparent)` newtype + `serde(tag = "type")` enum | M | M | Two-increment TDD: (1) newtype with constructors + `serde(transparent)` + hand-rolled `Encode` impl, red test round-trips newtype alone; (2) migrate ONE command struct field, verify, then the rest. Failure at (2) does not need to revert (1). Existing serde golden fixtures, anchored against CHE-0064:R2 canonical bytes, are the post-each-increment gate. Abort criterion A2. |
| 10.4 Merger ADR wording inadvertently amends CHE-0054:R4 ("Each aggregate has a dedicated ApplicationService") by phrasing Merger as the realisation | L | M | Prose discipline: "per-aggregate services *delegate* triad to shared Merger; per-aggregate identity preserved at service-method boundary". Linus reviews specifically for amendment-vs-amplification (PM6). |
| Scope creep into Vernon strategic patterns (Context Maps) or ACL (now §G #20) | M | M | Out-of-scope list explicit above; injection queue for Phase-3 candidates. Track 10 closes when 10.1–10.4 verify-green. |

**Abort criteria**: if 10.3's golden-fixture canonical-bytes gate cannot
stay green across the split within ≤ 2 hopper sub-mission attempts
(criterion A1), halt and re-orient via feynman — the per-variant
discriminant preservation is the Track 10 cornerstone and any silent
canonical-bytes drift is a halt-and-handback condition. Additional
package-level aborts (A2–A5) live in the package bead `adr-fmt-u3pim`
under "package_abort_criteria".

### Phase 2 v2 sequencing (remaining)

```
START
  └─ Track 3.1   adr-fmt-core lib extraction
       ▼
     Track 3.2   adr-srv skeleton
       ▼
     Track 3.A   ADR scrape pipeline (first persisted event)
       ▼
     Track 3.3   GraphQL Query schema + projection
       ▼
     Track 4.4   validate.rs → cherry-pit-web
       ▼
     Track 5    SEC-0003 bind-in-library; adr-fmt-spsd closes
       ▼
Track 10   gh-report DDD tactical alignment (VOs + per-aggregate enums + Merger ADR)
  ▼
Track 8    C3 idiomatic audit + remediation + ADR gap-fill
  ▼
END  (Phase 2 v2 exit; user ratifies Phase 2 → Phase 3 boundary)
```

### Phase 2 v2 risk register (remaining tracks)

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| async-graphql + cherry-pit-web composition gap | M | M | Spike at start of 3.2; if hostile, drop async-graphql for axum-only POST handler. User notified before re-scope. |
| adr-fmt library extraction breaks current binary | L | H | Track 3.1 is an internal-refactor; existing tests cover binary surface. Run `cargo test -p adr-fmt` after every step. |
| Track 4.4 reveals validate.rs surface needs gh-report-specific bits that conflict with adr-srv | M | M | Surface in evidence artefact, decide before coding. Halt-and-handback if conflict implies CHE-0049 / CHE-0050 amendment. |
| Track 8 "idiomatic" subjective, audit becomes bikeshedding | M | M | Authored from existing CHE ADRs only; no new criteria invented in Track 8. Disagreements escalate as ADR drafts (8.4), not as 8.1 churn. |
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

Tasks are numbered 1–18 contiguously across all six groups (§A–§F) plus
the injection queue (§G). Group headers are descriptive; numbering is
authoritative.

**§A. Interface trust boundaries (adversarial-input)**

1. Wire `gh-report` webhook trust-boundary validation (SEC-0002 R1–R3:
   signature verification, replay protection, request size caps).
2. Adversarial-input fuzz harness for `cherry-pit-web` HTTP surface
   (malformed bodies, oversize headers, slow-loris, encoding tricks).
3. Adversarial-input fuzz harness for `cherry-pit-gateway` event-decode
   surface (corrupt msgpack frames, truncated streams, mis-typed
   discriminants).
4. Webhook signature-verification negative tests (wrong secret, tampered
   payload, missing header, timing-attack resistance).

**§B. Error-path correctness**

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

**§C. Invariant correctness under stress**

8. CHE-0024 (persist-then-publish) failure-mode tests + CHE-0006
   (single-writer) concurrent-command tests + CHE-0022 (append-only)
   in-place-mutation rejection tests.

**§D. Formal specifications**

9. Smithy contract models for `gh-report` webhook ingress,
   `cherry-pit-web` projection-router API, and `cherry-pit` event-envelope
   shape (`specs/smithy/`); validation harness wired into ingress paths.
10. TLA+ specifications for the load-bearing temporal invariants
    (`specs/tla/`); TLC pass; counter-examples become failing tests.
    *(Scope and tool details — PlusCal vs raw TLA+, which invariants —
    decided at task activation.)*

**§E. Security ADR closure**

11. Resolve SEC-0010 (Transport Security / NATS TLS) and SEC-0011
    (Tamper-Evident Logs / hash-chain): elevate to Accepted with
    implementation citation, or retire with rationale. Coupled to
    CHE-0044 disposition.
12. Draft new CHE ADR for secret isolation (per SEC-0007).

**§F. Cross-cutting language doctrine (RST)**

13. Review the cross-cutting RST hardening ideas register against
    Phase-3 work-in-progress; promote any candidate to a real RST ADR
    if-and-only-if a Phase-3 task creates concrete pain or establishes
    a worked example the ADR would describe. Natural candidates: a
    property/fuzz testing methodology ADR (advisory framing) earning
    its keep once tasks 2, 3, 5 land adversarial-input + error-path
    harnesses; a formal-verification / model-checking gate ADR earning
    its keep once tasks 9, 10 land Smithy + TLA+ harnesses; a
    `cargo-deny` / `cargo-audit` enforcement ADR — actionable
    independent of other Phase-3 tasks, lowest-friction candidate if a
    security review surfaces a dependency advisory. Drafting any RST
    ADR remains user-ratified per FOCUS §6 (always-escalate: new ADR).

### §G. Phase-3 injection queue

Discovered work not yet promoted into the §A–§F task list. Numbered
contiguously with the main task list to avoid cross-reference ambiguity.

14. **WS connection cap mechanism for `cherry-pit-web`** (bead
    `adr-fmt-8qj5`, SEC-0003 R2). Deferred from P1-B sub-mission 3
    (`adr-fmt-3d86`). Three candidate mechanisms enumerated in surprise
    artefact `.ooda/surprise-p1b-sub3-1778699612.md`. Decision requires
    oracle orient on `cherry-pit-web` public-API surface (CHE-0049:R1 +
    CHE-0050:R2). Vacuous under default features per CHE-0049:R3+R11.

15. **Adversarial-input gap inventory for cherry-pit-storage lock**
    (bead `adr-fmt-htyk`). Enumerate adversarial inputs the lock
    primitive does not yet defend against (oversized PID, malformed
    UTF-8 in lockfile, symlink races on the lockfile path, etc);
    informational checklist that defers actual harness/fuzz work to
    existing Phase-3 task 5 (file-store error-path property tests).

16. **adr-srv GraphQL mutations** (retired from Phase 2 Track 3.4 on
    2026-05-17). `Mutation` surface — `ratifyAdr(id)` /
    `supersede(old, new)` — mapped to commands via
    `cherry_pit_web::CommandRouter`. Persist via the cherry-pit
    `EventStore`, project via Track 1.1. Verify: `cargo test -p adr-srv --test
    graphql_write_e2e`. Re-scoped here because Phase 2 v2 completion
    criterion C1 is **read-only**; the write-side surface needs separate
    ratification before re-activation. No bead yet (file when Phase 3
    activates).

17. **Projection-driven adr-fmt integration (metacircular)** (retired
    from Phase 2 Track 3.5 on 2026-05-17). adr-srv's projection re-runs
    adr-fmt's lint rules on every event; output surfaced via
    `{ lint { diagnostics { id, severity, … } } }`. Closes the
    metacircular loop. Depends on §G task 16 (mutation surface).
    Verify: `cargo test -p adr-srv --test lint_integration`; introduce
    a synthetic L0xx-violating ADR via mutation, assert diagnostic
    appears in query. No bead yet (file when Phase 3 activates).

18. **Workspace hash-algorithm consolidation** (drafted 2026-05-18; mission
   package preserved as bd evidence bead — see bd query
   `--label hash-consolidation,evidence`). Scope: items (1) COM-0039 umbrella
   ADR (workspace-wide hash-algorithm policy), (2) GEN-0016
   supersession + CHE-0053 R11 update, (4) cherry-pit-storage snapshot
   signature SHA-256→xxh3-128 (drop sha2 dep), (5a) extract three
   `compute_etag` sites to one shared helper (structural; SHA-256
   preserved), (5b) swap shared helper to xxh3-128 (behavioural; one-time
   RFC 9110 §8.8.3 revalidation), (6) audit gate. Collapse three hash
   policies (SHA-256, BLAKE3, xxhash) onto a single rule: "BLAKE3 where
   there's an adversary; HMAC-SHA256 for external protocols
   (GitHub `x-hub-signature-256`); xxh3-family otherwise (file checksums,
   snapshot signatures, ETags)." Verify: `rg 'use sha2' crates/` returns
   exactly 1 hit (`gh-report/src/webhook/signature.rs`); `cargo tree -p
   cherry-pit-storage -i sha2` empty; `rg 'fn compute_etag' crates/`
   returns exactly 1 hit; `cargo test --workspace --all-features` exit 0.
   Algorithm choices ratified by user 2026-05-18 (xxh3-128 not BLAKE3 for
   snapshot sig + ETags — right-sized for no-adversary threat model;
   ~10× faster than SHA-256). Mission-package body lives in bd evidence
   bead (queryable via the label-based query above).

19. **gh-report `SweepProgress` publish ordering** (observed 2026-05-18 in
    `/tmp/gh-report-eval2.log` lines 16, 19: `routing index has no
    AggregateId for batch_id="c25e81d…"`). Within a single process
    lifetime, `record_progress` reaches the merger before `start_sweep`
    has registered the batch_id in `runs_by_key`. Plausible causes
    include the warm-start synthetic publish path (`collect.rs:1407`
    `warm-start-{}`) racing real `SweepStarted`, or the merger queue
    being unordered with respect to publish/append. Needs feynman
    orient (multiple plausible causal models). Non-fatal warning today;
    logged at WARN level. Matches §B (Error-path correctness) intent.
    Verify: a regression test exercising the warm-start → first-sweep
    transition asserts no `routing index has no AggregateId for
    batch_id=…` warning is logged. No bead yet (file when Phase 3
    activates).

20. **Anti-Corruption Layer for GitHub API edge in gh-report** (deferred
    from Phase 2 Track 10 mission 10.5 on 2026-05-18). Separate
    GitHub-API DTOs from the domain `Repository` entity: move
    API-shaped fields (`node_id`, `html_url`, `topics`, `license_spdx`,
    `pushed_at`, `fork`, `description`) out of
    `crates/gh-report/src/domain/repository.rs:17-65` into
    `crates/gh-report/src/github/dto.rs`; define a pure-domain
    `Repository` retaining only identity + business-meaningful fields.
    Map at the adapter boundary. The hand-rolled `PartialEq` in
    `repository.rs` (visible Vernon-tell: equality had to be corrected
    because foreign fields don't belong to identity) disappears.
    **Deferred rationale**: CHE-0022 is silent on whether removing
    fields from already-emitted event payloads (the GitHub-shaped
    fields appear inside `RepoEvaluated`) is permitted under
    `event_type()` immutability (CHE-0010:R2 + CHE-0022:R4). Adding
    fields is additive-safe; removing fields requires either an ADR
    amendment to CHE-0022 carving out a removal protocol, or a new
    `RepoEvaluatedV2` event-type with the lean payload (which itself
    requires CHE-0010:R2 amendment for the rename). Neither path fits
    inside Track 10's tactical-only scope. Filing here so the GitHub
    adapter cleanup happens once the event-payload removal protocol is
    ratified at the ADR level. Verify (when activated): `cargo test -p
    gh-report --all-features` exit 0; `rg -n
    'html_url\|topics\|license_spdx\|pushed_at\|node_id'
    crates/gh-report/src/domain/` zero hits; CHE-0022 + CHE-0045
    amendments landed. No bead yet (file when Phase 3 activates).

---

## Injection log

Cross-phase discovery audit trail lives in bd
(`.beads/interactions.jsonl`, append-only). Query:
`bd query --label phase:1-cleanup,phase:2-generalize,phase:3-harden`.

---

## Revision history

See `git log -- docs/c4/roadmap.md` for revision history.
