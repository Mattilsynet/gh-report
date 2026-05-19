//! `AppState` — shared application state plumbed into the axum router.
//!
//! Minimal v0.1 shape: holds the `ApplicationService` and a placeholder
//! for the GraphQL schema (constructed in M1.4). Projection state
//! cache lands in M1.3.
//!
//! `Clone` is cheap (only `Arc` clones); axum hands a clone to each
//! request via `axum::extract::State`.

use std::sync::Arc;

use crate::app::service::AdrService;

/// Shared state for the adr-srv axum router.
#[derive(Clone)]
pub struct AppState {
    /// `ApplicationService` for the `AdrDocument` aggregate.
    pub adr_service: Arc<AdrService>,
}

impl AppState {
    /// Construct an `AppState` from a fully-wired `AdrService`.
    #[must_use]
    pub fn new(adr_service: Arc<AdrService>) -> Self {
        Self { adr_service }
    }
}
