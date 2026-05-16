# CHE-0058. Carve-out: `pub mod` permitted for feature-gated test fixtures

Date: 2026-05-16
Last-reviewed: 2026-05-16
Tier: A
Status: Accepted

## Related

References: CHE-0030, CHE-0029

## Context

CHE-0030:R1 forbids `pub mod` in `lib.rs`; the public API is flat,
modules are private, items are re-exported via `pub use`. Phase 2 v2
Track 1 introduces `cherry_pit_core::testing` — a feature-gated module
carrying `FakeBus`, `InMemoryEventStore`, `InMemoryProjectionStore` for
the conformance harness. Fixtures are intentionally namespaced so
production code cannot depend on them. Three shapes considered:
(1) flat re-export `pub use testing::*` collapses prod and test
surface; (2) `pub use testing as <other>` rustc rejects (E0365 / E0255);
(3) `#[cfg(any(test, feature = "testing"))] pub mod testing;` keeps
namespace but contradicts CHE-0030:R1 as written. Option 3 is the only
shape preserving surface segregation without fighting the compiler.

## Decision

CHE-0030:R1 is amended (not superseded): `pub mod` remains forbidden
**except** when the declaration is gated by a `#[cfg(...)]` attribute
that restricts the module to test or testing-feature builds. Production
builds remain flat-namespaced; the carve-out applies only to surface
that is invisible by default.

R1 [4]: A `pub mod` declaration in a cherry-pit crate's `lib.rs` is
  permitted only when it is immediately preceded by a `#[cfg(...)]`
  attribute whose predicate contains either `test` or
  `feature = "testing"`; all other `pub mod` declarations remain
  forbidden per CHE-0030:R1.

R2 [4]: A crate exposing a feature-gated `pub mod testing;` MUST
  declare `testing = []` under `[features]` in its `Cargo.toml`; the
  feature MUST NOT enable additional dependencies (CHE-0029:R4
  inviolate).

R3 [4]: The carve-out is for `testing` fixtures only. Adding a second
  feature-gated public module requires a fresh ADR citing CHE-0058 as
  parent; the M29 obligation test SHOULD reject unknown feature names
  until the new ADR lands.

## Consequences

+ becomes easier: `cherry_pit_core::testing::*` fixtures are reachable
  by downstream tests via `features = ["testing"]` without polluting
  the production namespace. The conformance harness (SM-4) can live in
  `testing::` and be called from every registrant crate.
− becomes harder: The M29 obligation test (`m29_no_pub_mod_in_lib`)
  grows a second branch — it must distinguish bare `pub mod` (still
  forbidden) from gated `pub mod` (permitted). Reviewers must verify
  the cfg predicate matches the carve-out exactly.
risks/migration: No migration. Existing crates are unaffected; only
  `cherry-pit-core` exercises the carve-out in Phase 2 v2 Track 1.
  Other crates remain bound by CHE-0030:R1 as originally written.
