# Clippy policy — pinned toolchain 1.98.0

> **DEFERRED WIP — NOT ACTIVE (ghr-ktvy5, checkpoint 1c).** The five-group
> `[workspace.lints.clippy]` table this document describes was removed from the
> root `Cargo.toml`; the active baseline is `pedantic = warn` only, and the
> 214 migration-only `#[expect(...)]` blocks written for the deferred groups
> have been retired from source. Every disposition, pilot-hit count and witness
> below therefore describes a CANDIDATE policy, not the current root. These
> files are intentionally untracked/unstaged WIP. Do not cite them as the
> shipped bar and do not run the fixture harness against the current root.

Status: **audit checkpoint, not enabled.** This document records a complete
active-lint roster with provisional dispositions, and the bounded pilot
diagnostics for epic `ghr-7qm12`. It is **not** a completed semantic audit.
**Current status (authoritative, 2026-09-18)** — superseding the per-section
figures below, which are retained as historical audit data: the manifest
*has* changed (`Cargo.toml:233-290`, five groups plus 47 justified `allow`s);
`tools/clippy-fixtures/run.sh` asserts **58** checks at exit 0 under native
Cargo lint resolution;
**seven** members are now
verified green under `cargo aclippy -p <name> --all-targets -- -D warnings`
(exit 0), superseding the earlier two-member figure:
`cherry-pit-core`, `cherry-pit-storage`, `cherry-pit-projection`,
`cherry-pit-wq`, `cherry-pit-web` (with `cherry-pit-web/projection`),
`cherry-pit-app` and `pardosa-cherry-pit-test-support`. The
remaining members, the full workspace/`--all-features` boundary, the sibling
repositories, and the full semantic/dormant/deprecated catalogue audit are
**unfinished and unverified** — no boundary-green claim is made. Both
`blanket_clippy_restriction_lints` and `struct_excessive_bools` ratchets are
**ACCEPTED** (RST-0003:48-54), not open. The epic remains **unshippable**
until workspace-wide green; do not weaken CI to claim success.

