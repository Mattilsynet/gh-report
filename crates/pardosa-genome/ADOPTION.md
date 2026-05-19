# Adopting `pardosa-genome` — Doctrine, Migration, Case Study

Forward-facing adoption guide for Rust crates wiring their `DomainEvent`
closure through `#[derive(GenomeSafe)]`. Distils the binding rules from
the GEN-domain ADRs and the migration patterns proved by the `gh-report`
adoption (mission `pardosa-genome-adoption-schism`).

This is **derived doctrine**. Authoritative rules live in the GEN ADRs;
this document is the prescriptive distillation a future adopter reads
first.

## Provenance

The catalog and case-study sections are derived from three oracle
evidence beads filed during the gh-report adoption:

| Bead | Scope | Contents |
|---|---|---|
| `adr-fmt-l1qx0` | β.1 | `pardosa-derive` rejection-rule catalog — the 15 rules enforced by `#[derive(GenomeSafe)]` (12 EVT-NNN diagnostics, GEN-0035:R4 enum well-formedness, GEN-0045:R4 Arc/Cow absence, EVT-001 union rejection), each with source pointer and canonical migration. |
| `adr-fmt-psj0q` | β.2 | `DomainEvent` closure type-by-rule matrix — every type reachable from `gh-report`'s `DomainEvent` (22 concrete types) checked against the 15 rules. |
| `adr-fmt-1j0vy` | α′ | Replay-as-rebuild oracle — the 19-ADR constraint set behind retiring on-disk projection caches in favour of event-log replay; precursor to CHE-0048 amendment (δ.0) and CHE-0022:R6 (δ.4). |

Authoritative GEN ADRs: see `cargo run -p adr-fmt -- --tree GEN`. Key
anchors used below: GEN-0001 (`GenomeSafe` marker), GEN-0004 (reject
non-deterministic types and serde attributes), GEN-0019 (Box/Arc hash
transparency; Rc exclusion), GEN-0029 (reject `#[serde(default)]`),
GEN-0035 (in-house canonical encoding, `#[repr(u8)]` requirement),
GEN-0045 (idiomatic event payload types).

## The 15 Rules

Each rule is MUST / MUST-NOT / SHOULD per RFC 2119. Citations point to
the originating GEN R-clause or `pardosa-derive` diagnostic. Migration
patterns are in § Migration Patterns; full source pointers live in
bead `adr-fmt-l1qx0`.

### 1. MUST NOT derive `GenomeSafe` for a `union` (EVT-001)

Unions have no canonical wire shape; serde cannot represent them.
Replace with `enum` (tagged sum) or `struct` (product).

### 2. MUST NOT use `HashMap<K, V>` in event payloads — use `BTreeMap<K, V>` (EVT-002, GEN-0004)

`HashMap` iteration order is non-deterministic, breaking byte-stable
encoding. Recurses through nesting (`Vec<HashMap<...>>`,
`Option<HashMap<...>>`, etc.). Migration requires `K: GenomeOrd + Ord`.

### 3. MUST NOT use `HashSet<T>` in event payloads — use `BTreeSet<T>` (EVT-003, GEN-0004)

Same rationale as rule 2; `T: GenomeOrd + Ord` required.

### 4. MUST NOT use `usize` in event payloads — use `u32` or `u64` (EVT-004, GEN-0004)

`usize` is target-arch-dependent (32-bit vs 64-bit). The wire format
is fixed; the source language type must be too. Convert at the
boundary; `u64` is the safe default. (See COM-0023 if applicable.)

### 5. MUST NOT use `isize` in event payloads — use `i32` or `i64` (EVT-005, GEN-0004)

Same rationale as rule 4. `i64` is the safe default.

### 6. MUST NOT use `#[serde(flatten)]` anywhere on a `GenomeSafe` type (EVT-006, GEN-0004)

Flattening switches serde from `serialize_struct` to `serialize_map`,
breaking the fixed-layout contract. Inline the flattened fields or
wrap in a nested named struct.

### 7. MUST NOT use `#[serde(untagged)]` on an enum (EVT-007, GEN-0004)

Untagged enums bypass variant serialisation — silent data corruption
on a fixed-layout wire. Use externally tagged enums (serde default —
the discriminant byte is emitted).

