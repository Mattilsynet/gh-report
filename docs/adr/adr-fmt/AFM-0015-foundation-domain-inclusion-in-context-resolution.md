# AFM-0015. Foundation Domain Inclusion in Context Resolution

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted
Parent-cross-domain: GND-0003 — foundation-domain-inclusion is the AFM-tier expression of GND-0003's universal directive that executors receive context two levels up

## Related

References: GND-0003, AFM-0008

## Context

The `--context` mode resolves which ADR rules apply to a given
crate. Domain configuration in `adr-fmt.toml` maps crate names to
domains, and each domain's ADRs contribute rules. Some domains are
marked `foundation = true` (currently COM and RST), meaning their
rules apply universally regardless of direct domain membership.
Without foundation domain inclusion, cross-cutting concerns like
commit conventions and Rust idioms would need explicit mapping to
every crate, violating DRY and risking incomplete coverage when
new crates are added.

## Decision

Foundation domains are always included in `--context` output
alongside the crate's directly mapped domain.

R1 [5]: `--context` resolution collects rules from the crate's
  mapped domain plus all domains where `foundation = true` in
  `adr-fmt.toml`
R2 [5]: Only ADRs with status Accepted contribute rules to
  `--context` output; Draft, Proposed, Deprecated, and Superseded
  ADRs are excluded
R3 [5]: Within each included ADR, only rules whose tagged layer
  falls within the ADR's declared tier range are emitted
R4 [6]: Foundation domain inclusion is configured declaratively
  in `adr-fmt.toml`, not hard-coded; adding or removing foundation
  status requires only a configuration change

## Consequences

New crates automatically inherit foundation rules without explicit
per-crate configuration. The COM and RST domains serve as the
cross-cutting governance layer. Domain maintainers can promote a
domain to foundation status by setting the flag in `adr-fmt.toml`,
making the propagation mechanism transparent and auditable. The
trade-off is that foundation rules cannot be selectively excluded
per-crate.
