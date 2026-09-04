#[path = "../../src/test_support.rs"]
#[allow(
    dead_code,
    reason = "each test binary including this module uses a different subset of the harness surface, so #[expect] would be unfulfilled in some of them"
)]
mod canonical;
#[allow(
    unused_imports,
    reason = "each test binary including this module uses a different subset of the harness surface, so #[expect] would be unfulfilled in some of them"
)]
pub use canonical::{LiveNats, LiveNatsServer};
