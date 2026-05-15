# ADR-003: Event immutability via private fields and #[non_exhaustive]

**Status:** Accepted

### Context

Events are immutable by design — once appended to the line, they must never
change. The original Go prototype used public fields, and the initial Rust
design followed suit. Two problems emerged:

1. Public fields allow post-construction mutation, undermining the append-only
   invariant at the API boundary.
2. Genome's binary layout depends on field declaration order. Public fields
   invite reordering during refactoring, which silently changes the schema hash
   and breaks binary compatibility.

### Decision

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

### Consequences

- **Positive:** Immutability enforced by the compiler — no runtime checks
  needed.
- **Positive:** `#[non_exhaustive]` allows adding fields in future versions
  without breaking downstream compilation (though adding a field still changes
  the genome schema hash — a migration is required).
- **Positive:** `GENOME LAYOUT` doc comments create a grep-able audit trail
  for field-order-sensitive types.
- **Negative:** Accessor boilerplate for each field. Acceptable for a small
  number of core types.
- **Negative:** `#[non_exhaustive]` prevents pattern matching on `Event<T>` in
  external crates. Acceptable — events should be accessed via methods, not
  destructured.
- **Cross-crate:** Field order is a shared invariant with pardosa-genome. See
  [genome ADR-001](../../pardosa-genome/adr/0001-serde-native-serialization-with-genomesafe-marker-trait.md).

### References

- `src/event.rs`: `Event<T>`, `Index`, `DomainId`
- `src/fiber.rs`: `Fiber`
- pardosa-next.md §P1, Design Invariant 9
