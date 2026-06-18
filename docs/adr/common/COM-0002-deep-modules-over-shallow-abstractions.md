# COM-0002. Deep Modules Over Shallow Abstractions

Date: 2026-04-25
Last-reviewed: 2026-06-18 — refined — corrected R4 and added R6 to represent APOSD 2e Ch.6 general-purpose-interface and push-specialization principles (mission:adr-fmt-sc11s)
Tier: S
Status: Accepted

## Related

References: COM-0001

## Context

Ousterhout (Ch. 4–5) models abstraction as a cost-benefit ratio: a module's value is the ratio of hidden complexity to interface complexity. Deep modules have simple interfaces hiding powerful implementations. Shallow modules have interfaces nearly as complex as their implementations. Red flags include traits with many trivial methods, wrapper types mirroring what they wrap, and "classitis." Cherry-pit's `EventStore` trait demonstrates depth: three methods hide file I/O, MessagePack serialization, concurrency locking, atomic writes, sequence validation, and aggregate ID assignment. Ousterhout (2nd ed., Ch. 6) holds over-specialization to be the single greatest source of complexity; general-purpose interfaces are correspondingly deeper, with the implementation reflecting current needs.

## Decision

Prefer deep modules: simple interfaces hiding substantial
implementation complexity. Measure module depth by the ratio of
interface complexity to implementation complexity.

R1 [2]: Every module exposes a simple interface and hides
  implementation complexity behind it
R2 [2]: Before adding a public method or type parameter, verify it
  hides more complexity than it exposes
R3 [2]: Combine small methods always called together into a single
  method; combine small types always used together into a single type
R4 [2]: Make interfaces somewhat general-purpose — the interface
  supports multiple uses while the implementation reflects current
  needs; a general interface yields less implementation code, not more.
  Resist over-generalization that complicates today's use
R5 [3]: Red flags — trait with more than five required methods, wrapper
  with pass-through methods, type parameter in only one method —
  trigger refactoring toward deeper modules
R6 [3]: Separate specialized from general-purpose code by pushing
  specialization outward — up into top-level feature code or down into
  driver-style adapters — leaving lower and shared layers
  general-purpose

### Exceptions

Some modules are intentionally shallow for type safety reasons.
`AggregateId(NonZeroU64)` is a newtype wrapper that adds no
implementation depth but prevents type confusion. These are justified
under COM-0001 (correctness > simplicity) and should be documented
as deliberate exceptions.

## Consequences

New traits are evaluated for depth before acceptance; proposals with many trivial methods must consolidate. The `EventStore` pattern (3 methods hiding 7+ concerns) benchmarks future infrastructure ports. Newtype wrappers for type safety are accepted as justified shallow modules. This creates tension with fine-grained error types (CHE-0015, CHE-0021); COM-0005 resolves it by eliminating unnecessary variants. Deep modules also create tension with observability (COM-0019): hidden complexity means hidden failure modes, resolved by requiring telemetry instrumentation at the absorption points where complexity is pulled down. "Classitis" refactoring becomes a named practice citable as COM-0002.
