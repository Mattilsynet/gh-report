# COM-0032. Question Every Requirement — First-Principles Justification

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: A
Status: Accepted

## Related

References: COM-0001, GND-0001

## Context

COM-0013 prefers evolutionary design; COM-0011 mandates trade-off
analysis. Neither requires interrogating *why a requirement exists
at all*. Requirements arrive carrying authority — a ticket, a spec,
an industry norm — and that authority short-circuits the question
"is this requirement actually needed?" Inherited requirements are
the largest source of accidental complexity because no one
remembers why they exist.

Three options:

1. **Accept stated requirements** — fastest; carries forward
   accidental complexity indefinitely.
2. **Periodic requirement audits** — episodic; loses context.
3. **Per-change requirement interrogation** — every retained
   requirement names the failure its absence would cause.

Option 3 chosen: tier-A because it gates whether structure exists
at all.

## Decision

Every requirement informing a design decision must be reduced to
the first principle it serves. Requirements without an articulable
failure mode on removal are deleted, not implemented.

R1 [4]: State the failure mode each retained requirement prevents
  in concrete terms — what breaks, who is harmed, what invariant
  is violated — and record it in the Context of the relevant ADR
R2 [4]: Reject requirements whose justification reduces to "best
  practice," "industry standard," or "we have always done it";
  promote them to first-principle form or drop them
R3 [5]: When implementing a feature, derive the simplest mechanism
  that satisfies the named failure mode rather than reproducing the
  shape of an existing analogous solution
R4 [5]: When a requirement and a constraint conflict, surface the
  conflict explicitly so the trade-off is recorded in the ADR
  rather than resolved silently in code

## Consequences

- **Pairs with COM-0026.** Subtractive design needs a justification
  test; first-principles framing is that test.
- **Pairs with COM-0011.** Trade-off analysis presumes the
  alternatives are real; first-principles framing keeps "do
  nothing" on the option list.
- **Cost.** Requirement interrogation slows feature intake. The
  wager: prevented complexity dominates delay.
- **Cultural.** Reviewers and ADR authors gain license to ask
  "why does this exist?" without it being adversarial.
