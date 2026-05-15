# PAR-0003. Event Immutability — Private Fields + non_exhaustive

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: PAR-0008

## Context

Events are immutable by design — once appended to the line, they must never
change. The original Go prototype used public fields, and the initial Rust
design followed suit. Two problems emerged:

1. Public fields allow post-construction mutation, undermining the append-only
   invariant at the API boundary.
2. Genome's binary layout depends on field declaration order. Public fields
   invite reordering during refactoring, which silently changes the schema hash
   and breaks binary compatibility.

## Decision

Make all fields on `Event<T>`, `Fiber`, `Index`, and `DomainId` private.
Provide a constructor and accessor methods. Mark `Event<T>` with
`#[non_exhaustive]` to prevent external construction via struct literal syntax.

Types that participate in genome encoding carry a `GENOME LAYOUT` doc comment:

```rust
/// GENOME LAYOUT: fields are serialized in declaration order.
/// Changing field order is a breaking change — `schema_id` will change.
```

This convention signals to future contributors that field reordering is a
schema migration, not a refactor.

R1 [5]: Make all fields on Event<T>, Fiber, Index, and DomainId
  private with constructor and accessor methods
R2 [5]: Mark Event<T> with non_exhaustive to prevent external
  construction via struct literal syntax
R3 [6]: Annotate genome-encoded types with a GENOME LAYOUT doc comment
  stating that field reordering is a breaking change

## Consequences

Immutability enforced by the compiler — no runtime checks needed. `#[non_exhaustive]` allows adding fields without breaking downstream compilation (though adding a field changes the genome schema hash — a migration is required). `GENOME LAYOUT` doc comments create a grep-able audit trail for field-order-sensitive types. Trade-offs: accessor boilerplate per field (acceptable for few core types) and `#[non_exhaustive]` prevents pattern matching on `Event<T>` in external crates. Field order is a shared invariant with pardosa-genome; see [GEN-0001](../genome/GEN-0001-serde-native-serialization-with-genomesafe-marker-trait.md).