**Durability paths (`ghr-pkdev`, decision `ghr-7qm12.34`).** In
`crates/gh-report/src/app/state/mod.rs`, `record_team` is **implemented under
explicit authorization**: a primary append failure now returns a typed
`PersistenceError` and skips the dedicated append and projection; a dedicated
failure preserves the primary write and publishes no projection. Covered by
**5 tests** (`cargo atest -p gh-report record_team` → exit 0). No atomicity
is claimed. **Two paths remain unauthorized and unchanged**: `detach_team`
and `record_org` still discard the primary write's result while a dedicated
success returns `Ok`. Applying the same policy to both is *recommended* but
awaits user authorization under `ghr-7qm12.34`; no source behaviour change and
no global `must_use` waiver is made for them here. The secondary
deletion-contract question (whether a dual-store success is permitted when one
store's delete fails) remains an **evidence gap**: the existing ADRs do not
settle it, and it is not a proven blocker.

**Test-run caveat.** Targeted and scoped suites pass (per-package `atest`,
`record_team`, web+projection, app, projection/wq/test-support, and
`tools/clippy-fixtures/run.sh` 58 assertions — all exit 0). One **combined
seven-package** `cargo atest` run hit a 120 s shell timeout during
`cherry-pit-wq`'s lib target (SIGTERM). That is **neither a pass nor a test
failure verdict**; root cause is unknown and the run is retained as an open
follow-up. Do not read the scoped passes as full-boundary green.

Accuracy corrections from review `ghr-cgtt6` (M1–M4, L1–L3) are applied
throughout this revision; superseded figures and claims are marked withdrawn
where they appeared.

Toolchain: `clippy 0.1.98 (88d9e12ae1 2026-08-18)`, pinned by
`rust-toolchain.toml:2`. `clippy.toml:1` sets `msrv = "1.98"`.

## 1. Mechanism (approved bounds, ghr-7qm12)

```toml
[workspace.lints.clippy]
all         = { level = "warn", priority = -1 }
pedantic    = { level = "warn", priority = -1 }
nursery     = { level = "warn", priority = -1 }
restriction = { level = "warn", priority = -1 }
cargo       = { level = "warn", priority = -1 }
# individual exclusions at priority 0, section 4
```

Group levels and individual allows are native Cargo/rustc/Clippy configuration.
No `clippy.toml` entry is required for this mechanism, which keeps the sibling
prohibitions (`adr-fmt/AGENTS.md:24-30`, `comment-free/AGENTS.md:32-40`) intact.

## 2. Catalogue equality — independently reproducible

Single native command, no project build, no custom lint framework:

```
rustup run 1.98.0 clippy-driver -W help
```

| Fact | Value |
|---|---|
| Active lints listed | 822 |
| Default levels | 333 allow, 422 warn, 67 deny |
| `all` | 489 |
| `pedantic` | 143 |
| `nursery` | 53 |
| `restriction` | 132 |
| `cargo` | 5 |
| `all` decomposition | correctness 67 + suspicious 83 + style 158 + complexity 143 + perf 38 = 489 |
| Union of the five policy groups | **822** |
| Lints in catalogue but in no group | **0** |
| Lints in a group but absent from catalogue | **0** |

The five approved groups therefore cover the entire active catalogue exactly;
group coverage is not an allowlist and leaves no ungrouped remainder. These
counts agree with the independently obtained official-catalogue figures in
`ghr-shwvt` (839 documented entries − 17 deprecated = 822 active).

## 3. Complete active inventory with provisional dispositions

This section is a **complete active-lint roster with a provisional disposition
assigned to every entry**. It is *not* a completed semantic audit. Assigning a
disposition is a routing decision; it is not per-lint evidence of
compatibility, dormant-configuration behaviour, meaningful-domain
exhaustiveness, or interaction with other retained lints. Semantic assessment
is tracked independently of pilot hit count (§6.9) and is open for every entry
below, including the `retained` ones.

`docs/clippy/catalogue-1.98.tsv` carries one row per active lint:
`lint, default_level, groups, disposition, pilot_hits`.

All `pilot_hits` values in the TSV, and every count quoted in §4, are **raw
diagnostic records** from the baseline pilot run (§5). Figures elsewhere in
this document — notably the deduplicated and differential figures in §5.1–§5.3
— name their own unit and are not raw records; see §5.1.

| Disposition | Count | Meaning |
|---|---|---|
| `retained` | 721 | Warn under the group level; zero diagnostics in the bounded pilot. Zero hits is **absence of pilot evidence**, not evidence of compatibility. |
| `retained-scope-pending` | 59 | Warn is intended, but the pilot shows live diagnostics whose scope (workspace vs test-only vs per-crate) is undecided |
| `excluded` | 42 | Individual `allow` at priority 0, justified in section 4 |

Restriction group specifically — all 132 carry a provisional disposition, none
left undispositioned (assignment complete; semantic assessment not):

| Disposition | Count |
|---|---|
| `retained` | 48 |
| `retained-scope-pending` | 43 |
| `excluded` | 41 |

The 42nd exclusion, `blanket_clippy_restriction_lints`, is not a restriction
member; it sits in `all,suspicious` and is **warn by default**. It fires
*because of* the approved mechanism — the pilot emitted
``warning: `clippy::restriction` is not meant to be enabled as a group``
**32 raw diagnostic records in each saved run** (baseline and exempt alike);
the commands and dedup key used here do not establish why there are 32, and
compilation-unit provenance is absent from the retained data. It is the **only** exclusion in the roster that is not
default-`allow`, and therefore the only one that weakens enforcement already
in effect; see section 4.6 for its ratchet treatment.

## 4. Exclusions — individual justification (provisional)

Every exclusion names its priority conflict per the fleet hierarchy
(1 maintainability, 2 correctness by design, 3 response time, 4 energy,
5 features) and the prior enforcement it affects.

**All exclusions in this section are provisional.** Each states the scope it
is actually justified at (workspace / library-only / binary-only / enum-only).
Where the recorded rationale is narrower than a workspace-wide `allow`, that
gap is named inline and the library/binary scoping decision is deferred to the
§6.3 scope checkpoint — it is **not** discharged here.

### 4.1 Mechanism consequence

| Lint | Conflict | Prior enforcement |
|---|---|---|
| `blanket_clippy_restriction_lints` | Fires on the approved mechanism itself. Retaining it would make the approved configuration self-reporting noise. | **Warn by default (`all,suspicious`) — currently in effect.** This is a genuine ratchet weakening; see 4.6. |

### 4.2 Direct contradiction with an enforced local invariant

| Lint | Conflict | Prior enforcement |
|---|---|---|
| `question_mark_used` | Rejects `?`; contradicts warn-by-default `clippy::question_mark` and the approval's explicit "idiomatic `?` propagation remains permitted". | New; contradicts an existing warn-by-default lint. |
| `missing_docs_in_private_items` | Contradicts the contract-only documentation policy (`AGENTS.md` House style — Rust comments): docs are written for public/unsafe contracts, not private items. | New. |
| `undocumented_unsafe_blocks` | Requires `// SAFETY:` comments; `comment-free/AGENTS.md:14-16` and the fleet no-plain-comment rule forbid exactly that carve-out. | New. |
| `exhaustive_enums`, `exhaustive_structs` | Demand `#[non_exhaustive]`. The recorded invariant is the **closed ERROR enum** policy at `AGENTS.md:288-292`; that justifies the `exhaustive_enums` exception directly. It does **not** by itself justify `exhaustive_structs`, whose exclusion rests on the weaker claim that no struct in this workspace is a published-stability surface — unverified here and deferred to §6.3. | New. Enum arm: justified. Struct arm: **provisional, rationale gap named.** |
| `implicit_return` | Demands explicit `return` in tail position; directly inverts warn-by-default `clippy::needless_return`. 5811 pilot diagnostics. | Contradicts an existing warn-by-default lint. |

### 4.3 Mutually contradictory pairs (both members excluded)

Retaining either member is a coin-flip with no priority basis; retaining both
is incoherent. Both are excluded and the choice is deferred, so no coverage is
silently claimed.

| Pair | Note |
|---|---|
| `semicolon_inside_block` / `semicolon_outside_block` | Opposite rewrites of the same code. |
| `pub_with_shorthand` / `pub_without_shorthand` | Opposite `pub(in ...)` spellings. 266 pilot diagnostics on the shorthand side alone. |
| `separated_literal_suffix` / `unseparated_literal_suffix` | Opposite literal-suffix spellings. 45 / 96 pilot records. |
| `mod_module_files` / `self_named_module_files` | Opposite module-file layouts. 27 / 0 pilot records. |

### 4.4 Style churn with no priority basis (Priority 1 — maintainability)

Each would force a large mechanical rewrite of working code with no
correctness, latency or energy gain; the approval explicitly forbids mass
rewrites to satisfy contradictory lints. Pilot volume in parentheses is **raw
records** (§5.1).

`absolute_paths` (3048), `arbitrary_source_item_ordering` (2144),
`str_to_string` (1569), `min_ident_chars` (1382), `single_call_fn` (621),
`default_numeric_fallback` (440), `pattern_type_mismatch` (365, inverts match
ergonomics), `pub_use` (104, inverts facade re-export design),
`single_char_lifetime_names` (91), `unused_trait_names` (89),
`field_scoped_visibility_modifiers` (70), `ref_patterns` (39, inverts match
ergonomics), `impl_trait_in_params` (33), `else_if_without_else` (16),
`redundant_test_prefix` (3), `shadow_reuse` (161) and
`shadow_same` (6) — idiomatic rebinding; the correctness-bearing member
`shadow_unrelated` (87) is **retained**.

`else_if_without_else` is excluded on **churn volume and absent priority
basis only**. The earlier claim that it contradicts the guard-clause doctrine
in `AGENTS.md` House style — Rust control flow is **withdrawn**: that doctrine
forbids splitting one decision between leading guards and a `match`, and says
nothing against a terminal `else` closing an `else if` chain. The two are
independent.

### 4.5 Inapplicable to this target

| Lint | Reason |
|---|---|
| `std_instead_of_core` (659), `std_instead_of_alloc` (160), `alloc_instead_of_core` (0) | Only meaningful for `no_std` targets. This workspace is `std`-only. |
| `non_ascii_literal` (285) | The domain content is Norwegian; non-ASCII literals are the product, not a defect. |
| `inline_modules` (79), `tests_outside_test_module` (79) | `inline_modules` fires on the standard `#[cfg(test)] mod tests { … }` idiom used throughout and is excluded on that basis. `tests_outside_test_module` does **not** contradict that idiom — it fires on `#[test]` functions *outside* a `mod tests`, which the idiom already avoids. Its 79 records are unadjudicated; exclusion here is **provisional pending §6.3**, not justified by the `mod tests` convention. |
| `missing_inline_in_public_items` (1519) | Blanket `#[inline]` inflates compile time and codegen without a measured latency requirement (Priorities 1 and 4 against an unmeasured Priority 3 claim). |
| `print_stdout` (6), `print_stderr` (9), `use_debug` (1) | `gh-report` is a CLI **binary**; writing to stdout/stderr is that binary's output contract. This rationale is **binary-scoped** and does not justify a workspace-wide `allow` across the library crates (`cherry_pit_*`), where console output is not a contract. Library/binary scoping is deferred to §6.3; the current workspace-level `allow` is provisional and over-broad relative to its rationale. |

### 4.6 Governance reconciliation

- `RST-0003:31-48` — root workspace levels with member inheritance and no
  per-member overrides is preserved; all 13 members already carry
  `[lints] workspace = true`. Exclusions are workspace-level `allow`s, not
  blanket module allows.
- `CHE-0029:62-66` records pedantic warnings. Every pedantic lint remains at
  warn; **no pedantic member appears in the exclusion roster**, so effective
  pedantic coverage is unchanged and CHE-0029 is not weakened.
- `COM-0035:34-51` (ratchet) — weakening is measured per lint, not per group.
  Measured default levels of the 42 exclusions: **41 are `allow` by default**
  (restriction members, never enforced here), and **1 is `warn` by default**:
  `blanket_clippy_restriction_lints`. Coverage arithmetic (corrected —
  `ghr-kmp4h` ReportMismatch): baseline `all,pedantic` enables **632**; of
  those **631 are retained** and **1 is excluded**; the proposed groups add
  **149 newly enabled**; final enforced set is **780** (631 + 149), a **net
  +148** before local overrides and dormancy. 780 is the *final total*, not
  the increase: effective coverage increases for **149** lints and
  **decreases for exactly one**. That one lint is a real
  ratchet event under COM-0035 and must be reopened against the existing
  record before enablement — it is recorded here as an open obligation, not
  as a discharged one. It cannot be avoided by configuration: the lint exists
  precisely to reject the group-level enablement that `ghr-7qm12` approves, so
  the approval and the lint are mutually exclusive by construction.
- `RST-0007:41-66` — no new merge gate is introduced by this checkpoint; the
  existing CI `-D warnings` contract is untouched because no manifest change
  has landed.
- `COM-0017:21-40` — the strongest feasible native enforcement is used; no
  claim of complete domain verification is made.

## 5. Pilot diagnostics (bounded, gh-report)

Command (no manifest mutation; command-line group flags only):

```
CARGO_TERM_PROGRESS_WHEN=never rustup run 1.98.0 cargo clippy -q \
  -p cherry-pit-core -p gh-report --all-targets --message-format=json \
  -- -W clippy::all -W clippy::pedantic -W clippy::nursery \
     -W clippy::restriction -W clippy::cargo
```

Exit 0. Wall time 24.8 s is an **anecdotal single observation** with no
recorded cache state, machine load or concurrency; per AGENTS.md § Iteration
speed #1 it is not a forecast and must not be used as a rollout budget until
re-measured with conditions recorded.

**Selection** is 2 explicitly selected packages (`cherry-pit-core`,
`gh-report`) of 13 members, with default features and `--all-targets` applied
to those two only. That is **not** the same as diagnostics originating from
only two crates: `-p` selects compilation roots, and the retained raw JSON
contains **eight** distinct library target names — `cherry_pit_app`,
`cherry_pit_core`, `cherry_pit_gateway`, `cherry_pit_projection`,
`cherry_pit_storage`, `cherry_pit_web`, `cherry_pit_wq`, `gh_report`. The
remaining members are unmeasured **as selection roots**; they are not wholly
unmeasured. Package coverage and target coverage are therefore separate
figures and are published separately.

Consequently: **no extrapolation is performed.** Multiplying any pilot figure
by 13/2 is unsupported, and no workspace-wide edit estimate is derived here.

### 5.1 Counting units

Three distinct units appear in the retained data. Every figure in this
document names the one it uses; they are not interchangeable.

| Unit | Definition | Baseline | Exempt |
|---|---|---|---|
| **raw records** | every `compiler-message` emitted | 27603 | 25107 |
| **target-span records** | unique (lint, target name, target kind, primary span) | 18615 | 16206 |
| **span-string keys** | unique (lint, primary-span string) — target identity removed. Path strings are **unnormalized**: an absolute and a relative spelling of the same physical line count as two keys (witnessed for `missing_const_for_fn` at `crates/gh-report/build_env.rs:18:9`). This is **not** a count of physical source sites. | 18551 | 16142 |

Provenance: `.ooda/tmp/clippy-policy-pilot-02/{baseline,exempt}.json`, retained
for resume. Reproduce with the jq commands in §5.3.

Unless stated otherwise, **every `pilot_hits` value in the TSV and every
parenthesised count in §4 is a raw record count.** The prose figures in the
pre-correction revision of this document disagreed with the TSV for twelve
lints; the TSV values are correct and the prose has been reconciled to them
(`ghr-cgtt6` L1).

The 18615 → 16206 reduction is a **target-span** figure, not a count of
distinct physical source lines; the span-string-key figures are 18551 → 16142,
computed over unnormalized path strings and therefore an upper bound on
physical sites. The gap
between raw and deduplicated counts is an **observation**, not an explanation:
feature-set identity, package identity, macro-expansion origin and
compilation-unit provenance are absent from the dedup key, so the cause of any
individual duplicate is **not established** here (`ghr-cgtt6` L2).

### 5.2 Residual after native test switches

| Measure | Value | Unit |
|---|---|---|
| Total clippy diagnostics | 27603 | raw records |
| Distinct lints triggered | 96 of 822 (11.7%) | lints |
| Of those, excluded by section 4 | 37 | lints |
| Of those, `retained-scope-pending` | 59 | lints |

Largest residual (retained) families, **raw records**, all requiring a scope
decision before enablement: `expect_used` 1041, `unwrap_used` 938,
`indexing_slicing` 844, `arithmetic_side_effects` 400, `missing_const_for_fn`
321, `missing_trait_methods` 286, `panic` 109. Measured pilot (`ghr-7qm12.2`)
replaces the earlier `panic` 141 prose figure with 109 raw records / 103 span-string keys,
and replaces "these concentrate in test targets" — an inference — with a
measured native-switch differential.

The four native `allow-*-in-tests` keys remove most of the
`expect`/`unwrap`/`indexing`/`panic` volume; **227 span-string keys** survive in
the exempt run. `arithmetic_side_effects` / `missing_const_for_fn` /
`missing_trait_methods` have no native test switch and are untouched: **465
span-string keys**.

Applying a purely **lexical** path filter (primary-span file not under a
`tests/`, `benches/` or `examples/` directory and not named `*_test.rs` /
`tests.rs`) leaves **148** and **401** span-string keys respectively.

**These are `lexical-other` buckets. They are not production reachability, not
defects, and not mandatory semantic migrations** (`ghr-cgtt6` M2). The filter
is path-based only: it routes inline `#[cfg(test)]` modules, bench and example
targets, and build scripts into `lexical-other` whenever their path does not
match the heuristic. The earlier 105 / 414 figures came from an unreproducible
variant of this heuristic and are **withdrawn**. The figures above are
**reconstructible by the reader** from the retained JSON by applying the filter
exactly as declared above; §5.3 supplies overall and single-lint commands only
and does **not** supply a family or lexical-filter command.

Adjudication status of the representative sites cited earlier:

| Site | Earlier reading | Corrected |
|---|---|---|
| `crates/gh-report/build_env.rs:18:9` (included by `build.rs:4`) | cited as a production site | `missing_const_for_fn` const-eligibility suggestion in code included by the build script — an **optional style/policy decision**, not a correctness finding |
| `cherry-pit-app/src/error.rs:80` | read as a broken error model by inference, never an established diagnosis | already implements `Error::source`; **not evidence of a defect** |
| `crates/…/state/mod.rs:1301` | — | a genuine non-test `panic!`. Residual production concerns are **not zero**; they are unquantified |

The next checkpoint is therefore blocked on **per-site adjudication,
governance reconciliation and fixtures** — *not* on a presumed production
migration. Correctness findings must be separated from optional const/style
policy decisions before any source change is proposed.

### 5.3 Reproducible verification

From `.ooda/tmp/clippy-policy-pilot-02/`:

```
# three counting units, blanket count, and observed library targets, per run
for f in baseline exempt; do jq -rs '
  def code: (.message.code.code // "NONE");
  [.[]|select(.reason=="compiler-message")] as $m
  | {raw: ($m|length),
     target_span: ($m|map([code,.target.name,.target.kind[0],
       (.message.spans[]?|select(.is_primary)|"\(.file_name):\(.line_start):\(.column_start)")]|tostring)|unique|length),
     source_site: ($m|map([code,
       (.message.spans[]?|select(.is_primary)|"\(.file_name):\(.line_start):\(.column_start)")]|tostring)|unique|length),
     blanket: ($m|map(select(.message.message|test("not meant to be enabled as a group")))|length),
     targets: ($m|map(select(.target.kind[0]=="lib")|.target.name)|unique)}' "$f.json" || exit; done

# per-lint raw and source-site counts (substitute any lint name)
jq -rs --arg l unseparated_literal_suffix '
  def code: (.message.code.code//"NONE"|sub("^clippy::";""));
  [.[]|select(.reason=="compiler-message")|select(code==$l)] as $s
  | {raw: ($s|length),
     site: ($s|map([$l,(.message.spans[]?|select(.is_primary)
       |"\(.file_name):\(.line_start):\(.column_start)")]|tostring)|unique|length)}' "baseline.json"
```

Expected: baseline `raw=27603 target_span=18615 source_site=18551 blanket=32`,
exempt `raw=25107 target_span=16206 source_site=16142 blanket=32`, eight
library targets in both. The `source_site` / `site` jq keys are retained
verbatim for reproduction, but they count **span-string keys** as defined in
§5.1, not physical source sites. The loop exits on the first failing `jq`
rather than masking it. No family or lexical-filter command is supplied here;
the §5.2 family and lexical figures require reader reconstruction from the
filter declared in §5.2.

## 6. Recorded gaps (not completed by this checkpoint)

1. **Deprecated catalogue.** `clippy-driver -W help` lists active lints only;
   it emits no deprecated section. The 17 deprecated entries are attributed to
   the official 1.98.0 catalogue via `ghr-shwvt`, not independently reproduced
   here. No native command in this checkpoint enumerates them. **Attribution
   closed in §6.1.3** against the pinned `deprecated_lints.rs`; the
   no-native-command limitation stands.
2. **Dormant configuration.** `excessive_nesting` (threshold default 0),
   `disallowed_fields`, `disallowed_macros`, `disallowed_methods`,
   `disallowed_types`, `missing_enforced_import_renames` and
   `await_holding_invalid_type` are enabled-but-dormant: they carry empty or
   zero default configuration and cannot fire without a `clippy.toml` list.
   Their `retained` disposition means "enabled", **not** "exercised". A full
   dormant census across all 822 was not performed. Populating them would
   require `clippy.toml` entries, which is out of scope here and collides with
   the sibling prohibitions. **Enumerated and sourced in §6.1.1**, with the
   `incompatible_msrv` contextual default distinguished in §6.1.2; the absent
   822-wide census and the absence of exercise stand.
3. **Scope decision** for the 59 `retained-scope-pending` lints (section 5).
4. **Thresholds.** The provisional 40 cognitive / 150 lines candidates are not
   measured and are not adopted by this document.
5. **No fixtures.** Retained/excluded/enum-evolution/must-use fixtures and
   plant-fail-revert-clean guard proof are not produced by this checkpoint.
6. **No sibling adjudication.** adr-fmt, comment-free, pardosa and scripts are
   untouched; the pardosa MSRV 1.89.0 versus 1.98.0 runner distinction and the
   unpinned `scripts` root remain open.
7. **Pilot selection scope** is 2 of 13 members as compilation roots; 8
   library targets were observed. 11 members are unmeasured **as selection
   roots**. No workspace extrapolation is made from this.
8. **Open ratchet obligation.** Excluding `blanket_clippy_restriction_lints`
   weakens one lint that is warn-by-default and in effect today. COM-0035
   requires reopening the relevant record before that lands. Not done here.
9. **Semantic assessment.** §3 is an inventory with provisional dispositions.
   Per-lint semantic assessment — compatibility, dormant-configuration
   behaviour, meaningful-domain exhaustiveness, interaction with other
   retained lints — is **not performed for any of the 822**, including the 721
   `retained`. Hit count is not a proxy for it.
10. **Exclusion scoping.** Every §4 exclusion is provisional. The
    library-versus-binary scoping decision (notably `print_stdout`,
    `print_stderr`, `use_debug`) and the `exhaustive_structs` rationale gap
    are open.
11. **Per-site adjudication.** The `lexical-other` buckets in §5.2 (148 and
    401 span-string keys) are unadjudicated. Separating correctness findings from
    optional style/policy decisions is a prerequisite to any source proposal.

## 6.1 Verified dormant configuration and deprecated entries (ghr-6z5sf)

Source: bead `ghr-6z5sf`, read-only research against pinned official material
— the `rust-1.98.0` lint index, `clippy_config/src/conf.rs`,
`clippy_lints/src/deprecated_lints.rs` and `clippy_utils/src/msrvs.rs` at the
`rust-1.98.0` tag. This section records that evidence and closes the
attribution half of §6 gaps 1 and 2. It does **not** retire §6 gap 9: none of
the entries below is semantically assessed or exercised by a fixture, and a
zero-hit row in `catalogue-1.98.tsv` still means "enabled", never "assessed".

### 6.1.1 Wholly dormant at default configuration (seven)

Each is enabled by the group activation but cannot fire without a
`clippy.toml` entry that this repository does not carry (`clippy.toml:1-6` is
MSRV plus the four test switches only).

| Lint | Config key | Default | Why dormant |
|---|---|---|---|
| `await_holding_invalid_type` | `await-holding-invalid-types` | `[]` | no prohibited await-held types configured |
| `disallowed_fields` | `disallowed-fields` | `[]` | requires a configured field-path list |
| `disallowed_macros` | `disallowed-macros` | `[]` | requires a configured macro-path list |
| `disallowed_methods` | `disallowed-methods` | `[]` | requires a configured method-path list |
| `disallowed_types` | `disallowed-types` | `[]` | requires a configured type-path list |
| `missing_enforced_import_renames` | `enforced-import-renames` | `[]` | requires configured path/rename pairs |
| `excessive_nesting` | `excessive-nesting-threshold` | `0` | `check_crate` returns immediately at zero; a nonzero threshold, not a lint level, activates it |

The five `disallowed-*` / `enforced-import-renames` keys stay **scoped
pending a concrete project-level ban**: no API name is invented here, which
also preserves the sibling prohibition on adding `clippy.toml` content.

Two boundaries on the list. It is a census of the declared configuration
surface in `conf.rs`, not seven executed positive controls. And an empty
exemption list is not the same shape: `absolute-paths-allowed-crates`,
`allow-unwrap-types`, `allowed-dotfiles`, `allowed-duplicate-crates`,
`allowed-wildcard-imports`, `arithmetic-side-effects-allowed*`,
`large-error-ignored` and `standard-macro-braces` all default empty while
their lints remain active — `nonstandard_macro_braces` in particular seeds a
built-in macro table in `macro_braces()`.

### 6.1.2 `incompatible_msrv` — contextual, not an eighth dormant check

`emit_lint_if_under_msrv` requires `Some(current)`, so with no MSRV supplied
by any route (`clippy.toml` `msrv`, Cargo `rust-version` via
`CARGO_PKG_RUST_VERSION`, or a local `clippy::msrv` attribute) the principal
check is suppressed. That condition **does not apply here**: `clippy.toml:1`
sets `msrv = "1.98"`. Separately, `check-incompatible-msrv-in-tests` defaults
`false`, so test targets are skipped — a scope exemption, not dormancy.

### 6.1.3 The 17 deprecated entries and their replacements

`clippy-driver -W help` emits no deprecated section (§6 gap 1); the list below
is `DEPRECATED` in `deprecated_lints.rs` at the pinned tag. These names are
removed and cannot be enabled, so none of them appears in, or is owed a
disposition by, the 822-name active roster. `RENAMED` aliases (for example
`blacklisted_name`, `cyclomatic_complexity`, `empty_enum`) are a separate list
and must not be folded into these 17.

| Deprecated name | Replacement | Removal reason |
|---|---|---|
| `assign_ops` | none named | compound operators harmless; outside Clippy scope |
| `extend_from_slice` | none named | `Vec::extend_from_slice` no longer faster than `Vec::extend` |
| `from_iter_instead_of_collect` | none named | lint proved problematic; deprecated in 1.98.0 |
| `match_on_vec_items` | `indexing_slicing` | covered by `Vec` indexing/slicing |
| `misaligned_transmute` | `cast_ptr_alignment` + `transmute_ptr_to_ptr` | split into these checks |
| `option_map_or_err_ok` | `manual_ok_or` | covers this case |
| `pub_enum_variant_names` | `enum_variant_names` | covered via `avoid-breaking-exported-api` |
| `range_step_by_zero` | none named | `Iterator::step_by(0)` now panics |
| `regex_macro` | none named | `regex!` removed in 2018 |
| `replace_consts` | none named | `min_value`/`max_value` deprecated |
| `should_assert_eq` | none named | `assert!(a == b)` can now print values |
| `string_to_string` | `implicit_clone` | covers those cases |
| `unsafe_vector_initialization` | none named | proposed alternative could be substantially slower |
| `unstable_as_mut_slice` | none named | `Vec::as_mut_slice` stable |
| `unstable_as_slice` | none named | `Vec::as_slice` stable |
| `unused_collect` | compiler `#[must_use]` on `Iterator::collect` | not a Clippy replacement |
| `wrong_pub_self_convention` | `wrong_self_convention` | covered via `avoid-breaking-exported-api` |

Every named replacement is already present in the active roster:
`indexing_slicing` (`catalogue-1.98.tsv:231`), `cast_ptr_alignment` (`:59`),
`transmute_ptr_to_ptr` (`:699`), `manual_ok_or` (`:350`),
`enum_variant_names` (`:153`), `implicit_clone` (`:219`),
`wrong_self_convention` (`:818`). No count was recomputed for this section,
and no new exclusion follows from it.

## 7. Enablement status — native config landed, pilot RED

Superseding the prior "explicit non-enablement" section: mission
`clippy-native-pilot-07` (`ghr-7qm12.7`) has **landed the native mechanism**.

- `Cargo.toml:233-290` now carries the five groups at `warn` / priority `-1`
  and 47 individually justified `allow`s at priority `0`. All 13 members
  still inherit via `[lints] workspace = true`; no per-member override was
  added (`RST-0003:31-48` preserved).
- `clippy.toml:3-6` carries the four permanent native test switches
  `allow-expect-in-tests`, `allow-unwrap-in-tests`,
  `allow-indexing-slicing-in-tests`, `allow-panic-in-tests`. No
  `disallowed-*` API lists were invented; no threshold was adopted.

### 7.1 Exclusions added beyond the §4 roster of 42

Each is an independent semantic decision, not a convenience waiver.

| Lint | Group | Priority conflict | Prior enforcement |
|---|---|---|---|
| `missing_const_for_fn` | nursery | Priority 1. `const fn` is an API-stability commitment: once published, removing `const` is a breaking change. Demanding it wherever the body happens to be const-eligible trades future changeability for no correctness, latency or energy gain. | `allow` by default — no enforcement lost. |
| `missing_trait_methods` | restriction | Priority 1. Demands every defaulted trait method be written out at every impl site; a pure shape demand that multiplies edit surface on every trait change. | `allow` by default. |
| `arithmetic_side_effects` | restriction | Priority 1 vs an unmeasured Priority 2 claim. A blanket ban on `+`/`-`/`*` is not the workspace's overflow control: `[profile.release] overflow-checks = true` (`Cargo.toml:231`) is, and the genuinely dangerous unchecked cases remain covered by retained `all`/`pedantic` members (`cast_possible_truncation`, `cast_sign_loss`, `as_conversions`). | `allow` by default. |
| `float_arithmetic` | restriction | Same family, same rationale; no fixed-point or deterministic-float requirement is recorded anywhere in this workspace. | `allow` by default. |
| `struct_excessive_bools` | pedantic | Priority 2, **against** the lint. `AGENTS.md` § Rustling records `archived` / `has_issues` / `fork` / `is_empty` on `domain::repository` as genuinely *independent* attributes, not exclusive states. Collapsing them into an enum to satisfy a count threshold would invent a domain constraint the domain does not have — the exact distortion the approval forbids. Reproduced by fixture: `tools/clippy-fixtures/excluded.rs::RepositoryFlags`. | `allow` by default. **This is the roster's only pedantic member**, so `CHE-0029:62-66` is weakened by exactly one lint; recorded here as an open ratchet item alongside §4.6. |
| `decimal_literal_representation` | restriction | Priority 1. Advises respelling a decimal literal in hex. The one witness is `MAX_LOCK_FILE_BYTES = 1_048_576` (`cherry-pit-storage/src/lock.rs`), a documented byte budget whose decimal form is the readable one. No correctness, latency or energy claim follows either way. | `allow` by default. |
| `redundant_pub_crate` | nursery | Priority 1. Advises narrowing `pub(crate)` to `pub` on items in a private module. The visibility written is the intended reach and survives the module later becoming public; the narrower spelling states less. Witnesses: `cherry-pit-storage/src/signature.rs:53,85`. | `allow` by default. |
| `shadow_unrelated` | restriction | Priority 1. Binding shadowing is idiomatic Rust and Rust's own scoping already preserves type legality at every witness (`cherry-pit-storage/src/lock.rs:285,302,308,382,727` — all `e`/`pid`/`dir` rebindings inside short blocks). Excluding it is a readability judgement; **no correctness property is claimed proven by its absence**. | `allow` by default. |
| `integer_division` | restriction | Priority 2, per-witness. Native `-D` witnesses (control C1 below): **3 sites, all workspace-authored** — `cherry-pit-wq/src/token_bucket.rs:40,115,462`, milli-token quantization against **constant** denominators declared in the same file. The divisor cannot be zero and the truncation is the quantization the algorithm specifies; satisfying the lint means a float or newtype algorithm change. **Explicitly not claimed**: that excluding this lint proves any denominator non-zero. | `allow` by default — no enforcement lost. |
| `integer_division_remainder_used` | restriction | Priority 2, per-witness. Native `-D` witnesses: **9 sites** — the three `integer_division` sites above, plus six emitted by `tokio::select!` branch-start randomization (`backoff.rs:97`, `budget.rs:329,354,476`, `token_bucket.rs:187`, `worker_pool.rs:406`), which are **not workspace-authored** and have no source-level remedy short of abandoning `select!`. The site sets of the two lints therefore overlap but are **not identical**. Where a genuine per-site bound exists in a test target it is discharged by statement-level `expect` instead (see the `cherry-pit-storage/tests/properties.rs` row below), never by widening this roster. | `allow` by default. |
| `modulo_arithmetic` | restriction | **Not excluded.** The same native `-D` run produces **zero** `modulo_arithmetic` diagnostics: the `tokio::select!` remainder is reported under `integer_division_remainder_used`, not here, so `select!` provenance establishes the source of the `%`, not this lint's identity. An exclusion would be unsupported and was removed from the roster. | Enforced at group level; no witness. |
| `partial_pub_fields` | restriction | Priority 2, **against** the lint. `cherry-pit-wq/src/work_queue.rs:45` `JobSpec` deliberately mixes `pub` data fields with a `pub(crate) enqueued_at`, which is a type-level invariant: producer-set values were silently overwritten before findings-F9. The lint's two remedies both destroy something real — widening `enqueued_at` removes the invariant, privatizing the data fields is a public API change. Excluded **without** widening visibility. | `allow` by default. |

`exhaustive_structs` (§4.2 rationale gap) is retained as an exclusion on the
commander's explicit direction: a blanket `#[non_exhaustive]` mandate denies
valid closed DTO/value construction, independently of the closed-error policy
at `AGENTS.md:288-292` that justifies `exhaustive_enums`.

Deliberately **not** excluded, per the approval's "safety/error/must-use/
exhaustiveness checks retained": `wildcard_enum_match_arm`,
`let_underscore_must_use`, `unused_result_ok`, `map_err_ignore`,
`panic_in_result_fn`, `unwrap_in_result`, `expect_used`, `unwrap_used`,
`indexing_slicing`, `panic`, `string_slice`, `as_conversions`.

Where one of those retained lints fires on a **test-only** target it is
discharged per-site under the RST-0003 test-assertion exception (RST-0003
Context, amended 2026-09-18), never by adding it to the roster above.
Discharged this way in `cherry-pit-storage`:

| Site | Lint | Disposition |
|---|---|---|
| `src/lock.rs::read_lock_rejects_oversized_file` | `wildcard_enum_match_arm` | Item-level `expect`: the arm is `other => panic!(…)`, the failure branch of an assertion. |
| `src/lock.rs::capture_survives_wrong_thread_first_registration`, `::capture_registration_child` | `print_stderr`, `use_debug` | Item-level `expect`: harness progress diagnostics in a multi-process capture fixture. |
| `src/lock.rs::concurrent_acquire_exactly_one_wins` | `needless_collect` | Item-level `expect`: the `Vec` forces all threads to spawn before any join; lazy iteration would serialise the race the test exists to exercise. |
| `tests/properties.rs` shuffle | `as_conversions`, `integer_division_remainder_used` | Statement-level `expect`: seeded Fisher-Yates index, in range by construction. |

Discharged the same way in `cherry-pit-projection` and `cherry-pit-wq`
(mission `clippy-projection-wq-tests-21`, test and bench targets only — both
libraries were already green):

| Site | Lint | Disposition |
|---|---|---|
| `cherry-pit-projection/tests/conformance_pardosa_store.rs` — `temp_pgno_path`, `aggregate_id`, `seq`, `recorded_envelopes` | `expect_used` | Item-level `expect`: fixture setup and nonzero-literal constructors. Each reason names the bound (all call sites pass nonzero literals; `create`/`append` return at least one envelope per submitted event), not the filename. |
| `cherry-pit-wq` — **25 item-level `expect` attributes** covering **29 executable `as` sites** across `src/budget.rs`, `src/token_bucket.rs`, `src/worker_pool.rs`, `tests/comparative.rs`, `tests/f1_phantom_304.rs`, `benches/contention.rs` | `as_conversions` | Item-level `expect`. **Two disjoint families, never one blanket.** (i) **23** trait-object *unsizing* coercions `Arc<Concrete> -> Arc<dyn Trait>`: no value is reinterpreted, only a vtable attached, so loss is impossible; `From`/`TryFrom` cannot express it in argument position. (ii) **6** `usize -> u64` widenings — `tests/comparative.rs:145,169,192,214,313` and `benches/contention.rs:95`. Their sources are a proptest vector bounded by `limit_and_schedule` (limit drawn `1u64..20`, length drawn `0..=limit`, hence **at most 19** elements), a product of file-local loop constants (`5 * 3 = 15`), and the bench product `CALLS_PER_THREAD * threads` (max `200 * 16 = 3200`). `u64` is at least as wide as `usize` on every supported target, so none can truncate. The unsizing family makes no numeric claim.
| `cherry-pit-wq/src/budget.rs::zero_limit_panics`, `token_bucket.rs::zero_capacity_panics`, `::zero_refill_rate_panics` | `let_underscore_must_use` | Item-level `expect`: the discard **is** the assertion — construction is asserted to panic, so control never reaches a binding. |
| `cherry-pit-wq/src/budget.rs::cancelled_resetter_releases_election_and_wakes_waiters` | `let_underscore_must_use` | No longer suppressed: the join result is not discarded. The test asserts `doomed_join.is_err_and(|e| e.is_cancelled())`, so a panic outcome is rejected rather than absorbed. |
| `cherry-pit-wq/src/worker_pool.rs`, `tests/f1_phantom_304.rs` | `wildcard_enum_match_arm` | Item-level `expect`: the arms are failure sinks (`_ => panic!`). Naming variants would silently admit a future variant instead of rejecting it — the inverse of the lint's intent at these sites. |
| `cherry-pit-wq/src/budget.rs::replenish` (two test doubles) | `significant_drop_in_scrutinee` | Item-level `expect`: the guard is held so upgrade and use observe one consistent view; the body does take a second lock (`self.seen`), in a consistent gate -> seen order that no path reverses. It performs no await and no I/O while holding the guard; `std::sync::Mutex::lock` may still block, so the argument is lock ordering, not non-blocking. |
| `cherry-pit-wq/src/token_bucket.rs` — two contention tests | `map_with_unused_argument_over_ranges` | Item-level `expect`: the range's *count*, not its values, is the workload parameter. |
| `cherry-pit-wq/benches/contention.rs` — two measured workloads | `missing_assert_message`, `unwrap_used` | Item-level `expect`: inside the measured region; the assertions guard that the benchmark exercised the admit path rather than a rejected fast path, and a join failure invalidates the sample. |

Not discharged by exception, fixed idiomatically instead (behaviour
identical): `cherry-pit-wq/tests/properties.rs::p3_fifo_distinct` dropped a
`redundant_clone` by moving the last owner, and
`tests/loom_election.rs:33` gained the terminal punctuation the
`doc_paragraphs_missing_punctuation` lint asked for.

`let_underscore_must_use` in `src/signature.rs:41,96` is **not** discharged
as a test exception — both are production sites. They are rewritten to
`write!(…).expect(…)` with a per-site `expect_used` exception, because
`std::fmt::Write for String` is infallible (`write_str` always returns `Ok`);
the `Result` there models the `fmt` contract, not IO. True IO error handling
in this crate stays strict. `c as u32` at `:96` became `u32::from(c)` — a
lossless, behaviour-preserving idiom change, not an exception.

### 7.2 Executable proof fixtures

`tools/clippy-fixtures/run.sh` — 22 assertions, **exit 0** *(historical; the
current harness asserts 58 — see Current status above)*. Test
orchestration only: it derives its lint flags **from the real
`[workspace.lints.clippy]` table** by parsing `Cargo.toml`, then invokes the
pinned native `clippy-driver` (`rust-toolchain.toml` channel 1.98.0) and
asserts on diagnostic identity. No custom lint engine, no generator, no
dependency, no new manifest entry.

- `excluded.rs` — 11 assertions that each excluded lint is **silent**:
  `implicit_return`, `exhaustive_structs`, `exhaustive_enums`,
  `missing_const_for_fn`, `arithmetic_side_effects`, `float_arithmetic`,
  `absolute_paths`, `str_to_string`, `std_instead_of_core`,
  `missing_docs_in_private_items`, `question_mark_used`. Includes the
  closed-enum-evolution case (`Outcome` without `#[non_exhaustive]`) and the
  independent-bool case (`RepositoryFlags`).
- `retained.rs` — 10 assertions that each retained lint **fires**:
  `panic_in_result_fn`, `unwrap_in_result`, `let_underscore_must_use`,
  `unused_result_ok`, `map_err_ignore`, `wildcard_enum_match_arm`,
  `indexing_slicing`, `expect_used`, `unwrap_used`, `panic`. Covers the
  must-use case (`let _ = checked_handle()`).
- Plus 1 bool-policy assertion.
- Positive controls: the same fixture is re-run in a second generated
  workspace whose copied table has the excluded `allow` entries removed;
  every one of the 12 excluded lints must fire there, so no silence
  assertion is vacuous.
- Configuration controls: `tests_ctl.rs` carries a recognized `#[test]` and
  an ordinary helper. Under the root `clippy.toml` only the helper fires;
  with `allow-*-in-tests = false` both fire. Configuration is pinned with
  `CLIPPY_CONF_DIR` and the manifest with `--manifest-path`, so the run is
  independent of the caller's working directory.
- Enum-evolution control: a separate crate with an exhaustive consumer
  compiles, gains a variant and fails with `E0004`, then compiles again once
  the consumer is repaired.
- Decoder controls: malformed input and a wrong-shaped `compiler-message`
  are errors (exit 2), not silence.

**C1 — native controls for the four §7.1 arithmetic/visibility dispositions.**
The fixture harness `excluded_lints` roster does **not** include these, so its
green is not their positive-control proof. They are controlled directly against
real workspace source with the pinned toolchain, no fixture and no source
mutation:

```
cargo clippy -p cherry-pit-projection -p cherry-pit-wq --all-targets --locked \
  --message-format=json -- -D clippy::integer_division \
  -D clippy::integer_division_remainder_used -D clippy::modulo_arithmetic \
  -D clippy::partial_pub_fields
```

| Control | Lint | Result |
|---|---|---|
| Positive | `integer_division` | fires — 3 sites (`token_bucket.rs:40,115,462`) |
| Positive | `integer_division_remainder_used` | fires — 9 sites (the 3 above + 6 `select!` sites) |
| Positive | `partial_pub_fields` | fires — 1 site (`work_queue.rs:45`) |
| **Negative** | `modulo_arithmetic` | **zero diagnostics** — exclusion unsupported, removed |

Negative control for the three retained exclusions: the ordinary
`cargo aclippy -p cherry-pit-projection -p cherry-pit-wq --all-targets --locked
-- -D warnings` (no overrides) exits 0, so each is silent under the committed
roster. Neither run mutates source.

Four-step enforcement proof (2026-09-18, toolchain 1.98.0): clean run exit 0;
removed the `question_mark_used` allow entry from the root `Cargo.toml`
→ exit 1 with `FAIL excluded: clippy::question_mark_used fired but is
allowed at priority 0`; reverted the manifest; clean run exit 0 again;
`git diff --check` exit 0.

Observed fail→clean transitions during construction (not a merge-gate proof):
`struct_excessive_bools` fired on `RepositoryFlags`, the exclusion was added,
and the assertion went silent; `unwrap_in_result` was silent on an
`Option`-typed fixture and fires on the `Result`-typed one, so the retained
assertion is proven live rather than vacuous.

### 7.3 Pilot is RED — concrete blocker, no migration attempted

*(Historical inventory. Superseded for `cherry-pit-core`/`cherry-pit-storage`,
both now exit 0; the figures below are the original witness counts.)*

`cargo aclippy -p cherry-pit-core --all-targets -- -D warnings` → **exit 101**,
64 errors. `cargo aclippy -p gh-report --all-targets -- -D warnings` → **exit
101**, aborting at the *build script* (2 errors) before the lib/bin targets
are reached, so gh-report's residual beyond `build_env.rs` is **unmeasured**.

**Correction (2026-09-18, review ghr-ma4og M2):** an earlier version of this
section claimed the four `allow-*-in-tests` switches reach `#[cfg(test)]`
modules **only**, and therefore that every integration test is structurally
excluded. That claim is **false** and is withdrawn. Native probes, reproduced
by the `tests_ctl` control in `tools/clippy-fixtures/run.sh`, show the
switches keyed on the *recognized test context*: a `#[test]` item is
suppressed without any `#[cfg(test)]` module, while an ordinary helper in the
same file fires all four lints. What remains live in integration targets is
therefore **helper and support code**, not test functions — a bounded set to
adjudicate, not a structural impossibility. `crates/cherry-pit-core/src/testing.rs`
is a genuine residual: a `pub` library-support module compiled into the lib,
which no test switch reaches. Per-item `#[expect(clippy::lint, reason = "...")]`
under `RST-0003:R5` is a third native route alongside per-file and per-crate
forms; only the per-crate `[lints]` override is forbidden (`RST-0003:31-48`).

Witnesses, `cherry-pit-core` only, 18 families over 13 distinct files:

| Family | Files |
|---|---|
| `inline_trait_bounds` | `src/policy.rs`, `src/testing.rs`, `tests/conformance_in_memory.rs`, `tests/event_history_in_memory.rs`, `tests/hexagonal_ports_only.rs` |
| `assertions_on_result_states` | `src/aggregate_id.rs`, `src/event.rs`, `tests/adt_obligations.rs` |
| `expect_used` | `src/testing.rs`, `tests/termination_is_domain_event.rs` |
| `indexing_slicing` | `src/testing.rs`, `tests/event_history_in_memory.rs` |
| `unwrap_used` | `tests/event_history_in_memory.rs`, `tests/scheduler.rs` |
| `string_slice` | `tests/adt_obligations.rs`, `tests/termination_is_domain_event.rs` |
| `option_if_let_else` | `tests/adt_obligations.rs`, `tests/termination_is_domain_event.rs` |
| `too_long_first_doc_paragraph` | `src/policy.rs`, `src/store.rs` |
| `wildcard_enum_match_arm` | `src/error.rs`, `src/testing.rs` |
| `panic`, `future_not_send`, `let_underscore_untyped`, `module_name_repetitions`, `significant_drop_tightening` | `src/testing.rs` |
| `redundant_clone` | `src/event.rs` |
| `renamed_function_params` | `tests/conformance_in_memory.rs` |
| `cargo_common_metadata`, `multiple_crate_versions` | manifest-level, no source span |

`crates/cherry-pit-core/src/testing.rs` is **not** a `#[cfg(test)]` module — it
is a `pub` test-support module compiled into the library, so its
`expect_used` / `panic` / `indexing_slicing` hits are production-surface by
compilation and no test switch reaches them.

Two manifest-level families are separate from the source question:
`cargo_common_metadata` wants `package.readme` / `description` / `keywords` /
`categories` across the members (12 records, e.g. `cherry-pit-merger is
missing package.readme`); `multiple_crate_versions` reports `syn 2.0.119` vs
`3.0.5`, which is dependency-graph-determined and not locally actionable.
Neither was suppressed.

**Correction (2026-09-18, review ghr-ma4og M2):** the earlier inference that
“13 files exceeds the 5-production-site abort bound” is **withdrawn**. The 13
files are mixed — six `src/` files (several hits inside inline unit tests)
plus integration-test files — so the count does not establish a production-site
bound, and the 5-site figure was a local budget, not a user-imposed
mass-rewrite threshold. The exact production sites remain to be adjudicated.
Migration was **not** attempted and nothing was relaxed to reach green. The
§4.6 ratchet obligation on `blanket_clippy_restriction_lints` and the
`struct_excessive_bools` pedantic item in §7.1 were both subsequently
**ACCEPTED** as policy exceptions (RST-0003:48-54); they are no longer open.

Gaps 3, 5, 10 and 11 of §6 are closed or narrowed by this section; gaps 1, 2,
4, 6, 7, 8 and 9 remain open exactly as written.

### 7.4 Style adjudications and domain/event site invariants (ghr-7qm12.41)

Two independent decisions, both scoped to the `gh-report` library.

**(a) Five pure-spelling exclusions.** Each mandates one of two legal
spellings of an unchanged expression, item grouping or prose. No correctness,
latency or energy property follows from either spelling, so the only effect of
enforcing them is a mechanical rewrite of existing code (Priority 1).

| Lint | Group | Witnesses (native `-p gh-report --lib`) | Decision |
|---|---|---|---|
| `use_self` | nursery | 39 | `allow`. `Self` vs the written type name is a spelling of the same path; the written name is often the clearer one in a long `impl`. |
| `if_then_some_else_none` | restriction | 7 | `allow`. `if c { Some(x) } else { None }` and `c.then(…)` denote the same value with the same laziness; witnesses include `(denominator > 0).then(…)` guards where the `if` form states the guard more plainly. |
| `return_and_then` | restriction | 1 | `allow`. Demands `?` in place of a terminal `and_then`. `?` is already permitted (`question_mark_used` is excluded, §4), so this is a preference between two sanctioned spellings, not a rule about which is legal. |
| `multiple_inherent_impl` | restriction | 7 | `allow`. Splitting an `impl` block is a grouping statement about the items inside it; merging them to satisfy a count states less. |
| `doc_paragraphs_missing_punctuation` | restriction | 1 | `allow`. Terminal punctuation in a doc paragraph. |

Group membership above is read from native `clippy-driver -Whelp` (1.98.0), not
from a catalogue restatement: `use_self` is **nursery**; `if_then_some_else_none`,
`return_and_then`, `multiple_inherent_impl`, `doc_paragraphs_missing_punctuation`
and `allow_attributes` are **restriction**. **None of the six is `pedantic`** —
an earlier claim that two were pedantic exceptions is withdrawn. All five groups
stay warn-by-default; no pedantic coverage is weakened by these allows.

**Not excluded**, though adjudicated in the same slice:
`missing_assert_message` (4 witnesses, all `crates/gh-report/src/app/collect.rs`).
Its remedy adds failure-diagnostic text, which is a maintainability gain rather
than a respelling, so it fails the pure-spelling test the five above pass. It
stays enforced and the four witnesses remain open.

**(b) `allow_attributes` — excluded on an exact doctrine conflict.**
RST-0003:R5 reads: "Per-site suppression uses `#[expect(clippy::lint, reason =
"...")]`, or `#[allow(clippy::lint, reason = "...")]` where the expectation
would be unfulfilled". `allow_attributes` bans the second form outright, so it
contradicts an enforced local governance rule rather than reporting a defect
(the §4.2 category). 11 witnesses. The justification requirement is *not*
relaxed: `allow_attributes_without_reason` is **not** excluded and stays
enforced at group level, so every `#[allow]` must still carry its `reason`.

