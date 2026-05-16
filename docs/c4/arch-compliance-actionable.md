# C4 — Architectural Actions (current)

Distilled from `docs/c4/arch-compliance-summary.md` (2026-05-11 batch reviews
+ 2026-05-12 deep cherry-review). This file keeps only items that are
actionable against the current workspace; historical scoreboards, closed
beads, and methodology notes are not repeated. The sibling
`arch-compliance-summary.md` retains the full audit record.

Truth source for bead status:

```
bd query --label remediation --status open
```

Workspace at time of distill: 15 crates in `Cargo.toml` `members`. Note that
`cherry-pit-runtime` and `cherry-pit-storage-primitives` no longer exist;
some bead titles still cite the old names — see §4.

---

## 1. Open remediation beads (workspace code)

Nine beads remain open. All P0/P1 from the original sweep are closed.

| Bead | P | Crate (current) | Action | Closes | Site |
|---|:-:|---|---|---|---|
| `adr-fmt-g66i` | P2 | cherry-pit-gateway | Add `tracing` dependency; instrument `recover_temp_files` + conflict paths | GND-0005 / COM-0019 | `crates/cherry-pit-gateway/src/event_store/msgpack_file.rs` |
| `adr-fmt-ll0i` | P2 | cherry-pit-web | Add `#[non_exhaustive]` to `ErrorBody` (wire-contract struct) | COM-0021:R4 | `crates/cherry-pit-web/src/middleware/error.rs:102` |
| `adr-fmt-gj6z` | P2 | cherry-pit-projection | Replace `u64` with `NonZeroU64` where CHE-0011 mandates the niche-optimised newtype | CHE-0011 | `crates/cherry-pit-projection/src/lib.rs` (see bead body for line cites) |
| `adr-fmt-xqc7` | P3 | cherry-pit-core | Add in-code GND-0004:R2 deviation comment on `AggregateId::into_inner` | GND-0004 | `crates/cherry-pit-core/src/aggregate_id.rs` |
| `adr-fmt-icjo` | P3 | cherry-pit-storage | Either generalize `build_snapshot_signature` (excluded-keys param) or amend CHE-0053:R11 to sanction the `run_timestamp` exclusion | CHE-0053 | `crates/cherry-pit-storage/src/signature.rs:9-23` |
| `adr-fmt-pr22` | P3 | gh-report | Make safe path the default name: rename `next_url` → `next_url_unchecked` (or feature-flag the unverified variant); keep `next_url_same_origin` as `next_url` | COM-0020 safe-path-default | `crates/gh-report/src/github/pagination.rs:18,28` |
| `adr-fmt-3hz6` | P3 | cherry-pit-web | Add proptest + golden fixtures for `/v1/` JSON | CHE-0038:R3/R4 | `crates/cherry-pit-web/` |
| `adr-fmt-k4dj` | P3 | corpus | Resolve lexical collision: CHE-0037 "no snapshot support" vs `FileProjectionStore.snapshot_path` | corpus naming | `docs/adr/cherry/CHE-0037-*.md` |
| `adr-fmt-wi72` | P4 | cherry-pit-agent | Name the GND-0006 backbriefing discipline explicitly in source / AGENTS docs | GND-0006 | `crates/cherry-pit-agent/` + `AGENTS.md` |

Suggested order (all parallelisable):

1. `adr-fmt-ll0i` — trivial attribute add; closes a wire-contract risk.
2. `adr-fmt-xqc7` — comment-only; ~5 LOC.
3. `adr-fmt-pr22` — rename pair; small blast radius if done before more callers land.
4. `adr-fmt-gj6z` — type tightening; verify by `cargo test -p cherry-pit-projection`.
5. `adr-fmt-icjo` — decide policy (generalize vs amend ADR) first, then act.
6. `adr-fmt-g66i` — adds a dep; treat as its own PR.
7. `adr-fmt-3hz6` — testing harness, may be larger.
8. `adr-fmt-k4dj`, `adr-fmt-wi72` — documentation / governance.

---

## 2. ADR-corpus actions

Items from the 2026-05-12 deep review that still apply to the current
corpus. RST-0005 status elevation was in the original recommendation list
but is already `Accepted` (`docs/adr/rust/RST-0005-*.md`) and is omitted.

### 2.1 GND-0005 observability sections — across all 54 CHE-* ADRs

