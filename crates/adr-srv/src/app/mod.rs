//! Application layer: `ApplicationService` (`AdrService`) and the
//! axum-pluggable `AppState`. CHE-0054:R8/R10 carve-out — no
//! cherry-pit-app, no cherry-pit-gateway, no `App<...>`.

mod service;
mod state;

pub use service::{AdrService, IngestOutcome};
pub use state::AppState;
