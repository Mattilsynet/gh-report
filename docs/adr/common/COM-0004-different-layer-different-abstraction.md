# COM-0004. Different Layer, Different Abstraction

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0012

## Context

Ousterhout (Ch. 7) observes that well-designed systems have layers where each provides a distinct abstraction. When adjacent layers have similar abstractions, one adds no value. Red flags include pass-through methods, pass-through variables threaded through unused layers, and signature mirroring.

Cherry-pit's store/bus layering demonstrates this: `load` returns `Vec::new()` (empty stream, no `NotFound`), while the bus returns `DispatchError::AggregateNotFound`. Each layer provides a different abstraction — the store abstracts persistence; the bus abstracts dispatch semantics.

## Decision

Adjacent layers in the architecture must provide distinct
abstractions. Each layer should add semantic value that justifies
its existence.

R1 [5]: Pass-through methods are red flags; a wrapping layer must add
  semantic value such as error translation, logging, or caching
R2 [5]: A parameter passing through two or more layers unchanged must
  be refactored into a context object, moved, or eliminated
R3 [5]: Adjacent layers must use different vocabulary for their
  concepts; identical names signal artificially split abstractions
R4 [6]: If analysis shows a layer adds no abstraction, removing the
  layer is the correct response

## Consequences

The store/bus split is validated: `load` returning `Vec::new()` vs `DispatchError::AggregateNotFound` demonstrates distinct abstractions (CHE-0019). New layers must demonstrate what abstraction they add — "separation of concerns" alone is insufficient. Pass-through methods in PRs are flagged with COM-0004. Tension with hexagonal port patterns (CHE-0004) is resolved because adapters translate between abstractions (domain types to infrastructure types), which is a valid abstraction difference.
