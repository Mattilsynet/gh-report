# AFM-0011. Meadows-Aligned Tier Classification

Date: 2026-04-28
Last-reviewed: 2026-05-01
Tier: S
Status: Accepted

## Related

Supersedes: AFM-0010
References: AFM-0001, GND-0008

## Context

AFM-0010 introduced a five-tier system using blast-radius framing
("If this changed, would X change?"). This diverges from Meadows'
leverage-point hierarchy in two ways: over-classifying high-impact
parameters and under-classifying information flows. System-
characteristic framing ("Does this decision define X?") corrects
both misalignments while splitting Self-organization from Design
to prevent bucket overload. Tiers stratify decisions by leverage
depth, distinct from the AFM-0020 parent-edge tree which answers
"which decision does this one specialize?". Higher tiers appear
first in `--context` output, exploiting LLM primacy bias so
foundational constraints precede implementation details.

## Decision

Classify ADRs by system characteristic using five tiers aligned
with Meadows' leverage-point hierarchy. Use system-characteristic
framing ("Does this decision define X?") instead of blast-radius
framing ("If this changed, would X change?").

R1 [5]: Classify ADRs using the first-yes-wins method: start at
  S-tier and assign the first tier whose classification question
  yields "yes"
R2 [5]: Frame tier classification questions as "Does this decision
  define X?" to classify by leverage type rather than blast radius
R3 [5]: Map tiers to Meadows' leverage hierarchy: S=Intent (levels
  1-3), A=Self-organization (level 4), B=Design (levels 5-6),
  C=Feedbacks (levels 7-8), D=Parameters (levels 9-12)

## Consequences

Information flows (CI gates, observability) now classify as B-tier
rather than old C-tier. Parameter values classify as D-tier
regardless of blast radius. All existing ADRs previously assigned
tier A, B, or C must be re-evaluated under system-characteristic
questions. The A-tier classification question references Rust
artifacts (traits, generics, plugin boundaries); extensibility
mechanisms not expressed through these may fall to B-tier — accepted
as rare edge cases.

**Asymmetric T019 bound.** T019 fires iff a rule's layer-derived tier
has higher leverage than the ADR's tier (`rule_rank < adr_rank`).
Rules at equal or lower leverage than their ADR tier pass silently —
lower-leverage enforcement within a higher-leverage decision reflects
the rule's intervention type, not a classification error. No
domain-specific carve-outs apply; the asymmetric bound is uniform
across all domains. See AFM-0012:R4 and AFM-0024.
