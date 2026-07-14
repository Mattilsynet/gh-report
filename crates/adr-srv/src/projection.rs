//! `AdrCorpus` — corpus-wide read model: latest `AdrDocument` per
//! `AdrId`, projected from the `AdrIngested` event stream.
//!
//! Single-aggregate-type projection over one event type; idempotent
//! replace-per-`AdrId` via `AdrDocument::apply`; rebuilt in-memory on
//! every process start by replaying the log (no snapshot store in
//! v0.1). See CHE-0048 (R3/R5/R6) for the binding substrate contract.
//!
//! `BTreeMap`, not `HashMap`: deterministic iteration order so `adrs`
//! queries return a stable order across replays.

use std::collections::BTreeMap;

use cherry_pit_core::{EventEnvelope, Projection, ReadPort};

use crate::domain::adr_id::AdrId;
use crate::domain::aggregate::AdrDocument;
use crate::domain::events::AdrIngested;

/// Corpus-wide latest-known projection of every ADR observed via
/// `AdrIngested` events.
#[derive(Default, Debug, Clone)]
pub struct AdrCorpus {
    docs: BTreeMap<AdrId, AdrDocument>,
}

/// Typed read query for the ADR corpus projection.
#[derive(Debug, Clone)]
pub enum AdrCorpusQuery {
    /// Return the ADR with the supplied id.
    ById(AdrId),
    /// Return ADRs whose id has the supplied domain prefix.
    ByDomain(String),
    /// Return all ADRs in corpus order.
    All,
}

/// Typed read response for the ADR corpus projection.
#[derive(Debug, Clone)]
pub enum AdrCorpusResponse {
    /// Optional single ADR result.
    One(Option<AdrDocument>),
    /// Ordered ADR list result.
    Many(Vec<AdrDocument>),
}

/// Static read port for [`AdrCorpus`].
pub struct AdrCorpusReadPort;

impl Projection for AdrCorpus {
    type Event = AdrIngested;

    fn apply(&mut self, envelope: &EventEnvelope<Self::Event>) {
        let event = envelope.payload();
        let id = event.id.clone();
        let updated = match self.docs.get(&id).cloned() {
            Some(existing) => existing.apply(event),
            None => AdrDocument::from_first(event),
        };
        self.docs.insert(id, updated);
    }
}

impl AdrCorpus {
    /// Look up a single ADR by id.
    #[must_use]
    pub fn get(&self, id: &AdrId) -> Option<&AdrDocument> {
        self.docs.get(id)
    }

    /// Iterate `(id, doc)` pairs in `BTreeMap` key order (domain then
    /// number).
    pub fn iter(&self) -> impl Iterator<Item = (&AdrId, &AdrDocument)> {
        self.docs.iter()
    }

    /// Number of ADRs in the corpus.
    #[must_use]
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// `true` if no ADRs have been projected yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

impl ReadPort for AdrCorpusReadPort {
    type Projection = AdrCorpus;
    type Query = AdrCorpusQuery;
    type Response = AdrCorpusResponse;

    fn resolve(projection: &Self::Projection, query: Self::Query) -> Self::Response {
        match query {
            AdrCorpusQuery::ById(id) => AdrCorpusResponse::One(projection.get(&id).cloned()),
            AdrCorpusQuery::ByDomain(domain) => AdrCorpusResponse::Many(
                projection
                    .iter()
                    .filter(|(id, _)| id.domain() == domain)
                    .map(|(_, doc)| doc.clone())
                    .collect(),
            ),
            AdrCorpusQuery::All => {
                AdrCorpusResponse::Many(projection.iter().map(|(_, doc)| doc.clone()).collect())
            }
        }
    }
}
