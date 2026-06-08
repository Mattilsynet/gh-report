# Solon — Closure

**Status**: Draft v0.1
**Genre**: Terminal milestone gate. v0.1 only.
**Reader**: moltke decomposing the closing missions; user ratifying the
v0.1 transition.
**Scope**: From the current Phase 2 v2 state to v0.1 shipped. Phase 3
(Harden) and publication are out of scope.
**Companions**: `docs/STORY.md` (apex; *why* and *where to play*),
`FOCUS.md` (refinement recipe), `docs/c4/roadmap.md` (live dashboard),
`AGENTS.md` (agent doctrine).

---

## 0. How to read this document

This is the closing checklist for **Solon v0.1**. It does not introduce
new doctrine; it indexes existing work at the v0.1-relevant grain and
declares the exit gate.

Distinctions from neighbouring documents:

| Document | Genre | Lifetime |
|----------|-------|----------|
| `STORY.md` | Strategic intent | Lives across versions |
| `FOCUS.md` | Refinement recipe (the *how*) | Lives across phases |
| `roadmap.md` | Live operational dashboard | Lives across tracks |
| `CLOSURE.md` | v0.1 exit gate | **Terminal**: archives on green |
| `AGENTS.md` | Agent collaboration doctrine | Lives across phases |
| ADR corpus | Binding doctrine | Lives indefinitely |

When the exit gate goes green, `CLOSURE.md` is annotated
`Status: Discharged` and moved to `docs/stale/` (mirroring ADR
lifecycle per GND-0007). A new `CLOSURE.md` is drafted for v0.2 when
v0.2 begins.

---

## 1. v0.1 definition

**Solon v0.1** is the smallest cut of the crate set that delivers on
the agent-first thesis stated in `STORY.md`:

> A set of Rust libraries that lets humans and AI agents collaborate
> on building correct, durable software fast, by encoding enabling
> constraints into Rust types, an event-store substrate, and a
> governance corpus that an agent can query.

Operationally, v0.1 = **Phase 2 v2 exit** as defined in
`docs/c4/roadmap.md`. That is the load-bearing definition; everything
below is index and gate.

The three closure criteria carried forward from Phase 2 v2:

| Id | Criterion | Substance |
|----|-----------|-----------|
| C1 | `adr-srv` operational, read-only | Second non-trivial consumer over the cherry-pit substrate, exposes ADR corpus over GraphQL Query |
| C2 | `gh-report` DDD tactical alignment | Vernon Value Objects, per-aggregate event enums, Tension-2 retirement, Merger ADR |
| C3 | Idiomatic architectural audit clean | Every workspace crate scored against an existing-ADR-derived checklist; remediation beads either closed or `phase:3-harden`-labelled with rationale |

These are the substance of "fully functional set of crates" for v0.1.

---

## 2. Closure inventory

The remaining Phase 2 v2 tracks at the time of this draft. One row
per track; per-task detail lives in `roadmap.md`. CLOSURE.md is the
index; `roadmap.md` is the catalogue.

| Track | Outcome | Depends on | Primary verify |
|-------|---------|------------|----------------|
| 3 (read-only re-scope) | `adr-srv` scrapes ADRs → projection → GraphQL Query | none | `cargo test -p adr-srv --test graphql_read_e2e` |
| 4.4 | `validate.rs` migrated from `gh-report` into `cherry-pit-web` | Track 3 | `cargo test -p cherry-pit-web && cargo test -p gh-report` |
| 5 | SEC-0003 bound in `cherry-pit-web` library surface; bead `adr-fmt-spsd` closes | Track 4.4 | `cargo test -p cherry-pit-web --test sec_0003_enforced_at_library_surface` |
| 8 | C3 idiomaticity audit; checklist committed; per-crate scores; remediations drained or deferred | Track 5 | `bd query --label 'track:8,remediation' --json` non-empty rows all closed or `phase:3-harden`-labelled |
| 10 | Vernon DDD tactical alignment in gh-report: Value Objects, per-aggregate event enums, Tension-2 retirement, Merger ADR | Track 8 | `cargo test -p gh-report` + ADR landings warnings-only |