Add an `Observability (per GND-0005:R1)` section to each `docs/adr/cherry/CHE-*.md`
naming the concrete mechanism (clippy lint, compile-fail test, integration
test, metric, log span) by which the ADR's directives are observed at
runtime or build time. Without these, directives drift silently. Suggested
template:

```markdown
## Observability (per GND-0005:R1)
- Type-level invariants: `cargo clippy --all-targets`
- Compile-fail tests: <test name or N/A>
- Integration tests: <test names>
- Runtime signal: <metric / log span / N/A>
```

Track as a single epic; do not file 54 individual beads.

### 2.2 SEC-0011 hash-chain metadata on `EventEnvelope`

`docs/adr/cherry/CHE-0042-*.md` currently defines `EventEnvelope` with no
`previous_hash` / `current_hash` fields, which leaves tampering undetectable
for deployments at SEC-0011's Tier B. Add:

```markdown
R5 [10]: `EventEnvelope` includes `previous_hash` and `current_hash`
  fields per SEC-0011:R1-R2 for tamper-evident deployments.
```

Then implement in `crates/cherry-pit-core/src/` (envelope construction +
storage path).

### 2.3 Foundation-domain cross-references

CHE-* ADRs constrain behaviour that is instantiated from RST, SEC, GND,
COM domains, but `Related` sections frequently omit those references. Audit
pass: for each CHE-* ADR, add the foundation ADRs its constraints derive
from to its `Related` section. Examples already identified:

- CHE-0001 P1 → RST-0005
- CHE-0001 P2 → SEC-0002
- CHE-0002 / CHE-0003 → COM-0017

### 2.4 Contradictions to resolve

| Issue | Source | Resolution path |
|---|---|---|
| CHE-0043 (file-based fencing) vs CHE-0044 (CAS) — no migration path | `docs/adr/cherry/CHE-0043-*.md`, `CHE-0044-*.md` | Document coexistence: `MsgpackFileStore` remains default for single-machine deployments; CAS for multi-writer. |
| CHE-0002 / CHE-0003 acknowledge serialization-boundary tension but neither names the validation contract | `docs/adr/cherry/CHE-0002-*.md`, `CHE-0003-*.md` | Add to CHE-0003:R1: "except at serialization boundaries where runtime validation is mandated (see SEC-0002:R1-R3)". |
| CHE-0035 `scc::HashMap` lock registry grows monotonically — memory leak for high aggregate churn | `docs/adr/cherry/CHE-0035-*.md` | Extend CHE-0035 with pruning strategy or documented memory bounds. |

---

## 3. Watch-items (not yet beads)

Patterns observed but not severe enough to file. Promote to bead on
recurrence.

- **`cherry-pit-gateway` naming vs content.** The crate currently holds
  `MsgpackFileStore` — an `EventStore` adapter, not a `CommandGateway`.
  Confirmed still present at
  `crates/cherry-pit-gateway/src/event_store/msgpack_file.rs`. Planning-
  level governance question; resolve via ADR before any new gateway
  consumer locks the name in.
- **`#[non_exhaustive]` discipline on wire-contract structs.** Public enums
  in core / storage / web consistently carry it; structs that form wire
  contracts have lagged (only `ErrorBody` found so far, tracked by
  `adr-fmt-ll0i`). Watch for new public structs in `cherry-pit-web` and
  `gh-report`.

---

## 4. Stale bead metadata to fix

Two open beads carry crate names that no longer exist in the workspace.
Update titles / labels before working them:

| Bead | Stale crate name | Current crate |
|---|---|---|
| `adr-fmt-pr22` | cherry-pit-runtime | gh-report (`crates/gh-report/src/github/pagination.rs`) |
| `adr-fmt-icjo` | cherry-pit-storage-primitives | cherry-pit-storage (`crates/cherry-pit-storage/src/signature.rs`) |

```
bd update adr-fmt-pr22 --title "gh-report: invert next_url naming (COM-0020 safe-path-default)"
bd label remove adr-fmt-pr22 crate:cherry-pit-runtime
bd label add    adr-fmt-pr22 crate:gh-report

bd update adr-fmt-icjo --title "cherry-pit-storage: generalize build_snapshot_signature or amend CHE-0053:R11"
bd label remove adr-fmt-icjo crate:cherry-pit-storage-primitives
bd label add    adr-fmt-icjo crate:cherry-pit-storage
```

---

## 5. Where the remaining work fits into the roadmap

