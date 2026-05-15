# ADR-010: Fallible constructors replacing debug_assert

**Status:** Accepted

### Context

The initial `Fiber::new` implementation used `debug_assert!` for invariant
checks: `len >= 1`, `current >= anchor`. These guards vanish in release builds.
A malformed `Fiber` silently corrupts the data model — every downstream
operation (advance, read, migration reindex) assumes these invariants hold.

Similarly, `Fiber::advance` had no bounds check — a caller could set
`new_current` to a value less than `current`, violating the singly-linked-list
invariant silently.

The distributed systems review (pardosa.md §H1, §H3) identified both as
high-risk.

### Decision

Replace `debug_assert!` with fallible constructors:

- `Fiber::new()` returns `Result<Fiber, PardosaError>`. Validates:
  - `anchor` is not `Index::NONE`
  - `current` is not `Index::NONE`
  - `len >= 1`
  - `current >= anchor`

- `Fiber::advance()` returns `Result<(), PardosaError>`. Validates:
  - `new_current` is not `Index::NONE`
  - `new_current > current` (strictly greater)
  - `len` does not overflow

Deserialization is validated via `#[serde(try_from = "FiberRaw")]` — a raw
struct deserializes first, then converts through `Fiber::new()`. This
prevents invariant bypass through serde.

### Consequences

- **Positive:** Invariants enforced in all build profiles — debug, release,
  test, production.
- **Positive:** `#[serde(try_from)]` closes the deserialization bypass gap.
  Malformed JSON/genome input is rejected at the boundary.
- **Positive:** `FiberInvariantViolation` error variant provides specific,
  actionable error messages.
- **Negative:** Every call site must handle `Result`. Acceptable given that
  these are internal operations — not hot-path user-facing APIs.
- **Negative:** `Fiber::is_empty()` always returns `false` (invariant:
  `len >= 1`). Required by Clippy for types with `len()`, despite being
  a constant.

### References

- `src/fiber.rs`: `Fiber::new()`, `Fiber::advance()`, `FiberRaw`,
  `#[serde(try_from)]`
- `src/error.rs`: `PardosaError::FiberInvariantViolation`
- pardosa.md §H1, §H3 (Distributed Systems Review)
- pardosa-next.md §P3
