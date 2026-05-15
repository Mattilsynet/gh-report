# COM-0010. Code Should Be Obvious

Date: 2026-04-26
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0006

## Context

Ousterhout (Ch. 18) addresses the complement to documentation: code itself should minimize the need for explanation. The test is not "can the author explain this?" but "can a qualified reader understand this without asking?" Techniques include judicious naming (abstractions, not implementations), structural clarity (guard clauses, exhaustive matching), and avoiding cleverness.

Cherry-pit leverages type-level obviousness: `AggregateId(NonZeroU64)` communicates identity semantics; `apply` returning `()` makes infallibility obvious (CHE-0009); `#[non_exhaustive]` signals extensibility (CHE-0021); exhaustive `match` on domain events makes handling visibly complete.

## Decision

Code should be understandable on first reading by a qualified
developer who has not seen it before. Prefer clarity over brevity,
explicitness over cleverness.

R1 [5]: Choose names a reader unfamiliar with the implementation
  would understand; full words over abbreviations, domain terms
  over generic names
R2 [5]: Use Rust's type system to make invariants visible — newtypes
  for domain concepts, NonZero for exclusion constraints, unit
  return for infallible operations
R3 [6]: Code must not surprise the reader; side effects not implied
  by a name, hidden early returns, and implicit ordering
  dependencies are prohibited
R4 [5]: When two implementations produce the same result, choose the
  straightforward one over the clever one unless profiling justifies
  the complexity

## Consequences

Type-driven design (CHE-0002, CHE-0003) is a direct application: the type system makes constraints obvious to both compiler and reader. Naming conventions from COM-0009 and COM-0006 reinforce this. Code review can cite COM-0010 for non-obvious constructions — the bar is "would a new team member understand this on first reading?" Rust idioms (iterator chains, destructuring) are obvious to the target audience; non-idiomatic cleverness is not. When performance requires non-obvious code, COM-0006 applies: document why.