### 8. MUST NOT use `#[serde(default)]` anywhere on a `GenomeSafe` type (EVT-008, GEN-0029)

The genome wire format always carries every field; `#[serde(default)]`
is silently inert and would mislead future maintainers. Encode
optionality explicitly via `Option<T>` instead.

### 9. MUST NOT use `#[serde(tag = "…")]` (internally tagged enum) (EVT-009, GEN-0004)

Only externally tagged enums are compatible with fixed
discriminant-based layout. Drop `tag`; use the serde default.

### 10. MUST NOT use `#[serde(content = "…")]` (adjacently tagged enum) (EVT-010, GEN-0004)

Same restriction as rule 9 (the adjacent-tag pair is not
discriminant-compatible). Drop `tag` + `content`; use the serde
default.

### 11. MUST NOT use `#[serde(skip_serializing_if = "…")]` (EVT-011, GEN-0004)

Conditional field omission breaks fixed-layout serialisation. Encode
optionality via `Option<T>` so the wire shape is fixed; the wire
position is always written, the value differentiates `Some`/`None`.

### 12. MUST NOT use raw pointers (`*const T`, `*mut T`) in event payloads (EVT-012, GEN-0004)

Raw pointers have no canonical wire representation. Replace with
owned `T` (or `Box<T>` if heap shape matters). If the pointer is
structural-only, lift it out of the `GenomeSafe` closure into the
application layer.

### 13. MUST NOT use function pointers (`fn(..) -> ..`) in event payloads (EVT-013, GEN-0004)

Function pointers carry process-local addresses with no portable wire
form. Encode the action as data — e.g. an enum tag dispatched at the
use site.

### 14. MUST give every `enum` in the closure `#[repr(u8)]` with explicit integer-literal discriminants (GEN-0035:R4)

Canonical `[discriminant:u8]` encoding requires statically-known `u8`
bytes at derive time. The validator (`pardosa-derive/src/reject.rs`)
rejects: (a) missing `#[repr(u8)]`, (b) any variant without an
explicit discriminant literal, (c) discriminant expressions that are
not integer literals. Surfaces as an un-coded `syn::Error` rather
than an EVT-NNN diagnostic; the substance is the same.

### 15. MUST NOT use `Arc<T>`, `Cow<'_, T>`, or `Rc<T>` in event payloads — use owned `T` (GEN-0045:R4, GEN-0019)

Three distinct reasons for the same prescription:

- **`Arc<T>`** — shared ownership does not survive serialisation;
  decoding always allocates a fresh `Arc`, so the impl would be
  semantically misleading. Enforced by deliberate **absence** of a
  blanket `GenomeSafe for Arc<T>` impl in
  `pardosa-genome/src/genome_safe.rs`; surfaces as `E0277 the trait
  bound 'Arc<T>: GenomeSafe is not satisfied'`.
- **`Cow<'_, T>`** — the `Borrowed` variant is a reference and
  cannot survive a storage round-trip; same blanket-impl absence.
- **`Rc<T>`** — `!Send`, incompatible with async runtimes
  (Tokio / Axum). Enforced by the same blanket-impl absence in
  `pardosa-genome/src/genome_safe.rs` (see GEN-0019); has no
  EVT-NNN code and currently no dedicated GEN-NNN rule, but the
  rejection is real and the migration is identical.

Migration: use the owned inner type `T`. If shared ownership is
required at runtime only, lift the `Arc`/`Rc`/`Cow` outside the
`GenomeSafe` closure (keep it in the application layer; store
owned `T` in the event payload).

### Footnote on rule count

The catalog is **15 rules** as enforced by `#[derive(GenomeSafe)]` at
the time of this writing. EVT-001 through EVT-013 (13 EVT-NNN
diagnostics), GEN-0035:R4 (enum well-formedness, currently un-coded),
and GEN-0045:R4 (Arc/Cow blanket-impl absence) sum to 15. The
`Rc<T>` rejection is folded into rule 15 because the migration is
identical to Arc; it may earn its own rule_id in a future GEN
amendment.

`#[serde(rename)]` and `#[serde(rename_all)]` are **not** rejected by
`pardosa-derive` (see `reject.rs:96-103`). Borrowed types (`&str`,
`&[u8]`) are rejected by missing blanket impl rather than by an
EVT-NNN diagnostic; they share the GEN-0045 family but lack a
specific sub-clause.

