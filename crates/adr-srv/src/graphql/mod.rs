//! GraphQL Query schema for the adr-srv read model.
//!
//! v0.1 is read-only: `Query` only — `EmptyMutation` is the load-bearing
//! enforcer of the Track 3 v0.1 "no mutations" scope (M1 sub-package
//! A5 abort). Phase 3 lands `Mutation` for `ratifyAdr` / `supersede`
//! per the retired Track 3.4 / 3.5 roadmap.
//!
//! ## Vernon Rule 4 — flat DTO, not the aggregate
//!
//! `AdrGql` and `AdrRef` (see [`query`]) are denormalised projection
//! views over `AdrDocument`. The aggregate snapshot is NOT exposed
//! directly; the resolver materialises a fresh DTO per query. Phase 3
//! mutations will route through the aggregate; reads do not.
//!
//! ## CHE-0048 substrate
//!
//! The `AdrCorpus` projection (CHE-0048:R6 single-aggregate-type,
//! R3 idempotent, R5 in-memory replay) backs every resolver. Schema
//! state is `Arc<Mutex<AdrCorpus>>` threaded through `Schema.data(...)`.

pub mod query;
pub mod types;

use std::sync::{Arc, Mutex};

use async_graphql::{EmptyMutation, EmptySubscription, Schema};

pub use query::Query;
pub use types::{AdrGql, AdrRef};

use crate::projection::AdrCorpus;

/// Concrete schema type alias. `EmptyMutation` is the wire-level
/// enforcement of the read-only contract (M1 sub-package A5).
pub type AdrSchema = Schema<Query, EmptyMutation, EmptySubscription>;

/// Build the GraphQL schema, threading the `AdrCorpus` projection
/// through resolver context via `Schema.data(...)`.
#[must_use]
pub fn build_schema(corpus: Arc<Mutex<AdrCorpus>>) -> AdrSchema {
    Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(corpus)
        .finish()
}
