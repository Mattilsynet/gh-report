//! `AdrCorpus` — corpus-wide read model: latest `AdrDocument` per
//! `AdrId`, projected from the `AdrIngested` event stream.
//!
//! ## CHE-0048 substrate
//!
//! - **R3 idempotency**: `apply` is deterministic over the same
//!   `EventEnvelope<AdrIngested>` sequence — total replacement of the
//!   per-`AdrId` entry via `AdrDocument::apply` (M1 single-event-type
//!   path) is trivially idempotent.
//! - **R5 in-memory replay**: no `FileProjectionStore` snapshot writes
//!   in v0.1. The corpus is rebuilt on every process start by
//!   replaying the event log via `AdrService::new_with_replay`
//!   (gh-report-style election per CHE-0065).
//! - **R6 single-aggregate-type**: `type Event = AdrIngested`.
//!   The internal `BTreeMap<AdrId, AdrDocument>` does NOT widen the
//!   trait — every consumed event belongs to the same logical
//!   aggregate type; per-aggregate state is partitioned by `AdrId`
//!   inside one Projection impl.
//!
//! ## Why `BTreeMap`, not `HashMap`
//!
//! Deterministic iteration order. `adrs` queries iterate the corpus
//! and must return ADRs in a stable order across replays so test
//! assertions hold and any future cursor-based pagination is sound.

use std::collections::BTreeMap;

use cherry_pit_core::{EventEnvelope, Projection};

use crate::domain::adr_id::AdrId;
use crate::domain::aggregate::AdrDocument;
use crate::domain::events::AdrIngested;

/// Corpus-wide latest-known projection of every ADR observed via
/// `AdrIngested` events.
#[derive(Default, Debug, Clone)]
pub struct AdrCorpus {
    docs: BTreeMap<AdrId, AdrDocument>,
}

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
