# COM-0006. Interface Documentation Before Implementation

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0002

## Context

Ousterhout (Ch. 13, 15) argues documentation is a design tool. Writing interface comments before implementation forces thinking about abstraction — what and why — before getting lost in how. If you cannot describe the abstraction clearly, it is likely unclear. Comments written after implementation tend to restate function names. Cherry-pit's 685-line `cherry-pit-core.md` design document was written before implementations existed. Red flags: comments restating function names, describing implementation rather than abstraction, or absent on public APIs.

## Decision

Interface documentation is written before implementation. Comments
describe the abstraction ("what" and "why"), not the code ("how").

R1 [5]: Write doc comments for public functions, traits, and types
  before implementing them — the comment is a design artifact
R2 [5]: Interface comments answer purpose, context, guarantees,
  requirements, and motivation — never implementation details
R3 [6]: Comments that restate the function name or describe code
  rather than the abstraction must be rewritten
R4 [5]: For complex subsystems, a design document describing
  abstractions and relationships is written before any code

## Consequences

Public APIs without doc comments are flagged in review, indicating a missing design step. Design documents like `cherry-pit-core.md` are first-class artifacts capturing intent source code cannot express. Comments restating function names are refactored with a COM-0006 citation. This supports Rust's `#![warn(missing_docs)]` lint — the lint catches missing comments; COM-0006 ensures existing comments are meaningful. For distributed components, interface documentation must include failure modes and ordering guarantees — callers across process boundaries cannot inspect implementation to infer these. Over-documentation is mitigated by the "abstraction, not code" rule — implementation comments are inline, not part of the public interface.
