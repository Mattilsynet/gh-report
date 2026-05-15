# COM-0024. Testing Strategy Aligned to Architectural Layers

Date: 2026-04-28
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: COM-0012, GND-0005

## Context

Cohn's test pyramid prescribes many unit tests; Dodds' testing trophy and Spotify's honeycomb invert this toward integration tests. Fowler's meta-analysis resolves the contradiction: the right distribution depends on where risk lives. Bernhardt's "Boundaries" separates pure functional core (fast unit tests) from thin imperative shell (integration tests), eliminating mocking by architecture. Hughes/Wayne demonstrate property-based testing finds bugs example-based tests systematically miss. Cherry-pit already applies layer-aligned testing: compile-fail tests for type contracts, property-based tests for invariants, golden-file tests for serialization, exhaustive state-machine coverage, and structured fuzzing for codecs.

## Decision

Each architectural layer is tested with the verification tool
appropriate to its abstraction level. The test strategy mirrors
the dependency rule — inner layers use faster, more exhaustive
verification; outer layers use broader integration tests.

R1 [5]: Type-system contracts verified by compile-fail tests use
  trybuild to assert that invalid type combinations produce
  compiler errors
R2 [5]: Domain logic invariants verified by property-based tests
  use generated inputs and shrunken counterexamples to cover
  the input space beyond hand-written examples
R3 [5]: Serialization stability verified by golden-file tests
  compares byte-level output against committed reference fixtures
  to detect unintentional format changes
R4 [5]: Infrastructure adapter correctness verified by integration
  tests exercises the full adapter stack including filesystem or
  network operations against real or in-process dependencies
R5 [6]: State machines with finite transition tables are tested
  exhaustively over all state-action pairs including invalid
  transitions
R6 [6]: Distributed invariants such as replay ordering, idempotency,
  fencing, corruption handling, and crash recovery are verified with
  property, fault-injection, or golden-fixture tests

## Consequences

Test failures localize to the correct architectural layer — a compile-fail test failure points to a type contract regression, not an integration issue. Property-based tests catch edge cases hand-written examples miss, but require investment in writing meaningful properties. Golden-file tests are brittle to intentional format changes. The strategy does not prescribe coverage targets; mutation testing is a more honest quality signal. Mocking is minimized by architecture rather than discipline. This principle does not mandate TDD; it mandates the verification tool matches the abstraction layer.
