//! Application layer: `ApplicationService` (`AdrService`), the
//! axum-pluggable `AppState`, and the `AdrStorePort` persistence seam
//! (CHE-0098 N-R7) they are generic over.

mod service;
mod state;
pub mod store_port;

pub use service::{AdrService, IngestOutcome};
pub use state::AppState;
pub use store_port::{AdrStorePort, NativeAdrStorePortError};
