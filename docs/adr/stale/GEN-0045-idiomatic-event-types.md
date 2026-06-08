# GEN-0045. Idiomatic Types for Event Payloads

Date: 2026-05-18
Last-reviewed: 2026-05-18
Tier: B
Status: Accepted

## Related

References: GEN-0035, GEN-0036, GEN-0040, GEN-0041, GEN-0042, GEN-0044

## Context

`GenomeSafe` blanket impls in `genome_safe.rs` widen the event-payload
type surface to most `std` primitives and several containers. The set
was not curated against stored-event semantics: borrowed views force
lifetime threading, runtime-sharing wrappers don't survive serialisation,
and raw floats admit `NaN`/`±∞`/subnormals that downstream invariants
must re-check per call. A Phase-2 F9 audit identified four load-bearing
findings (float discipline, Unicode-scalar wrapper, two blanket removals)
plus four doctrine-only findings. Companion mechanisms already shipped:
GEN-0042 bounded wrappers, GEN-0040 `Validate`, GEN-0041 foreign-crate
floor. Missing: a type-selection doctrine F9a–F9d implementations cite.

## Decision

R1 [5]: Float event fields select from a four-tier wrapper family:
  `FiniteF{32,64}` (reject `NaN`), `RealF{32,64}` (reject `NaN`,
  `±∞`, subnormals), `OrderedF{32,64}` (`RealF*` + `Ord` via
  `total_cmp`). Raw `f32`/`f64` retain `GenomeSafe` for fields that
  deliberately carry IEEE-754 divergence signal. Wire byte-identical
  to inner (PM4).

R2 [5]: `CharScalar` wrapper over `char` rejects surrogate codepoints
  U+D800..=U+DFFF at `TryFrom` and `Decode`. Raw `char` retains
  `GenomeSafe`. Semantic distinction: `CharScalar` = one Unicode
  scalar; `EventString<4>` = up to 4 UTF-8 bytes. Wire byte-identical
  to raw `char`.

R3 [5]: `GenomeSafe` blanket impls for `&str` and `&[u8]` are
  removed. Borrowed views force lifetime threading; PAR-0021:R5
  hash-over-canonical-bytes requires owned predecessors. Replacements:
  `EventString<MAX>` / `EventBytes<MAX>` (GEN-0042), or
  `String`/`Vec<u8>` when MAX is unpinned. A `compile_fail` test
  rejects both as event fields.

R4 [5]: `GenomeSafe` blanket impls for `Arc<T>` and `Cow<'_, T>` are
  removed. Refcount-sharing is in-memory only — decode produces a
  fresh `Arc::new(T)`. `Cow<'_, T>` generalises the &str hazard.
  Parallels the existing `Rc<T>` exclusion. Migration: wrap with
  `Arc::new(decode(…)?)` at the call site. A `compile_fail` test
  rejects both.

R5 [6]: The following blankets are retained but documented as
  *non-idiomatic standalone event fields*: `u8` (use `EventBytes`),
  `u16` (use `u32` + newtype), `Box<T>` (hash-transparent), `()`
  (use `Option<T>`), `u128`/`i128` (use a range-constrained newtype),
  raw tuples (use a named struct per GEN-0028).

R6 [6]: F9a–F9d impl-level doc-comments cite this ADR.
  `adr-fmt --refs GEN-0045` shows backlinks from at least one site
  per locked-in finding (R1–R4).

## Consequences

+ becomes easier: event-type review — one ADR pins rationale for
  every retained-but-non-idiomatic blanket.
+ becomes easier: adversarial-input handling — `RealF*` rejects
  `NaN`/`±∞`/subnormals at decode; `CharScalar` removes surrogate
  hazard.
+ becomes easier: lifetime hygiene — R3/R4 removals force ownership
  decisions at the type level.

− becomes harder: float-heavy event types — authors pick a tier per
  field (raw / Finite / Real / Ordered) instead of letting `f64`
  compile by default.
− becomes harder: shared-payload optimisation — `Arc<T>` as a field
  no longer permitted; share at the call site, post-decode.

risks/migration: R3/R4 are wire-compatible for valid values; the
schema-hash changes per GEN-0035 / GEN-0044:R4. Consumers re-validate
once across F2+F9 under the FORMAT_VERSION=3 squash (PAR-0021 F2a) —
one re-validation, not two.

## Open questions

- R3/R4 in-tree consumers are enumerated by F9c/F9d pre-flight
  copernicus sweeps, not by this ADR.
- Float-tier ergonomic API (`new` vs `TryFrom` vs both) is an F9a
  implementation choice, not pinned here.
- R5's `i128` clause is the weakest — re-examine if a future ADR
  names a 128-bit clock type as load-bearing wire.
