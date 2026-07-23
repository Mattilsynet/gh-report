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
use crate::app::store_port::AdrStorePort;
use crate::graphql::AdrSchema;
use crate::projection::AdrCorpus;

/// Shared state for the adr-srv axum router, generic over the
/// persistence port `adr_service` is built on (CHE-0098 N-R7).
///
/// `Clone` is implemented manually (not derived): every field is
/// `Arc`-backed, so cloning `AppState` never requires `S: Clone` —
/// derive would add that bound spuriously (neither store port
/// implements `Clone`).
pub struct AppState<S: AdrStorePort> {
    /// `ApplicationService` for the `AdrDocument` aggregate.
    pub adr_service: Arc<AdrService<S>>,
    /// Corpus-wide read-model projection.
    pub corpus: Arc<Mutex<AdrCorpus>>,
    /// Constructed GraphQL schema (read-only: `Query` + `EmptyMutation`).
    pub schema: AdrSchema,
}

impl<S: AdrStorePort> Clone for AppState<S> {
    fn clone(&self) -> Self {
        Self {
            adr_service: Arc::clone(&self.adr_service),
            corpus: Arc::clone(&self.corpus),
            schema: self.schema.clone(),
        }
    }
}

impl<S: AdrStorePort> AppState<S> {
    /// Construct an `AppState` from a fully-wired `AdrService`,
    /// corpus mutex, and pre-built schema.
    #[must_use]
    pub fn new(
        adr_service: Arc<AdrService<S>>,
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
