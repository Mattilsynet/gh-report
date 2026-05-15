# COM-0034. ADR Lifecycle Feedback — Operational Signal Triggers Review

Date: 2026-04-30
Last-reviewed: 2026-05-03
Tier: B
Status: Accepted
Parent-cross-domain: GND-0007 — operational-signal-to-ADR-review is the COM-tier expression of GND-0007's universal directive that directives have explicit lifecycle feedback

## Related

References: GND-0007, COM-0019, COM-0013

## Context

COM-0019 designs for observability; COM-0013 favors evolutionary
design. Missing: the loop from operational signal back into ADR
review. Today `Last-reviewed` captures *time since review*, not
*evidence the decision still holds*. Research shows 20-25% of
architectural decisions go stale within two months — a date stamp
does not surface drift; runtime triggers do.

Three options:

1. **Time-only cadence** — current state; misses fast drift.
2. **Manual audits** — episodic; finds drift after damage.
3. **Operational triggers** — incidents, SLO breaches, recurring
   exceptions, and contributor questions schedule review.

Option 3 chosen: closes the loop from runtime to governance.

## Decision

Operational signal — incidents, SLO breaches, recurring exceptions,
deprecation of cited dependencies, and repeated contributor
questions — schedules review of the relevant ADRs as a feed-forward
mechanism into governance.

R1 [6]: Tag every incident postmortem with the ADR IDs whose
  decisions were tested; the tagged ADRs schedule review within
  the next governance cycle
R2 [6]: Treat repeated exceptions to a rule — three or more
  documented overrides — as evidence the rule is too strict or
  too vague and trigger ADR review
R3 [6]: When a dependency cited by an ADR is deprecated, removed,
  or replaced, schedule the ADR for review in the same change
  set rather than relying on the next time-based cadence
R4 [5]: Record review outcomes — reaffirmed, amended, superseded,
  retired — in the ADR's Last-reviewed entry with a one-line
  rationale so future readers see the decision's history

## Consequences

- **Pairs with COM-0019.** Observability provides the signal;
  this ADR turns the signal into governance action.
- **Tooling pressure.** `adr-fmt` may need an ADR-tagging
  convention for postmortems; out of scope here, in scope for AFM.
- **Cost.** More ADRs reviewed per cycle. Mitigation: review is
  cheap when the trigger names the question to ask.
- **Closes a loop.** Decisions evolve from operational reality
  rather than calendar pressure alone.
