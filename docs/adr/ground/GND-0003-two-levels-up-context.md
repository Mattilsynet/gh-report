# GND-0003. Directives Are Scoped Two Levels Up, No Further

Date: 2026-04-30
Last-reviewed: 2026-04-30
Tier: S
Status: Accepted

## Related

References: GND-0002

## Context

Bungay's briefing cascade gives each level the situation, the
*immediately superior* commander's intent, and the unit's role —
nothing more. Two levels up suffices because the executor must
understand the intent it is serving and the boundary that intent
sits within; deeper context is noise. Detail-as-clarity is a
recurring failure mode: more context degrades attention, increases
priming for irrelevant constraints, and slows decision-making.

Three options:

1. **Provide all context** — load the full corpus into every
   decision. Noise dominates; primacy/recency biases distort
   compliance.
2. **Provide nothing** — executor reads only the local directive.
   Cannot detect when local rule contradicts higher intent.
3. **Provide the directive plus its parent intent** — executor sees
   what to achieve, why (parent's intent), and where (boundaries).

Option 3 chosen: it is the smallest envelope that lets executors
detect when local action conflicts with higher purpose.

## Decision

Context delivered to any executor — human, agent, or tool — is
scoped to the directive plus its structural parent's intent. Tooling
that aggregates directives enforces this scope mechanically.

R1 [3]: Limit context delivered by `adr-fmt --context` (or any
  executor-facing directive aggregator) to the target ADR plus its
  first `References:` parent's intent; include deeper ancestors
  only when directly cited, because finite attention makes added
  context displace signal
R2 [3]: Make the two-level scope (directive plus structural parent)
  the default for `adr-fmt --context` and any directive-aggregator;
  require an explicit, logged CLI flag for broader delivery, so
  every scope expansion produces an auditable record
R3 [3]: Deliver tagged rules to the executor in ascending Meadows-
  layer order (L1 to L12) so paradigm rules precede parameter
  rules; let primacy bias align the executor on intent before
  constraint

## Consequences

- **Tooling alignment.** `adr-fmt --context` already implements the
  spirit of this principle by grouping by root and including
  foundation domains; this ADR makes the principle explicit and
  domain-agnostic.
- **Constrains future tooling.** Any tool that delivers directives
  to an executor must respect the two-levels-up scope by default.
- **Cost.** The executor cannot satisfy curiosity about distant
  ancestors without an explicit flag. This is the intended tax;
  it preserves attention for what matters.
- **Edge case.** When a directive's structural parent is itself
  thin, the executor may need to traverse one further. The flag
  exists for exactly this case; it is not the common path.
- **Observation mechanism (per GND-0005).** Tooling enforcement:
  `adr-fmt --context` implements the two-level scope mechanically
  by default; broader scope requires an explicit flag whose
  invocation is logged, surfacing exceptions for review.