## Migration Patterns

Concrete before/after for the patterns exercised during the gh-report
adoption. All examples assume `use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};`.

### `HashMap` → `BTreeMap`

```rust
// Before — rejected by EVT-002:
#[derive(GenomeSafe)]
struct PerOwner { totals: HashMap<String, u64> }

// After:
#[derive(GenomeSafe)]
struct PerOwner { totals: BTreeMap<String, u64> }
```

The key type must additionally satisfy `GenomeOrd + Ord` (GEN-0033).

### Nested `HashMap<K, Vec<V>>` → owned-key flatten

```rust
// Before — rejected by EVT-002 at outer position:
#[derive(GenomeSafe)]
struct AlertsByOwner { by_owner: HashMap<String, Vec<Alert>> }

// After — option A: BTreeMap retains the map shape with deterministic order:
#[derive(GenomeSafe)]
struct AlertsByOwner { by_owner: BTreeMap<String, Vec<Alert>> }

// After — option B: flatten to a Vec of (key, value) pairs when ordering
// is established externally (e.g. by an aggregate-supplied iteration):
#[derive(GenomeSafe)]
struct AlertsByOwner { entries: Vec<OwnerAlerts> }

#[derive(GenomeSafe)]
struct OwnerAlerts { owner: String, alerts: Vec<Alert> }
```

Option B is preferred when the consumer never queries by key — `BTreeMap`
serialises as a sorted sequence anyway, so the only difference is
in-memory access shape.

### `Arc<T>` → owned `T` (lift sharing to caller)

```rust
// Before — rejected by GEN-0045:R4 (blanket-impl absence):
#[derive(GenomeSafe)]
struct Sweep { evidence: Arc<RepositoryEvidence> }

// After — event payload owns the value; runtime sharing lives in the
// application layer outside the GenomeSafe closure:
#[derive(GenomeSafe)]
struct Sweep { evidence: RepositoryEvidence }

// Runtime callers that previously shared via Arc now hold their own
// owned copy; if a single in-memory representation is required across
// many sites, the sharing wrapper lives in the application layer:
fn collect(...) -> Arc<RepositoryEvidence> {
    let owned: RepositoryEvidence = decode_event(...);
    Arc::new(owned)
}
```

`Rc<T>` and `Cow<'_, T>` follow the same shape: replace with owned `T`.

### Bare `enum` → `#[repr(u8)]` with explicit discriminants

```rust
// Before — rejected by GEN-0035:R4:
#[derive(GenomeSafe)]
enum Status { Pending, Active, Done }

// After:
#[derive(GenomeSafe)]
#[repr(u8)]
enum Status {
    Pending = 0,
    Active  = 1,
    Done    = 2,
}
```

Every variant carries an explicit integer literal. Renaming a variant
is wire-compatible; **renumbering** a variant is a wire break.

### `usize` at the boundary → `u64`

```rust
// Before — rejected by EVT-004:
#[derive(GenomeSafe)]
struct Batch { count: usize }

// After — fixed-width at the wire; convert at the boundary:
#[derive(GenomeSafe)]
struct Batch { count: u64 }

let api_count: usize = items.len();
let event = Batch { count: u64::try_from(api_count).expect("...") };
```

Use `u32` only when the bound is statically known to fit (e.g. enum
discriminants). `u64` is the safe default for collection counts /
timestamps / lengths. (`isize` → `i64` analogously.)

### `#[serde(default)]` + `#[serde(skip_serializing_if)]` → `Option<T>`

```rust
// Before — rejected by EVT-008 + EVT-011:
#[derive(GenomeSafe)]
struct RepoEvaluated {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<Box<RepositoryEvidence>>,
}

// After — Option<T> is already the canonical encoding of optionality;
// the wire position is always written, the discriminant byte differentiates
// Some/None:
#[derive(GenomeSafe)]
struct RepoEvaluated {
    evidence: Option<Box<RepositoryEvidence>>,
}
```

The attributes were silently inert under the genome format (every
field is on the wire); removing them changes nothing on the wire and
makes the source honest.

## gh-report Case Study

