# COM-0021. Design Public Interfaces for Additive Evolution

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0013

## Context

Rich Hickey's "Spec-ulation" argues software should grow by accretion and relaxation, never breakage. Hyrum's Law warns all observable behaviors will be depended on. Kleppmann extends this to persisted schemas: forward/backward compatibility requires tagged fields, optional new fields with defaults, and never-reused field numbers. Cherry-pit's event-sourced persistence makes breakage especially costly — stored events must remain loadable indefinitely. COM-0013 addresses evolutionary design architecturally but provides no interface-level guidance. `#[non_exhaustive]` on error enums, additive event schema evolution, and Genome's compatibility contract already apply this independently.

## Decision

Public types exposed to downstream consumers must allow additive
evolution without breaking existing callers. Breaking changes require
explicit justification and a migration path.

R1 [5]: Public enum types use non_exhaustive so new variants can be
  added without breaking downstream match expressions
R2 [5]: New fields on persisted types are optional with defaults so
  existing serialized data deserializes without migration
R3 [6]: Removed public items are deprecated for at least one release
  cycle before removal; removal requires a superseding ADR
R4 [5]: Interface contracts specify what is guaranteed and what is
  implementation detail; only guaranteed behaviors form the
  compatibility surface
R5 [6]: Wire format field identifiers are stable and never reused
  after removal to prevent deserialization ambiguity

## Consequences

Downstream consumers update dependencies without code changes for minor releases. Every public item becomes a permanent commitment — interfaces must be designed carefully upfront (COM-0008). Persisted data schemas carry the heaviest commitment: field numbers and variant discriminants are effectively permanent. This principle does not prevent breaking changes; it requires them to be deliberate, documented via superseding ADR, and accompanied by migration guidance.