Mapping each open item to `FOCUS.md` § 4 / `docs/c4/roadmap.md`. Roadmap
state at time of mapping (per roadmap.md):

- Phase 1 (Cleanup) — **complete** (2026-05-14).
- Phase 2 v2 (Generalize) — **active**; Tracks 0 + 0.5 + 1 + 2 closed;
  Track 4 mid-flight; Tracks 3 + 4.3 + 4.4 + 5 + **6 (Cleanup)** remaining.
- Phase 3 (Harden) — **not started**.

Track 6 was added to Phase 2 v2 specifically to absorb the residual
P2/P3/P4 remediation cohort + ADR-corpus sweeps in this document; it
gates on Track 5 and runs as the closing track of Phase 2. See
`docs/c4/roadmap.md` § "Track 6 — Cleanup".

By doctrine (`FOCUS.md` §4.4 task-injection rules), each open item routes
to the phase matching its **nature**, not its discovery date:
cleanup → 1 (closed; absorbed into Phase 2 v2 Track 6 as the new cleanup
slot), generalize → 2, harden → 3. Bead label suffix listed in the
"Phase label" column is the one to apply with
`bd label add <id> phase:<n>-<name>`.

### 5.1 Workspace-code beads (§1)

| Bead | Phase | Track / slot | Rationale |
|---|---|---|---|
| `adr-fmt-ll0i` (web `ErrorBody` `non_exhaustive`) | 2-generalize | Track 6.1 (Cleanup) | Wire-contract hygiene; small attribute add. Closes one of the deep-review CC-4 findings. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-xqc7` (core `AggregateId::into_inner` GND-0004:R2 comment) | 2-generalize | Track 6.5 (Cleanup) | Comment-only, on a core public API; matches Track-6 hygiene remit. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-pr22` (gh-report `next_url` rename) | 2-generalize | Track 6.2 (Cleanup) | Safe-path-default rename in `crates/gh-report/src/github/pagination.rs`. Run the §4 stale-bead-metadata fix before working. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-gj6z` (projection `u64` → `NonZeroU64`) | 2-generalize | Track 6.4 (Cleanup) | Type tightening on `cherry-pit-projection` aggregate-id parameters. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-icjo` (storage `build_snapshot_signature` policy) | 2-generalize | Track 6.6 (Cleanup) | Policy choice: generalize (code) vs amend CHE-0053:R11 (ADR — high-risk per FOCUS §6). Run the §4 stale-bead-metadata fix before working. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-g66i` (gateway `tracing` instrumentation) | 2-generalize | Track 6.3 (Cleanup) | Observability gap on the gateway file-store path; brings parity with cherry-pit-web telemetry. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-3hz6` (web `/v1/` proptest + golden fixtures) | 3-harden | Phase 3 task 2 — cherry-pit-web adversarial-input harness | Maps directly onto Phase 3 task 2 (fuzz harness for cherry-pit-web HTTP surface) and task 5 (error-path property tests). NOT Track 6 — adversarial-input testing is Phase 3 by FOCUS classification. Label: `phase:3-harden`. |
| `adr-fmt-k4dj` (CHE-0037 vs `snapshot_path` lexical collision) | 2-generalize | Track 6.8 (Cleanup, ADR sweep) | CHE edit → high-risk per FOCUS §6 (user ratification). Bundle with 6.9 / 6.10 / 6.11 into one ratification round. Label: `phase:2-generalize,track:6`. |
| `adr-fmt-wi72` (cherry-pit-agent GND-0006 naming) | 2-generalize | Track 6.7 (Cleanup) | Module-level rustdoc + `AGENTS.md` cross-ref. Label: `phase:2-generalize,track:6`. |

### 5.2 ADR-corpus actions (§2)

