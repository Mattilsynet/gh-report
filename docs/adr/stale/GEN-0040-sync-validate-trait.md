# GEN-0040. Sync Validate Trait

Date: 2026-05-15
Last-reviewed: 2026-05-15
Tier: A
Status: Accepted

## Related

References: GEN-0036, CHE-0008, GEN-0039

## Context

The v2 typing refresh adds bounded wrappers and custom event types
that check invariants on construction. The invariant-check trait is a
one-time decision with wide blast radius — every event, aggregate,
and wrapper names it.

Two axes:

1. **Sync vs async.** Async (`async fn` or RPITIT) admits I/O-bound
   validators. Sync confines them to pure decision logic.
2. **Error: fixed vs associated.** Fixed `-> Result<(), EventError>`
   forces canonical error space; associated permits narrower sets.

CHE-0008 fixes `HandleCommand::handle(&self, cmd) -> Result<Vec<Event>,
Error>` — sync, pure, no I/O. Validation is the same decision phase
one layer shallower: admitting `async` contradicts that discipline
and forces validators onto an executor for no semantic gain. The
error axis is independent: fixed `EventError` is ergonomic but
pretends a length-bounded wrapper might return `NotFound`. Associated
`Error` preserves precision.

## Decision

`Validate` is a synchronous trait with an associated `Error` type:

```rust
pub trait Validate {
    type Error;
    const COST: ValidationCost = ValidationCost::Cheap;
    fn validate(&self) -> Result<(), Self::Error>;
}
```

Four properties earn their keep:

1. **Sync signature.** No `async`, no `async_trait`, no RPITIT.
   Validators are pure decisions — no I/O, no global state, no
   observable side effects. The rationale is cited from CHE-0008
   directly: command handling is pure, validation is the same
   decision phase, and `async` would force every consumer onto an
   executor without earning a semantic benefit.
2. **Associated `Error` type.** Bounded wrappers (F sub-mission)
   can use a narrower error type when their failure space is
   genuinely smaller; canonical event types use
   [`EventError`](../genome/GEN-0039-timestamp-and-event-error-representations.md)
   for uniform `?`-chaining.
3. **`&self` receiver, no `&mut`.** Validation never mutates the
   value under inspection — symmetric with CHE-0008's
   `handle(&self, …)`.
4. **Declared-with-default `COST: ValidationCost`.** Each impl
   declares an upper-bound classification — `Free`, `Cheap`,
   `Bounded { ops }`, `Unbounded` — so callers (gateways, batch
   validators) can budget admission control without inspecting the
   impl body. The default is `Cheap`, matching the O(1) shape of
   every workspace impl as of this amendment (the bounded wrapper
   family from sub-mission F). Existing impls inherit the correct
   classification without an explicit override; only impls whose
   work exceeds O(1) need to act. `Unbounded` is the load-bearing
   variant — it shifts rate-limiting responsibility to the caller
   (GEN-0040:R7).

Placement: `Validate` lives in `pardosa-traits` (GEN-0036 substrate
crate), reachable from `pardosa-genome` via the re-export block.
External crates impl `Validate` for their own types without
depending on `pardosa-traits` directly.

The derive macro (in `pardosa-derive`) does not emit `Validate`
impls in this revision — derive support can be added when a
mechanical derivation rule is needed (e.g. propagate `validate`
through every field for struct types). The hand-written impl path
is sufficient for the F1 reframed test and for the F sub-mission's
bounded wrappers.

R1 [4]: `Validate::validate` is synchronous; no `async`, no
  `async_trait`, no RPITIT
R2 [4]: `Validate` has an associated `Error` type so bounded
  wrappers can express narrower error sets than `EventError`
R3 [4]: Validators must be pure functions per CHE-0008 — no I/O,
  no global state, no observable side effects
R4 [4]: `Validate` lives in `pardosa-traits` and is re-exported
  from `pardosa-genome` for the standard public surface
R5 [4]: `Validate` carries an associated `const COST: ValidationCost`
  with default `ValidationCost::Cheap` — declared-with-default so
  existing impls inherit the correct classification without an
  explicit override
R6 [4]: Impls whose `validate` work exceeds O(1) SHOULD override
  `COST` with `Bounded { ops }` or `Unbounded`; `Free` is reserved
  for structural no-ops the compiler can elide
R7 [4]: `ValidationCost::Unbounded` REQUIRES caller rate-limiting;
  impls choosing it MUST document the rate-limiting obligation on
  the impl's `validate` doc-comment

## Consequences

- **Positive:** Validation is testable as a pure function — given a
  constructed value, assert `Ok(())` or `Err(E)` without an
  executor.
- **Positive:** The trait composes with CHE-0008's command handler:
  validators run in the same pure decision phase, no impedance
  mismatch.
- **Positive:** Associated `Error` lets bounded wrappers (F) keep
  their error type as narrow as their actual failure space.
- **Negative:** Validators that genuinely need I/O (e.g. uniqueness
  checks against external state) cannot fit `Validate` and belong
  in command handling proper. The trait's narrowness is the
  feature.
- **Negative:** Purity is convention, not compiler-enforced — same
  caveat as CHE-0008's `handle`. Enforcement relies on code review
  and on the social signal of citing CHE-0008 in the trait's
  doc-comment.