**(c) Per-site invariants in the four domain/event files.** Suppressions are
per-function `#[expect(…, reason = …)]`, never a module or workspace waiver,
and each reason states the actual precondition rather than asserting safety.

| Site | Lint | Invariant discharged |
|---|---|---|
| `event/mod.rs` `impl_pardosa_enum!` `encode_type` | `as_conversions` | Every invocation declares explicit discriminants on a fieldless enum, with actual maximum 6 (`SecurityPolicyEvidence::NotApplicable`), so `*self as u8` is exact and agrees with the `discriminant_width: 1` the same macro writes into the descriptor. |
| `event/mod.rs` `impl_pardosa_enum!` `decode_type` | `indexing_slicing` | `buf[0]` is preceded in the same body by the `buf.is_empty()` guard returning `TruncatedPayload`. |
| `event/mod.rs` `impl_pardosa_struct!` `decode_type`, and the three hand-written `decode_type` impls (`SweepTimeoutEvent`, `CollectionCoverage`, `DomainEvent`) | `indexing_slicing` | `cursor` starts at 0 (1 after a guarded tag read) and advances only by the count each field decoder reports consuming from the slice it was handed, so `cursor <= buf.len()` at every `&buf[cursor..]`. A short buffer surfaces as `TruncatedPayload` from the field decoder. |
| `event/mod.rs::team_domain_key`, `config/runtime.rs::org_token` | `indexing_slicing` | `HEX: [u8; 16]`; `byte >> 4` and `byte & 0x0f` are both in 0..=15 for every `u8`. |
| `domain/run.rs::correlation_context` | `indexing_slicing` | The in-function `assert!(hex.len() == 32)` plus `bytes: [u8; 16]` bound the enumerate index to 0..=15, so the maximum access `i * 2 + 1` is 31. |
| `domain/run.rs::decode_hex_nibble` | `panic` | **Not suppressed.** An earlier revision claimed non-hex was unrepresentable; that is false — `RunMetadata::run_id` is a public `String` field and is also derived `Deserialize`, so an invalid value is constructible and the panic is reachable. The expectation was removed and the diagnostic is left open pending a validated `RunId` type. |
| `domain/run.rs::generate_run_id` | `expect_used` | `core::fmt::Write for String` returns `Ok` unconditionally — infallible, not an unhandled error path. |
| `event/convert.rs` `to_es`, `to_event_vec`, `to_nes`, `ts_required`, `TryFrom<sm::OrgAlertSummary>` | `map_err_ignore` | Each discarded error is the single range/length failure mode of the constructor called on the preceding expression. The translated error keeps the `field` provenance (and, in `to_nes`, *widens* classification into distinct `Empty` / `TooLong`). No error is folded into an absence. |
| `event/convert.rs` `TryFrom<sm::OrgAlertSummary>` | `iter_over_hash_type` | Both map iterations only collect; each collected `Vec` is `sort_by`'d on its key before becoming an `EventVec`, so serialized order is decided by the sort, not by hash order. This lint is **not** excluded globally — the per-function proof is the determinism argument. |

**Honestly remaining in these files** (3 diagnostics, not suppressed, no
remedy attempted in this slice):

- `event/convert.rs:554` `map_err_ignore` and `:582` `as_conversions` — both
  inside `conversion_pair!` invocation bodies, where no item or statement
  exists to carry a per-site attribute. Annotating them requires either a
  macro change or moving the expression, neither of which is a suppression
  decision.
- `event/mod.rs:316` `same_name_method` — an inherent `event_type` colliding
  with a trait method of the same name. The remedy is a rename, which is a
  code change, not a policy call; left for separate adjudication.

The library as a whole is **not** green; families outside these four files
(`string_slice`, `wildcard_enum_match_arm`, `expect_used`, the remaining
`indexing_slicing`) are untouched by this slice, and the reduced count is not
a green claim.