| Action | Phase | Track / slot | Rationale |
|---|---|---|---|
| 2.1 — GND-0005 observability sections, all 54 CHE-* ADRs | 2-generalize | Track 6.9 (Cleanup, ADR sweep) | Mechanical sweep; one user-ratification round (FOCUS §6 high-risk for ADR edits). Many of the named observation mechanisms (clippy, compile-fail, conformance tests from Track 1.3, falsifier tests from Phase-2 v1 task 6) already exist — wire them in. Label: `phase:2-generalize,track:6`. |
| 2.2 — SEC-0011 hash-chain on `EventEnvelope` (CHE-0042) | 3-harden | Phase 3 task 11 — SEC-0010 / SEC-0011 closure | Roadmap Phase 3 task 11 owns SEC-0011 disposition; Track 0.5 research also flagged opt-in `HashChainedEventStore` extension as a possible Phase-3 head-start. Hold the CHE-0042 amendment until task 11 activates. Label: `phase:3-harden`. |
| 2.3 — Foundation-domain cross-references on CHE-* `Related` sections | 2-generalize | Track 6.10 (Cleanup, ADR sweep) | Same nature as 2.1; bundle into the same ratification round. Label: `phase:2-generalize,track:6`. |
| 2.4a — CHE-0043 ↔ CHE-0044 fencing migration path | 3-harden | Phase 3 task 11 / CHE-0044 review | Roadmap §7 keeps CHE-0044 object_store backend deferred to Phase 3; the migration-path ADR amendment belongs in that same review. Coordinate with existing injection-queue bead `adr-fmt-9b4n` (CHE-0043 vs CHE-0053 flock-mandate). Label: `phase:3-harden`. |
| 2.4b — CHE-0002 / CHE-0003 serialization-boundary clause | 2-generalize | Track 6.11 (Cleanup, ADR sweep) | Single-line addendum to CHE-0003:R1; bundle into the Track-6 ratification round. Label: `phase:2-generalize,track:6`. |
| 2.4c — CHE-0035 lock-registry pruning / bounds | 3-harden | Phase 3 task 6 — resource-bound enforcement | Roadmap Phase 3 task 6 already targets resource-bound enforcement; lock-registry growth is in the same family. Label: `phase:3-harden`. |

### 5.3 Watch-items (§3)

| Watch-item | Phase | Slot |
|---|---|---|
| cherry-pit-gateway naming vs content (MsgpackFileStore) | 2-generalize | Track 4.2 governance question | When Track 4.2 inventories "category (a) reusable upstream" layers, settle whether `MsgpackFileStore` stays in `cherry-pit-gateway` or relocates (e.g. to a new `cherry-pit-eventstore` crate). Does not need a bead until Track 4.2 starts. |
| `#[non_exhaustive]` on wire-contract structs | 2-generalize → 3-harden | Track 6.1 + Phase-3 fuzz pre-check | After Track 6.1 (`adr-fmt-ll0i`) closes, add a workspace clippy allow-list or small `adr-fmt` rule that flags public structs without `#[non_exhaustive]` in wire-contract modules. Promote to bead on the first new instance found. |

### 5.4 Net mapping summary

| Roadmap slot | Open beads landing here | ADR actions landing here |
|---|---|---|
| Phase 2 v2 — Track 6 (Cleanup) workspace-code | `adr-fmt-ll0i`, `adr-fmt-pr22`, `adr-fmt-g66i`, `adr-fmt-gj6z`, `adr-fmt-xqc7`, `adr-fmt-icjo`, `adr-fmt-wi72` | — |
| Phase 2 v2 — Track 6 (Cleanup) ADR-sweep round | `adr-fmt-k4dj` | 2.1 GND-0005 sections; 2.3 foundation cross-refs; 2.4b CHE-0003 boundary clause |
| Phase 3 task 2 / 5 (adversarial input + error-path) | `adr-fmt-3hz6` | — |
| Phase 3 task 6 (resource bounds) | — | 2.4c CHE-0035 pruning |
| Phase 3 task 11 (SEC-0010/11 + CHE-0044 review) | — | 2.2 SEC-0011 hash-chain on CHE-0042; 2.4a CHE-0043↔CHE-0044 migration |

### 5.5 What is NOT actionable from this file right now

- Nothing here is a Phase-1 (Cleanup) item — that phase is closed; the
  cleanup nature is now absorbed by Phase 2 v2 Track 6.
- Track 6 is **gated on Track 5**; do not pre-empt Tracks 3 / 4.3 / 4.4 / 5
  with Track-6 work. Items 6.1–6.7 are individually trivial enough that
  exceptions can be made when an active Track-3/4/5 mission already
  touches the same surface — file as inline "hygiene-while-touched" with
  the same Track-6 bead.
- Track 6 ADR sweeps (6.8–6.11) are high-risk per `FOCUS.md` §6 and
  require user ratification. Bundle into **one** ratification round
  (recommended), not four.
- Phase 3 items in §5.1 / §5.2 / §5.3 do not move into Track 6 even though
  some are small — adversarial-input testing, hash-chain semantics, and
  resource-bound enforcement are Phase-3 by nature per FOCUS §4.3.
