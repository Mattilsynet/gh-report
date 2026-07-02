//! Generic in-memory HTTP read-serve pipeline.
//!
//! Provides the reusable serve surface ruled into `cherry-pit-web` by
//! CHE-0086: security headers, ETag/304 handling, zstd negotiation,
//! WebSocket live updates, and path normalization over caller-owned
//! in-memory content. Consumers implement [`ServerState`] on their
//! concrete application state; all dispatch remains monomorphised.

pub mod config;
pub mod error;
pub mod runtime;
pub mod state;

pub use config::{ConfigError, ServerConfig, ServerConfigBuilder, ValidatedConfig};
pub use error::ServerError;
pub use runtime::{bind_serving_port, build_router, start};
pub use state::{CachedPage, PageUpdateEvent, ServerState};
