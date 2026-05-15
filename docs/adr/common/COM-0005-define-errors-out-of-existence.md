# COM-0005. Define Errors Out of Existence

Date: 2026-04-25
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0002

## Context

Ousterhout (Ch. 10) identifies exception handling as a major complexity source. Every error variant adds interface complexity, propagation decisions, test cases, and combinatorial state explosion. Before adding a variant, ask: "Can the operation be redefined so this error does not exist?"

Techniques: redefine operations to succeed — `load` returns `Vec::new()` instead of `NotFound` (CHE-0019); make operations infallible — `apply` returns `()` (CHE-0009); use idempotent semantics — duplicates produce `Ok(vec![])` (CHE-0041); mask exceptions when defaults suffice.

## Decision

Before adding an error variant to any `Result` type, demonstrate
that the operation cannot be redefined to succeed. Error elimination
is preferred over error handling.

R1 [5]: Before writing a new error variant, ask whether the operation
  can be redefined so the condition becomes a success case
R2 [5]: Operations whose failure would be unrecoverable must be
  infallible; truly unrecoverable conditions use panic, not Result
R3 [5]: Repeated operations with the same input return Ok with no
  side effects rather than a duplicate-operation error
R4 [6]: Each error variant requires justification explaining why the
  caller cannot avoid it, why the module cannot handle it, and what
  the caller does with it
R5 [5]: When a module can handle an error with retry, fallback, or
  empty result, it masks the exception rather than propagating it

## Consequences

`StoreError` has no `NotFound` variant — eliminated by redefining `load` to return empty vectors (CHE-0019). `apply()` is infallible by design; corrupt data panics as a bug (CHE-0009). Idempotent command handling returns `Ok(vec![])` (CHE-0041). New error variants require COM-0005 justification. Tension with explicit error reporting (CHE-0015) is resolved: COM-0005 minimizes variant *count*; CHE-0015 ensures remaining variants are typed. Tension with operational visibility is resolved by COM-0019: eliminated errors must still be observable through structured telemetry at the absorption point. Truly unrecoverable conditions (data corruption, I/O failure) must still be reported — the principle targets eliminable errors only.