For per-track task tables, risk registers, and abort criteria see
`docs/c4/roadmap.md` directly.

---

## 3. The closure sequence

Track-level granularity only. Per-task sequencing lives in
`roadmap.md` § "Phase 2 v2 sequencing".

```
3.1         adr-fmt-core lib extraction
  ▼
3.2         adr-srv skeleton
  ▼
3.A         ADR scrape pipeline (first persisted event)
  ▼
3.3         GraphQL Query schema + projection
  ▼
4.4         validate.rs → cherry-pit-web
  ▼
5           SEC-0003 bind-in-library; adr-fmt-spsd closes
  ▼
10          gh-report DDD alignment: VOs, per-aggregate enums, Merger ADR
  ▼
8           C3 idiomaticity audit + remediation + ADR gap-fill
  ▼
EXIT GATE   (see § 4)
```

---

## 4. Exit gate

All items mechanical; all items in CI or runnable from a shell.

**Workspace**:

- [ ] `cargo build --workspace` exit 0
- [ ] `cargo test --workspace --all-features` exit 0
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `cargo run -p adr-fmt -- --lint` warnings-only, no errors

**Trait conformance**:

- [ ] `cargo test --workspace --test '*_conformance'` exit 0 with the
      cherry-pit `EventStore` impl(s) registered
- [ ] `cargo tree` shows **no `async-trait`** anywhere in
      `cherry-pit-*` dependency trees

**C1 — adr-srv read-only**:

- [ ] `cargo run -p adr-srv` starts successfully
- [ ] GraphQL query `{ adr(id: "AFM-0001") { title, references { id } } }`
      returns the scraped + projected ADR with its `References` edges
- [ ] adr-srv's on-disk store contains ≥ 1 event per ADR file under
      `docs/adr/**`
- [ ] Re-running the scrape is idempotent (body_hash skip; zero new
      events on unchanged corpus)

**C2 — gh-report DDD tactical alignment** (see Track 10 below):

- [ ] `cargo test -p gh-report` exit 0
- [ ] SMI invariants preserved across Track 10 changes:
  - [ ] `rg -n 'sequence_tracker|run_index|repo_index|delivery_index' crates/gh-report/src/`
        zero hits

**Track 10 — Vernon DDD tactical alignment**:

- [ ] Value Objects landed in `gh-report` domain
      (`RepoIdentity`, `BatchId`, `Org`, `EventTimestamp`)
- [ ] `DomainEvent` partitioned into `RunEvent` / `RepoEvent` / `WebhookEvent`
      with original discriminants preserved
- [ ] Tension-2 lock retired; multi-projection composition under
      `ProjectionDriver`
- [ ] `cargo test -p gh-report` exit 0
- [ ] Merger-pattern ADR landed under `docs/adr/cherry/`;
      `cargo run -p adr-fmt -- --refs CHE-0054` lists it

**C3 — idiomaticity audit**:

- [ ] Track 8 checklist bead committed with all C3 criteria
- [ ] Every workspace crate has an audit row
- [ ] `bd query --label 'track:8,remediation' --json` — every entry is
      either closed or `phase:3-harden`-labelled with rationale
- [ ] Any audit-finding without an ADR home has a landed draft ADR;
      `cargo run -p adr-fmt -- --lint` warnings-only

**Governance**:

- [ ] `bd query --label story-override --status open --json` returns
      `[]`. Any open `story-override` bead is a release blocker
      (STORY.md § 9)
- [ ] `bd query --label phase:2-generalize --status open --json`
      returns `[]` — no open Phase 2 work

When every checkbox above is green, v0.1 ships. The user ratifies the
Phase 2 v2 → Phase 3 boundary per FOCUS.md § 6.

---

## 5. Out of scope for v0.1

Recorded explicitly so an in-flight closing mission does not absorb
work it should not.

