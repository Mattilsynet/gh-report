//! GraphQL `Query` root resolvers.
//!
//! All resolvers read from the `AdrCorpus` projection injected via
//! `Schema.data(...)`. Lock acquisition is per-resolver-call to keep
//! the critical section short — the corpus state is `Clone`-cheap
//! per-doc but the map iteration in `adrs` holds the lock across the
//! materialisation. Acceptable for v0.1 (boot-time scrape only; no
//! concurrent ingress — that's bead `adr-fmt-azu5e`, Phase 3).

use std::sync::{Arc, Mutex};

use async_graphql::{Context, Object};

use crate::domain::adr_id::AdrId;
use crate::domain::aggregate::AdrDocument;
use crate::graphql::types::{AdrGql, AdrRef};
use crate::projection::AdrCorpus;

/// GraphQL `Query` root.
pub struct Query;

#[Object]
impl Query {
    /// Fetch a single ADR by canonical id (e.g. `"AFM-0001"`).
    /// Returns `null` for unknown ids or unparseable inputs.
    async fn adr_by_id(&self, ctx: &Context<'_>, id: String) -> Option<AdrGql> {
        let corpus = ctx.data_unchecked::<Arc<Mutex<AdrCorpus>>>();
        let parsed: AdrId = id.parse().ok()?;
        let guard = corpus.lock().ok()?;
        guard.get(&parsed).map(to_gql)
    }

    /// List every ADR whose id has the given domain prefix
    /// (e.g. `"AFM"`). Order: `BTreeMap` key order (domain then number).
    async fn adrs_by_domain(&self, ctx: &Context<'_>, domain: String) -> Vec<AdrGql> {
        let corpus = ctx.data_unchecked::<Arc<Mutex<AdrCorpus>>>();
        let Ok(guard) = corpus.lock() else {
            return Vec::new();
        };
        guard
            .iter()
            .filter(|(id, _)| id.domain() == domain)
            .map(|(_, doc)| to_gql(doc))
            .collect()
    }

    /// List every known ADR in `BTreeMap` key order.
    async fn all_adrs(&self, ctx: &Context<'_>) -> Vec<AdrGql> {
        let corpus = ctx.data_unchecked::<Arc<Mutex<AdrCorpus>>>();
        let Ok(guard) = corpus.lock() else {
            return Vec::new();
        };
        guard.iter().map(|(_, doc)| to_gql(doc)).collect()
    }
}

/// Materialise the flat DTO from an `AdrDocument` snapshot. Allocates
/// fresh strings per field; cheap and the lock is held across this.
fn to_gql(doc: &AdrDocument) -> AdrGql {
    AdrGql {
        id: doc.id.to_string(),
        title: doc.frontmatter.title.clone(),
        date: doc.frontmatter.date.to_string(),
        last_reviewed: doc.frontmatter.last_reviewed.to_string(),
        tier: doc.frontmatter.tier.as_str().to_string(),
        status: doc.frontmatter.status.as_str().to_string(),
        body_hash: doc.body_hash.to_string(),
        references: doc
            .references
            .iter()
            .map(|r| AdrRef { id: r.to_string() })
            .collect(),
    }
}
