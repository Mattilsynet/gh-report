# COM-0030. Structural Symmetry Across Scales

Date: 2026-04-30
Last-reviewed: 2026-05-12
Tier: A
Status: Accepted

## Related

References: COM-0012, COM-0014, COM-0009

## Context

COM-0014 aligns boundaries to rate of change; COM-0009 makes
similar things look similar; COM-0012 fixes inward-only dependency
direction. Neither of the first two states the cross-scale
property: the same patterns — port/adapter, command/event, validate-
then-commit, dependency inversion — should hold at function, module,
crate, and service scale. This ADR generalizes COM-0012's direction
rule to every scale. Cherry-pit already shows partial symmetry
(hexagonal at crate and workspace scale). This ADR makes it
normative so future structures inherit the property.

Three options evaluated:

1. **Per-scale optimization** — locally best, globally fragmented.
2. **Symmetry as accident** — works until pressure bends one scale.
3. **Symmetry as design constraint** — declare cross-scale patterns
   and reject local optimizations that break them.

Option 3 chosen: tier-A because it governs *where* new structure
can be added.

## Decision

Apply the same architectural patterns at every scale of the system.
Where a pattern holds at one scale, it should hold at the scales
above and below unless the asymmetry is justified by a property
unique to that scale.

R1 [4]: Apply port-and-adapter separation at every scale — function
  parameters, module boundaries, crate APIs, and service contracts —
  so dependency direction matches at every level
R2 [4]: Use the same vocabulary across scales; a concept named
  command at the function level is named command at the module,
  crate, and service level rather than renamed per layer
R3 [5]: Compose larger structures from instances of the smaller
  pattern rather than introducing a new pattern at the larger
  scale; service-level composition mirrors module-level composition
R4 [4]: When a scale resists the dominant pattern, document the
  property unique to that scale that justifies the asymmetry; an
  unjustified break is a refactoring trigger

## Consequences

- **Pairs with COM-0014.** Rate-of-change boundaries determine
  *where* to split; symmetry determines *how* the resulting parts
  relate. Both are needed.
- **Compounds learning.** New contributors learn one pattern and
  apply it at any scale they encounter, reducing onboarding time.
- **Cost.** Some scales may have a locally simpler alternative;
  symmetry forgoes that local win for a system-level coherence win.
- **Audit hook.** Cross-scale audits — "does this crate's API look
  like its modules look like its functions?" — become valid review
  questions.
