//! Single `assemble()` entry point that wires the 2-aggregate fixture
//! into a runnable [`App`]. Load-bearing for the ergonomic LOC
//! benchmark — keep this file lean (no helpers, no domain reshaping).

use std::sync::Arc;

use cherry_pit_app::{App, InProcessEventBus, TracingDeadLetterSink};
use cherry_pit_core::CommandGateway;
use cherry_pit_gateway::MsgpackFileStore;
use tempfile::TempDir;

use super::domain::{BarEvent, FooEvent, FooToBarOutput, FooToBarPolicy};
use super::infra::{BarGateway, FooGateway};

pub struct Assembled {
    pub app: App<
        FooGateway,
        MsgpackFileStore<FooEvent>,
        InProcessEventBus<FooEvent>,
        (),
        TracingDeadLetterSink,
    >,
    pub foo_gateway: Arc<FooGateway>,
    pub bar_gateway: Arc<BarGateway>,
    pub _dirs: (TempDir, TempDir, TempDir),
}

pub fn assemble() -> Assembled {
    let foo_dir = tempfile::tempdir().expect("foo dir");
    let bar_dir = tempfile::tempdir().expect("bar dir");
    let app_store_dir = tempfile::tempdir().expect("app store dir");
    let foo_store = Arc::new(MsgpackFileStore::<FooEvent>::new(foo_dir.path()));
    let bar_store = Arc::new(MsgpackFileStore::<BarEvent>::new(bar_dir.path()));
    let foo_gateway = Arc::new(FooGateway::new(Arc::clone(&foo_store)));
    let bar_gateway = Arc::new(BarGateway::new(Arc::clone(&bar_store)));
    let mut app = App::new(
        FooGateway::new(Arc::clone(&foo_store)),
        MsgpackFileStore::<FooEvent>::new(app_store_dir.path()),
        InProcessEventBus::<FooEvent>::new(),
        (),
        TracingDeadLetterSink::new(),
    );
    let bar_for_policy = Arc::clone(&bar_gateway);
    app.register_policy(
        FooToBarPolicy,
        move |out: FooToBarOutput, _gw: &FooGateway, ctx| {
            let bar = Arc::clone(&bar_for_policy);
            async move {
                let FooToBarOutput::Ping(cmd) = out;
                bar.create(cmd, ctx)
                    .await
                    .map(|_| ())
                    .map_err(|e| cherry_pit_app::AgentError::Policy(format!("bar: {e}").into()))
            }
        },
        "FooToBarPolicy",
        "FooToBarOutput",
    );
    Assembled {
        app,
        foo_gateway,
        bar_gateway,
        _dirs: (foo_dir, bar_dir, app_store_dir),
    }
}
