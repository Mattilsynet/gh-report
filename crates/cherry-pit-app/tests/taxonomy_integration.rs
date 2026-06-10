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

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let run_handle = tokio::spawn(async move {
        app.run(async move {
            let _ = shutdown_rx.await;
        })
        .await
    });

    let (foo_id, foo_envs) = foo_gw
        .create(FooDo { value: 42 }, ctx.clone())
        .await
        .expect("foo create");
    assert_eq!(foo_envs.len(), 1);
    assert_eq!(foo_envs[0].correlation_id(), Some(correlation_id));

    let derived_ctx =
        cherry_pit_app::correlation_for(foo_envs[0].correlation_id(), foo_envs[0].event_id());
    let (bar_id, bar_envs) = bar_gw
        .create(fixture::domain::BarPing { from: 42 }, derived_ctx.clone())
        .await
        .expect("bar create");
    assert_eq!(bar_envs.len(), 1);
    assert_eq!(bar_envs[0].correlation_id(), Some(correlation_id));
    assert_eq!(bar_envs[0].causation_id(), Some(foo_envs[0].event_id()));

    let _ = (foo_id, bar_id);

    let _ = shutdown_tx.send(());
    run_handle.await.expect("run join").expect("run ok");
}
