# AFM-0008. Domain-Scoped Prefix Naming Convention

Date: 2026-04-27
Last-reviewed: 2026-05-02
Tier: S
Status: Accepted

## Related

References: AFM-0001

## Context

A growing ADR corpus requires a naming scheme providing global
uniqueness (unambiguous cross-domain references), domain affinity
(identifier reveals which domain without consulting an index), and
sortable ordering (filesystem sorting matches creation order). The
`PREFIX-NNNN` scheme satisfies all three: a 2–4 letter uppercase
domain code plus a zero-padded four-digit sequence number. Filename
extends this with a kebab-case slug for human-readable context in
directory listings and git logs. Domains partition the corpus by
rate of change and audience: Ground (epistemic foundations), Common
(cross-cutting principles), Rust (platform), Security (qualities),
domain-specific (architecture). Each domain has a distinct rate of
change; a decision spanning two domains at equal weight triggers a
scoping discussion and may produce a boundary ADR with cross-references.

## Decision

Every ADR filename follows `PREFIX-NNNN-kebab-slug.md` where PREFIX
is a configured domain code and NNNN is a zero-padded sequence
number.

R1 [5]: Match filename to `PREFIX-NNNN-kebab-slug.md` and confirm
  H1 title contains the same `PREFIX-NNNN` identifier —
  validated by N001, N002, N003
R2 [5]: Record domain prefixes in `adr-fmt.toml` under `[[domains]]`
  and permit `adr-fmt` to trigger warning N004 for any unregistered prefix
R3 [5]: Bind permanent, non-recycling sequence numbers within each
  domain and permit gaps left by rejected or superseded ADRs
R4 [5]: Name slug segments as lowercase kebab-case — letters, digits,
  and hyphens only, with at least one letter segment, rejecting
  leading, trailing, and consecutive hyphens (validated by N003)

## Consequences

Cross-domain references are unambiguous (`References: GEN-0007`
identifies exactly one ADR). Directory listings sort chronologically
within each domain. Adding a new domain requires only a config
entry — no code changes. The 9,999 ADR-per-domain limit is
sufficient for any realistic project.
