# COM-0033. Single-Threaded Ownership of Decisions and Components

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: B
Status: Accepted

## Related

References: COM-0014, COM-0018, GND-0007

## Context

COM-0018 establishes single-writer at runtime; COM-0014 splits by
rate of change. Neither addresses ownership at the decision and
component level — who is accountable for the continued health of a
service, ADR domain, or module. Decision-by-committee drifts;
multiple maintainers without a designated owner evolve
inconsistently. Bezos's single-threaded leader maps to module and
domain ownership.

Three options:

1. **Shared ownership** — broad bus factor, slow, drifts.
2. **Committee gates** — consistent, high-latency.
3. **Single-threaded ownership with succession** — one accountable
   owner; ownership recorded; transfer explicit.

Option 3 chosen: mirrors the runtime single-writer pattern.

## Decision

Every component, ADR domain, and architectural decision has exactly
one accountable owner. Ownership is recorded, transferable, and
visible; co-ownership without a tiebreaker is treated as no
ownership.

R1 [5]: Record the accountable owner for each crate, ADR domain,
  and externally-visible service in a CODEOWNERS or equivalent
  file derived from a single canonical source
R2 [5]: Distinguish reversible from irreversible decisions in
  every ADR; reversible decisions ship with one approver,
  irreversible decisions require the recorded owner plus review
R3 [6]: Transfer ownership through an explicit handover that
  updates the canonical record and notifies dependents in the
  same change set
R4 [5]: When a decision touches two owned components, name the
  owning ADR's domain owner as the tiebreaker rather than
  resolving by consensus

## Consequences

- **Pairs with COM-0018.** Single-threaded concurrency at runtime
  is mirrored by single-threaded ownership at design time.
- **Reduces decision latency.** Type-2 (reversible) decisions move
  at the speed of one owner; only Type-1 incur full review.
- **Cost.** Ownership creates accountability load; succession
  planning is required to prevent bus-factor exposure.
- **Pairs with COM-0027.** The CODEOWNERS file is itself an SSOT
  candidate — derive other access lists from it.
