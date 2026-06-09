//! Type-level assertions for `AppState` and `build_router`.
//!
//! These tests prove at compile time that the public generics carry
//! the bounds axum requires for state — `Clone + Send + Sync +
//! 'static` — and that `build_router` is callable with any
//! `(G, S, R)` triple satisfying the documented bounds (CHE-0049 R1 +
//! CHE-0050 R2). They do *not* instantiate any concrete
//! gateway/store/router impl: behavioural coverage lives in the
//! smoke test (`command_router_smoke.rs`) and S6 integration tests.
//!
//! If any of these helpers fail to compile, the public API has
//! regressed against CHE-0049 R1 or CHE-0050 R2.
//!
//! Every helper in this file is a compile-time bound check that is
//! intentionally never called at runtime — `dead_code` is the expected
//! state, and #[`expect`] fails closed when a helper gains a real caller
//! (which would be a sign the bound check has been turned into a
//! behavioural test and should be moved elsewhere).

use axum::Router;
use cherry_pit_core::{Aggregate, CommandGateway, EventStore};
use cherry_pit_web::{AppState, CommandRouter, LayerLimits, build_router};
use serde::Serialize;

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}
fn assert_clone<T: Clone>() {}
fn assert_static<T: 'static>() {}

#[expect(
    dead_code,
    reason = "compile-time bound check; intentionally never called at runtime"
)]
fn appstate_is_axum_state_compatible<G, S, R>()
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    assert_send::<AppState<G, S, R>>();
    assert_sync::<AppState<G, S, R>>();
    assert_clone::<AppState<G, S, R>>();
    assert_static::<AppState<G, S, R>>();
}

#[expect(
    dead_code,
    reason = "compile-time bound check; intentionally never called at runtime"
)]
fn build_router_is_callable<G, S, R>(state: AppState<G, S, R>) -> Router
where
    G: CommandGateway,
    S: EventStore<Event = <G::Aggregate as Aggregate>::Event>,
    <G::Aggregate as Aggregate>::Event: Serialize,
    R: CommandRouter<Gateway = G> + Clone + Send + Sync + 'static,
{
    build_router(state, LayerLimits::permissive_for_tests(), Router::new())
}

#[test]
fn type_level_bounds_compile() {}