| Out of scope | Deferred to | Why |
|--------------|-------------|-----|
| Adversarial-input fuzz harnesses | v0.2 (Harden) | Correctness under stress is Phase 3; v0.1 establishes baseline |
| Property-based error-path tests at scale | v0.2 (Harden) | Same |
| TLA+ specifications for temporal invariants | v0.2 (Harden) | Specs follow implementation stability |
| Smithy interface contracts | v0.2 (Harden) | Same |
| SEC-0010 (NATS TLS) | v0.2 | Production-deploy concern |
| SEC-0011 (full hash-chain tamper-evidence contract) | v0.2 | Out of v0.1 substrate scope |
| `object_store` backend (CHE-0044) | v0.2 review | Single-region durable file storage is enough |
| Trait-level `ApplicationService<A, S, B>` extraction | v0.2+ | Awaits a second write-side consumer |
| Anti-Corruption Layer at the GitHub edge (gh-report) | v0.2 (§G #20) | CHE-0022 silence on event-payload field removal |
| Vernon strategic DDD (Context Maps) | v0.2+ | Tactical-only in v0.1 |
| `crates.io` publication | **v0.3** | Separate concern; not governed by refinement recipe |
| Public-API freeze, semver commitments, docs.rs polish | **v0.3** | Same |
| Multi-region, multi-tenant, webscale anything | **never** | Out of niche per STORY.md § 3 |

Items deferred to v0.2 or v0.3 are not promises to deliver; they are
acknowledgements that the work is recognised but does not gate v0.1.

---

## 6. Post-v0.1 horizon

Two short paragraphs, for orientation only. Neither is governed by
this document.

**v0.2 — Harden**. Correctness under stress. Fuzz harnesses on every
trust boundary; property suites on error paths; TLA+ for temporal
invariants on the event-store and projection layers; Smithy for the
adr-srv GraphQL schema and any other interface contract. Adversarial
behaviour at the library edges. See FOCUS.md § 4.3 for the standing
recipe.

**v0.3 — Publish**. Public-API freeze, semver commitments, docs.rs
polish, dependency licence audit, release-engineering tooling. Solon
becomes consumable by humans and agents outside this repository. The
v0.3 closure document does not yet exist; it will when v0.3 begins.

---

## 7. Governance of this document

`CLOSURE.md` is **always-escalate** for content changes. Per FOCUS.md
§ 6 (codified during the anchoring step):

- Adding a checkbox tick (i.e. recording a closed gate item) is
  routine; not an escalation. The check is the gate's truth, not a
  decision.
- Changing the exit-gate composition, adding or removing tracks from
  the closure inventory, or moving items between "in scope" and
  "out of scope" is **always-escalate**. User ratifies.
- Declaring v0.1 shipped — i.e. annotating this document
  `Status: Discharged` and archiving to `docs/stale/` — is
  always-escalate. User ratifies.

**ADR edits during closing missions are autonomous-permitted** per
FOCUS.md § 6 "long-autonomous-job exception". The closing missions
in § 2 may draft, amend, supersede, or retire ADRs without per-edit
ratification; moltke's mission-complete report enumerates touched
ADRs for user review. STORY edits, CLOSURE structural edits, and
§ 2 invariant weakening remain always-escalate.

Cross-link discipline: if `roadmap.md` re-shapes a track,
CLOSURE.md's one-line row updates; the per-task table does not
migrate here. CLOSURE.md is an index, never a copy.

---

## 8. Revision history

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 0.1 | 2026-05-19 | acje + agent | Initial draft. v0.1 = Phase 2 v2 exit; mirrors `roadmap.md` exit gate and adds the `story-override` zero-open governance check. |
| 0.2 | 2026-05-19 | acje + agent | § 7 amended: ADR edits during closing missions are autonomous-permitted per FOCUS.md § 6 "long-autonomous-job exception". STORY edits + CLOSURE structural edits + § 2 invariant weakening remain always-escalate. No exit-gate composition change. |
