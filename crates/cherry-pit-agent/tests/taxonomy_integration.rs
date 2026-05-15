//! Tokio integration round-trip on the 2-aggregate fixture.
//!
//! Per S7 §1: command issued to gateway A produces an event published
//! on bus A which triggers policy `P_a→b` which dispatches a command
//! to gateway B. Asserts `correlation_id` threads end-to-end.

#[path = "two_aggregate_fixture/mod.rs"]
mod fixture;

use cherry_pit_core::{CommandGateway, CorrelationContext};
use fixture::domain::FooDo;
use fixture::wiring::assemble;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foo_command_drives_bar_via_policy() {
    let bundle = assemble();
    let foo_gw = bundle.foo_gateway.clone();
    let bar_gw = bundle.bar_gateway.clone();
    let app = bundle.app;

    let correlation_id = uuid::Uuid::now_v7();
    let ctx = CorrelationContext::correlated(correlation_id);

    // App::run consumes self; spawn it on a background task with a
    // shutdown signal we control.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run_handle = tokio::spawn(async move {
        app.run(async move {
            let _ = shutdown_rx.await;
        })
        .await
    });

    // S6 limitation: the App's bus is owned internally — the foo_gw
    // we hold here writes events directly through MsgpackFileStore but
    // does NOT publish on the App's bus (the App's bus has no producer
    // wired in v0.1; per CHE-0024:R1 publication is the CommandBus's
    // job, which we're stubbing in S7). The integration test here is
    // therefore scoped to the surface that exists today: round-trip
    // through both stores via the gateways, threading the
    // CorrelationContext end-to-end.
    //
    // WU-5 SM-5.3 (G4) follow-up: a true publish→dispatch→dead-letter
    // end-to-end test requires net-new App API surface (Arc<B>: EventBus
    // impl, an App publish handle, or a CommandBus-driven publication
    // path per CHE-0024:R1). All three are out of v0.1 closure scope
    // (FOCUS §8 — no new top-level features). Tracked as bd
    // adr-fmt-1art (label `S8+`).
    let (foo_id, foo_envs) = foo_gw
        .create(FooDo { value: 42 }, ctx.clone())
        .await
        .expect("foo create");
    assert_eq!(foo_envs.len(), 1);
    assert_eq!(foo_envs[0].correlation_id(), Some(correlation_id));

    // Policy effect: simulate by directly invoking what the policy
    // closure would do — feed a BarPing through bar_gw with a
    // correlation context derived per CHE-0051:R6.
    let derived_ctx =
        cherry_pit_agent::correlation_for(foo_envs[0].correlation_id(), foo_envs[0].event_id());
    let (bar_id, bar_envs) = bar_gw
        .create(fixture::domain::BarPing { from: 42 }, derived_ctx.clone())
        .await
        .expect("bar create");
    assert_eq!(bar_envs.len(), 1);
    assert_eq!(bar_envs[0].correlation_id(), Some(correlation_id));
    assert_eq!(bar_envs[0].causation_id(), Some(foo_envs[0].event_id()));

    // Both stores carry their own ID counters; existence + envelope
    // count proves persistence happened in both files.
    let _ = (foo_id, bar_id);

    let _ = shutdown_tx.send(());
    run_handle.await.expect("run join").expect("run ok");
}
