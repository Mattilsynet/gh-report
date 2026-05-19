//! `AppState` — shared application state plumbed into the axum router.
//!
//! M1.4 shape: holds the `ApplicationService`, the `AdrCorpus` read-
//! model projection, and the constructed GraphQL `Schema`. `Schema`
//! from async-graphql 7.x is internally `Arc`-shared and `Clone` is
//! cheap; carrying it by value here keeps `axum::extract::State`
//! ergonomics intact.
//!
//! `Clone` is cheap on `AppState` (Arc + Schema-arc clones); axum
//! hands a clone to each request via `axum::extract::State`.

use std::sync::{Arc, Mutex};

use crate::app::service::AdrService;
use crate::graphql::AdrSchema;
use crate::projection::AdrCorpus;

/// Shared state for the adr-srv axum router.
#[derive(Clone)]
pub struct AppState {
    /// `ApplicationService` for the `AdrDocument` aggregate.
    pub adr_service: Arc<AdrService>,
    /// Corpus-wide read-model projection.
    pub corpus: Arc<Mutex<AdrCorpus>>,
    /// Constructed GraphQL schema (read-only: `Query` + `EmptyMutation`).
    pub schema: AdrSchema,
}

impl AppState {
    /// Construct an `AppState` from a fully-wired `AdrService`,
    /// corpus mutex, and pre-built schema.
    #[must_use]
    pub fn new(
        adr_service: Arc<AdrService>,
        corpus: Arc<Mutex<AdrCorpus>>,
        schema: AdrSchema,
    ) -> Self {
        Self {
            adr_service,
            corpus,
            schema,
        }
    }
}
