# COM-0020. Make the Safe Path the Only Path

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0017

## Context

Multiple independent traditions converge: Rico Mariani's "pit of success," Alexis King's "parse, don't validate," Yaron Minsky's "make illegal states unrepresentable," and Shigeo Shingo's poka-yoke — all argue the safe path should be the only path, not merely the default. COM-0003/COM-0005/COM-0017 address related concerns but none state this constraint directly. Cherry-pit applies it across domains: EventEnvelope only via `::new()`, CorrelationContext has no Default, GenomeSafe verification is inline and mandatory. Escape hatches add expert-user friction; at the application level this tradeoff favors safety.

## Decision

When an operation has safe and unsafe variants, expose only the safe
variant. "Forgot to call verify" or "constructed without validation"
must be a compile error, not a runtime bug.

R1 [5]: Constructors for types with invariants return Result or
  use types that encode validity; struct literal construction is
  blocked via private fields
R2 [5]: Types requiring multi-step initialization provide a builder
  or single constructor that performs all steps; partial construction
  is not representable
R3 [5]: When a verified and unverified variant of an operation could
  exist, expose only the verified variant; unverified use is a
  compilation failure
R4 [6]: Default trait implementations are omitted when a sensible
  default does not exist; forcing explicit construction prevents
  accidental use of meaningless zero-values

## Consequences

API surface shrinks — fewer methods, fewer misuse paths. New types require more upfront design investment to find the right constructor shape. Performance-sensitive inner loops may need purpose-built types encoding pre-validated state. Downstream consumers cannot construct invalid instances even through deserialization — serde `TryFrom` bridges the gap. The principle applies to application and domain modules, not language-level primitives.
