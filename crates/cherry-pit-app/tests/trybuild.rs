//! trybuild driver — runs every `tests/compile_fail/*.rs` fixture and
//! asserts each fails to compile with a stderr matching the paired
//! `*.stderr` snapshot.
//!
//! Per S7 §1: locks two negative-space invariants of the agent surface
//! that linus called out as "the type system should already be doing
//! this work":
//!
//! - `trybuild/mismatched_event_types.rs` — `App::new` rejects an
//!   `EventBus::Event` that doesn't agree with the gateway's aggregate
//!   event (CHE-0051:R3).
//! - `trybuild/policy_closure_missing_ctx.rs` — `register_policy`'s
//!   closure must take `(P::Output, &G, CorrelationContext)`; dropping
//!   the `CorrelationContext` parameter is a hard compile error
//!   (CHE-0051:R4 + R6 amendment).

#[test]
fn compile_fail_fixtures() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
