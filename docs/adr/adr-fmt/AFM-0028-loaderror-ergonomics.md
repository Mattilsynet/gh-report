# AFM-0028. LoadError Ergonomics Amendment to AFM-0026

Date: 2026-05-18
Last-reviewed: 2026-05-18
Tier: S
Status: Proposed

## Related

References: AFM-0026, CHE-0030, COM-0013

## Context

AFM-0026:R1 pinned the `adr-fmt` library API surface — including the
`LoadError` type — but did not constrain its trait impls. The surface
fixed *what types are public*; it left *what those types implement*
unsaid. On first integration, that gap bit the current consumer.

`adr-srv` (the current downstream per COM-0013:R1) needs idiomatic
Rust error handling and discovered that `LoadError` lacks
`core::fmt::Display`, `core::fmt::Debug`, and `std::error::Error`. The
workaround in `crates/adr-srv/src/lib.rs:27-30` and
`crates/adr-srv/tests/smoke.rs:24-29` is a variant-match shim — it
unblocks the smoke test but does not generalise to the bridge-stage
patterns `adr-srv` will need: `?` into `Box<dyn Error>`,
`tracing::error!(?e, ...)`, `panic!("{e}")`, and
`#[derive(thiserror::Error)] #[from] LoadError` in higher-layer error
enums. None of these compile against the current `LoadError`.

The conventional baseline is Rust API Guidelines C-GOOD-ERR: public
error types should implement Display + Debug + std::error::Error.
AFM-0026:R1 silently dropped this baseline by pinning the surface set
without naming the trait floor. COM-0013:R1 (no speculative widening
of the surface without a current consumer) is satisfied here by
adr-srv being the actually-blocked consumer — this is not anticipatory
ergonomic polishing, it is unblocking an integration on disk.

The alternative considered was an in-place edit of AFM-0026:R3 to add
the trait surface to the semver contract directly. Rejected because
AFM-0026 is Accepted, and the AFM lifecycle convention amends Accepted
ADRs via successor (this ADR), not via in-place body edits. The
amendment-via-successor route also gives the trait-floor rule its own
identity for future `--refs` traversal.

## Decision

Amend AFM-0026 by adding a trait-surface constraint on every public
error type in the AFM-0026:R1 set; reaffirm semver stability of those
trait surfaces under AFM-0026:R3; and make the constraint inheritable
so future error types added to the R1 surface do not each need their
own follow-up ADR.

R1 [5]: Every public error type in the AFM-0026:R1 surface set MUST
  implement `core::fmt::Display`, `core::fmt::Debug`, and
  `std::error::Error`. `LoadError` is the current consequence
  (`adr-srv` is the current consumer per COM-0013:R1); future error
  types added to R1 inherit this rule by construction.

R2 [4]: The `Display` impl MUST produce a human-readable,
  single-line-preferred message. The impl MUST NOT include sensitive
  paths or values beyond what the variant already names in its
  public-field contract per AFM-0026:R3. Existing variant fields (e.g.
  file paths inside `LoadError::Io(String)`) are already part of the
  public surface and may appear unchanged in the Display output.

R3 [5]: The `Display`, `Debug`, and `std::error::Error` trait surfaces
  of AFM-0026:R1 error types are part of the v0.1 semver contract per
  the extension of AFM-0026:R3. New trait impls may be added in minor
  versions; existing trait impls MUST NOT be removed or reshaped.
  Field-shape stability of error variants themselves is already
  governed by AFM-0026:R3 and remains unchanged.

R4 [4]: Future error types added to the AFM-0026:R1 surface —
  including any type added under AFM-0026:R5's "ADR with
  current-consumer justification" clause — inherit R1 by construction.
  No per-type follow-up ADR is required to establish
  Display/Debug/std::error::Error coverage. A successor ADR is
  required only to RELAX this rule.

## Consequences

+ becomes easier: bridge-stage `adr-srv` work uses idiomatic Rust
  error handling — `?` into `Box<dyn Error>`,
  `tracing::error!(?e, ...)`, `panic!("{e}")`, and
  `#[derive(thiserror::Error)] #[from] LoadError` all compile. The
  variant-match shims currently in `adr-srv/src/lib.rs:27-30` and
  `adr-srv/tests/smoke.rs:24-29` (introduced as a workaround in commit
  `4a91a9d`) become unnecessary and will be removed when AFM-0028 is
  ratified and `LoadError` gains the trait impls.
− becomes harder: every new public error type added to the
  AFM-0026:R1 surface MUST include the three trait impls before merge
  — an extra ~5-line burden per type. `thiserror` is permitted and
  reduces this burden but is not mandated.
risks/migration: `LoadError` migration is small and semver-compatible
  — `#[derive(Debug)]`, a hand-rolled or `thiserror`-derived
  `Display`, and a trivial `impl std::error::Error for LoadError {}`
  (or `thiserror`-derived). No `LoadError` variant fields change.
  Downstream consumers gain capabilities; none lose them. The
  implementation is the responsibility of the follow-up sub-mission
  (status-flip + impl + adr-srv shim cleanup), NOT this draft mission.
