# PAR-0024. pardosa-derive — Substrate-Agnostic Event-Invariant Carrier

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: PAR-0006, GEN-0001

## Context

PAR-0006 established `pardosa-genome` as the primary serialization
format and acknowledged that `#[derive(GenomeSafe)]` requires a
companion proc-macro crate. The genome typing v2 work (decision record
`.ooda/decision-genome-structural-20260514T214716Z.md`, decisions D-α
in-house canonical encoding and D-γ xxh3-128) generalises the trait
surface from `GenomeSafe` (genome-specific) to a sealed stack
`EventSafe ⊂ GenomeSafe ⊂ GenomeOrd`, where `EventSafe` and
`Validate` are substrate-agnostic event invariants reusable beyond
the genome wire format. Substrate-agnosticism is realised via the
in-house canonical encoding ratified in GEN-0035 — the carrier crate
hosts the sealed event-invariant traits and the derive macros that
emit conformant `Encode`/`Decode` implementations against that spec,
with no dependency on any wire-format substrate. The original crate
name framed the crate as a derive helper for the genome wire format
alone, contradicting the new role as the carrier of cross-substrate
event-invariant traits. Renaming to `pardosa-derive` and lifting the
crate into the PAR domain matches the new responsibility surface.

## Decision

`pardosa-derive` is the substrate-agnostic event-invariant carrier
crate in the PAR domain. It hosts sealed event-invariant traits
(`EventSafe`, `Validate`) and the derive macros that emit conformant
implementations. The genome-specific `GenomeSafe` and `GenomeOrd`
extend this stack and continue to live in `pardosa-genome`. The
runtime sibling `pardosa::Dragline<T>` is a separate concept and is
not derived from `pardosa-derive`.

R1 [4]: Host the sealed event-invariant trait stack EventSafe and
  Validate in the pardosa-derive crate so substrates beyond the
  genome wire format may consume the same invariant surface
R2 [4]: Emit derive macros from pardosa-derive only; substrate-specific
  derives such as GenomeSafe extend the stack from their owning crate
  rather than duplicating macro infrastructure
R3 [5]: Place pardosa-derive in the PAR domain of adr-fmt.toml
  alongside the pardosa runtime crate to reflect substrate-agnostic
  scope
R4 [5]: Forbid pardosa-derive from depending on pardosa-genome or any
  substrate crate so the trait carrier remains reusable across future
  substrates
R5 [6]: Document in PAR-0024 and the crate README that pardosa-derive
  the crate and pardosa::Dragline the runtime type are sibling
  concepts and never confused for derived-from relationships

## Consequences

+ becomes easier: introducing a second substrate that needs the
  EventSafe invariants without forking the derive infrastructure;
  reasoning about which traits are wire-format-specific (genome) vs
  invariant-only (event); future ADR authoring under the PAR domain
  for the trait carrier crate.
− becomes harder: short-term reader confusion between the renamed
  crate and the unchanged `pardosa::Dragline<T>` runtime type;
  migration friction for every external citation of the former name
  (resolved by sub-mission 0's mechanical rewrite step).
risks/migration: rename completed in a single atomic commit covering
  workspace `members`, `adr-fmt.toml` PAR/GEN domain mapping, and all
  in-tree citations. PAR-0024 establishes the rationale; subsequent
  GEN-0035..0041 build on the substrate-agnostic framing this ADR
  ratifies. If a future substrate-specific concern leaks into
  `pardosa-derive`, R4 is the tripwire — the violation is observable
  in `Cargo.toml` dependency edges.
</content>
</invoke>