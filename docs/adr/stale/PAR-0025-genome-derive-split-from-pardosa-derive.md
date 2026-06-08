# PAR-0025. pardosa-genome-derive — Genome Wire-Format Derive Carrier

Date: 2026-05-20
Last-reviewed: 2026-05-20
Tier: A
Status: Accepted

## Related

References: PAR-0024, GEN-0035, GEN-0037

## Context

PAR-0024:R2 mandated that `pardosa-derive` emit derive macros only
for the substrate-agnostic event-invariant traits (`EventSafe`,
`Validate`), and that substrate-specific derives such as
`GenomeSafe` "extend the stack from their owning crate rather than
duplicating macro infrastructure." The initial v0.1 implementation
landed `#[derive(GenomeSafe)]` in `pardosa-derive` as a transitional
single-crate compromise. The compromise violates PAR-0024:R2
literally: `GenomeSafe` is genome-wire-format-specific (per
GEN-0035's trait stack `EventSafe ⊂ GenomeSafe ⊂ GenomeOrd`) and
must therefore not live in the substrate-agnostic crate.

PAR-0024:R4 forbids `pardosa-derive` from depending on any
substrate crate; today the dependency edge points the right way
(none), but the *macro source* hosted in `pardosa-derive` emits
calls into `pardosa_genome`'s trait surface — a substrate-specific
output betraying R4 in spirit even where the Cargo edge is clean.

## Decision

R1 [4]: Create `crates/pardosa-genome-derive/` as a workspace
  member in the GEN domain (see R5). The crate hosts the
  `#[derive(GenomeSafe)]` proc-macro and any future
  genome-wire-format-specific derives.

R2 [4]: Remove `#[derive(GenomeSafe)]` and all genome-specific
  emission code (schema-hash computation, GenomeSafe trait-path
  emission, genome-specific reject diagnostics) from
  `pardosa-derive`. `pardosa-derive` retains the
  substrate-agnostic `Encode`/`Decode` derive infrastructure and
  reserves slots for the forthcoming `#[derive(EventSafe)]` and
  `#[derive(Validate)]` macros.

R3 [4]: `pardosa-genome` depends on `pardosa-genome-derive`
  (previously `pardosa-derive`) and re-exports `GenomeSafe` from
  its own crate root. Downstream consumers continue to write
  `use pardosa_genome::GenomeSafe;` and `#[derive(GenomeSafe)]`
  with no source-level migration. The re-export is the migration
  surface.

R4 [4]: `pardosa-genome-derive` may depend on `pardosa-derive` to
  reuse the substrate-agnostic codec emission machinery; the
  reverse dependency is forbidden, preserving PAR-0024:R4 in the
  new topology.

R5 [5]: Domain placement: `pardosa-genome-derive` belongs to the
  GEN domain of `adr-fmt.toml` because its scope is the genome
  wire format. Update `[domains.GEN]` to add the crate.
  `pardosa-derive` remains in the PAR domain per PAR-0024:R3.

R6 [5]: The split lands as a single atomic commit covering: new
  crate scaffolding (Cargo.toml, src/lib.rs, README), file moves
  with `git mv` to preserve history where possible, `pardosa-genome`
  dependency rewiring, the re-export from `pardosa-genome::lib.rs`,
  workspace `members` update in root Cargo.toml, and the
  `[domains.GEN]` entry in `adr-fmt.toml`.

## Consequences

+ becomes easier: introducing a second substrate (e.g. a hypothetical
  `pardosa-arrow` wire format) without coupling its derive to
  `pardosa-derive`; visual inspection of `Cargo.toml` reveals which
  crate hosts which derive; PAR-0024:R4's tripwire (substrate
  leakage into the agnostic crate) is now also a *visible* tripwire
  in the source layout.

− becomes harder: a one-time consumer churn (none in v0.1 — the
  re-export from `pardosa-genome` covers all current users
  including `gh-report` and forthcoming `adr-srv`); the workspace
  acquires one more crate to track.

risks/migration: the re-export must produce a byte-identical
`#[derive(GenomeSafe)]` expansion; verified by running the existing
trybuild compile-pass fixtures in `pardosa-genome/tests/compile_pass/`
which already exercise the derive surface. If the re-export breaks
attribute-resolution or span-info, the trybuild fixtures fail before
commit. cherry-pit-core and gh-report are verified by
`cargo test --workspace --all-features` pre-push.