Two named insights surfaced during the gh-report adoption. Both were
load-bearing enough to motivate ADR amendments in the same mission.

### Insight 1 — Computed aggregates in event payloads are a category error

The pre-adoption `gh-report` design carried per-owner roll-ups,
denormalised joins, and metric snapshots inside event variants.
Tracing the `DomainEvent` closure (bead `adr-fmt-psj0q`) revealed
that **these types were not in the closure at all** — they lived
under the `Evidence` aggregate, which is the read-side artefact.
The closure was 22 types of raw single-aggregate state; the
"problematic" computed types were only ever read-side.

The principle: events carry raw observed signals within a single
aggregate's scope. Derived views (summaries, counts, rollups,
cross-aggregate joins) are reconstructed by replaying the event log
into a projection, and live only there. Persisting a computed
aggregate inside an event payload creates a parallel truth that can
drift from the replayed projection.

**Codified by δ.4 as CHE-0022:R6** (commit `e81adfb`): "Event
payloads MUST NOT carry computed aggregates — summaries, counts,
rollups, cross-aggregate joins. … Derived state is reconstructed by
replay (CHE-0051:R5) and persists, if at all, only as a CHE-0048
projection checkpoint — never inside an event."

Adopter checklist:

- For each candidate event payload field, ask: "Is this a raw signal
  scoped to this aggregate, or a view computed across aggregates?"
- If the latter — it belongs in a projection, not in the event.
- If unsure — trace the closure. The type may not be in the closure
  at all (as was the case for several `gh-report` metric types).

### Insight 2 — Replay-as-rebuild eliminates on-disk projection caches

The pre-adoption `gh-report` design persisted two derived artefacts:
`baseline.msgpack` (yesterday's evidence, used to short-circuit
re-evaluation) and `<run>-checkpoint.msgpack` (in-flight sweep
state, used to resume on crash). Both encoded a parallel truth that
could drift from the event log.

Under Interpretation A (preserve algorithms, retire only the
persistence surface), both collapse into a single dimension: "what
does the projection say about this `inventory_key`?" The projection
is rebuilt at boot via event-log replay (CHE-0051:R5); the
`should_reuse` check then runs against projection state directly.
The on-disk artefacts are removed; their algorithms (the
`should_reuse` filter, the `is_total_failure` check, the
`snapshot_signature` invalidator) survive unchanged inside the saga.

Proof of concept landed across two commits:

- `30505fc` (δ.3c-i) — wire `SweepStarted::snapshot_signature` so
  replay can identify the org-alert snapshot a sweep was scoped to.
- `63236ac` (δ.3c-ii) — retire `infra/baseline.rs` + `infra/checkpoint.rs`
  on-disk surfaces; pivot `reuse_from_baseline` / `warm_start_from_baseline`
  to read from the projection; rewrite `--dump-baseline` as
  replay-and-dump-projection.

Net effect: −1554 / +435 lines, 13 files, all inside `crates/gh-report/`;
zero new cross-crate API surface.

Doctrine references:

- **CHE-0048** — projection checkpoint topology (the durable artefact
  pattern for derived state, when one is needed at all).
- **CHE-0051:R5** — replay-past-checkpoint driver (the reconstruction
  mechanism).
- **CHE-0022:R6** — codifies the rule that motivates this pattern
  (δ.4 amendment, see Insight 1).

Adopter checklist:

- Map every on-disk artefact in the crate. For each: is it the event
  log (durable truth) or a derived cache (rebuildable)?
- For each derived cache: can it be reconstructed by replay? If yes,
  consider retirement; if no, it belongs in the projection
  checkpoint surface per CHE-0048, not in a parallel file.
- Algorithms (filters, invalidators, ordering rules) are usually
  separable from the persistence surface. Retire the file; keep the
  algorithm in the saga or projection.

## See also

- GEN-domain ADRs: `cargo run -p adr-fmt -- --tree GEN`.
- Cherry-pit ADRs cited above: CHE-0022, CHE-0048, CHE-0051,
  CHE-0064, CHE-0065. Browse via `cargo run -p adr-fmt -- --refs CHE-NNNN`.
- Oracle beads behind this document: `adr-fmt-l1qx0`, `adr-fmt-psj0q`, `adr-fmt-1j0vy`.
