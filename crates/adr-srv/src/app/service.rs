//! `ApplicationService` — load → handle → append cycle owner for the
//! `AdrDocument` aggregate.
//!
//! Generic over [`AdrStorePort`] (CHE-0098 N-R7 port seam): production
//! wiring (`main.rs`) instantiates `AdrService<NativeAdrStore>`; the
//! cherry-pit test-suite reference store
//! (`cherry_pit_gateway::MsgpackFileStore`, CHE-0098 R10) remains a
//! valid instantiation via its own [`AdrStorePort`] impl. Indices
//! (`adrs_by_id`, `next_seq`, `latest_body_hash`) are keyed uniformly
//! by [`AdrId`] regardless of which port is in play; only the opaque
//! [`AdrStorePort::Id`] needed to re-target a subsequent `append`
//! varies per implementation.
//!
//! ## Surface
//!
//! - `ingest_if_changed(event)` — body-hash idempotency check per
//!   AFM-0027:R4: creates on no prior aggregate, returns `Unchanged`
//!   when `body_hash` matches the latest projection, else appends.
//! - `new_with_replay(store)` — replay-on-boot constructor: folds
//!   every persisted stream (grouped by `AdrId`) to populate
//!   `adrs_by_id` + `next_seq` + the projection.
//! - `lookup(&AdrId) -> Option<S::Id>` — accessor confirming an ADR
//!   file's stream is known after ingest.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use crate::app::store_port::AdrStorePort;
use crate::domain::adr_id::AdrId;
use crate::domain::aggregate::AdrDocument;
use crate::domain::events::AdrIngested;
use crate::projection::AdrCorpus;

/// Outcome of [`AdrService::ingest_if_changed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestOutcome {
    /// No prior aggregate existed for this `AdrId`; one was created.
    Created,
    /// Prior aggregate existed; the new `body_hash` differed; a fresh
    /// event was appended.
    Appended,
    /// Prior aggregate existed and the new `body_hash` matched the
    /// latest projection; no event was emitted.
    Unchanged,
}

/// `ApplicationService` for the `AdrDocument` aggregate, generic over
/// its persistence port (CHE-0098 N-R7).
pub struct AdrService<S: AdrStorePort> {
    /// Event store port. `Arc` so `AppState::clone` is cheap.
    store: Arc<S>,
    /// Per-`AdrId` last-applied-sequence tracker, passed back as the
    /// `expected_sequence` argument to [`AdrStorePort::append`].
    next_seq: Arc<Mutex<HashMap<AdrId, NonZeroU64>>>,
    /// `AdrId` → opaque store handle index. Populated by
    /// `ingest_if_changed` and `new_with_replay`.
    adrs_by_id: Arc<Mutex<HashMap<AdrId, S::Id>>>,
    /// Latest projected `body_hash` per `AdrId`, used for
    /// `ingest_if_changed`'s idempotency check (AFM-0027:R4). Kept
    /// in lock-step with `next_seq` so a single mutex pair covers
    /// the per-`AdrId` read-modify-write cycle.
    latest_body_hash: Arc<Mutex<HashMap<AdrId, crate::domain::body_hash::BodyHash>>>,
}

impl<S: AdrStorePort> AdrService<S> {
    /// Construct a new `AdrService` over an open event store. Indices
    /// start empty.
    ///
    /// Use [`Self::new_with_replay`] when the store may already
    /// contain streams from a prior process — `new` assumes a virgin
    /// store.
    #[must_use]
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            next_seq: Arc::new(Mutex::new(HashMap::new())),
            adrs_by_id: Arc::new(Mutex::new(HashMap::new())),
            latest_body_hash: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Construct a new `AdrService` and replay every stream already in
    /// the store, populating `adrs_by_id`, `next_seq`,
    /// `latest_body_hash`, AND the supplied `AdrCorpus` projection.
    ///
    /// The corpus is owned by `AppState` and threaded in here so that
    /// the boot-time replay seeds the read model AND the per-`AdrId`
    /// indices in one pass (CHE-0048:R5 in-memory replay election).
    ///
    /// # Errors
    /// Surfaces `S::Error` from [`AdrStorePort::replay_all`].
    ///
    /// # Panics
    /// Panics on a poisoned `adrs_by_id` / `next_seq` /
    /// `latest_body_hash` / `corpus` mutex, or if a replayed group is
    /// empty (an `AdrStorePort` MUST NOT emit an empty group). Mutexes
    /// are private to this service (or single-owner in `AppState`) and
    /// only held for short index updates; poisoning indicates a prior
    /// panic and is treated as non-recoverable.
    pub async fn new_with_replay(
        store: Arc<S>,
        corpus: &Arc<Mutex<AdrCorpus>>,
    ) -> Result<Self, S::Error> {
        let service = Self::new(Arc::clone(&store));

        for (adr_id, handle, events) in store.replay_all().await? {
            let mut doc: Option<AdrDocument> = None;
            for event in &events {
                doc = Some(match doc {
                    Some(existing) => existing.apply(event),
                    None => AdrDocument::from_first(event),
                });
            }
            let doc = doc.expect("AdrStorePort::replay_all MUST NOT emit an empty group");

            service
                .adrs_by_id
                .lock()
                .expect("adrs_by_id mutex not poisoned")
                .insert(adr_id.clone(), handle);
            service
                .next_seq
                .lock()
                .expect("next_seq mutex not poisoned")
                .insert(
                    adr_id.clone(),
                    NonZeroU64::new(u64::try_from(events.len()).unwrap_or(u64::MAX))
                        .unwrap_or(NonZeroU64::MIN),
                );
            service
                .latest_body_hash
                .lock()
                .expect("latest_body_hash mutex not poisoned")
                .insert(adr_id, doc.body_hash);

            {
                let mut guard = corpus.lock().expect("corpus mutex not poisoned");
                for event in &events {
                    guard.apply_event(event);
                }
            }
        }

        Ok(service)
    }

    /// Access the underlying store port. Used by replay-on-boot paths
    /// and by tests.
    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Look up the opaque store handle for an `AdrId`. Returns `None`
    /// when the ADR has never been ingested by this process (or by a
    /// prior process whose store was not replayed via
    /// [`Self::new_with_replay`]).
    ///
    /// # Panics
    /// Panics on a poisoned `adrs_by_id` mutex. The mutex is private
    /// to this service and only held for short index reads; poisoning
    /// indicates a prior panic inside the service.
    #[must_use]
    pub fn lookup(&self, id: &AdrId) -> Option<S::Id> {
        self.adrs_by_id
            .lock()
            .expect("adrs_by_id mutex not poisoned")
            .get(id)
            .copied()
    }

    /// Ingest a parsed `AdrIngested` event with body-hash idempotency
    /// (AFM-0027:R4), updating the supplied `AdrCorpus` projection in
    /// lock-step on every created/appended event.
    ///
    /// - No prior stream for `event.id` → `store.create`,
    ///   [`IngestOutcome::Created`].
    /// - Prior stream, `event.body_hash` matches latest projection →
    ///   no-op, [`IngestOutcome::Unchanged`] (corpus untouched).
    /// - Prior stream, `event.body_hash` differs → `store.append` with
    ///   the tracked `expected_sequence`, [`IngestOutcome::Appended`].
    ///
    /// # Errors
    /// Surfaces `S::Error` from the underlying store port.
    ///
    /// # Panics
    /// Panics on a poisoned `adrs_by_id` / `next_seq` /
    /// `latest_body_hash` / `corpus` mutex (see [`Self::new_with_replay`]
    /// § Panics).
    pub async fn ingest_if_changed(
        &self,
        event: AdrIngested,
        corpus: &Arc<Mutex<AdrCorpus>>,
    ) -> Result<IngestOutcome, S::Error> {
        let adr_id = event.id.clone();
        let existing = self.lookup(&adr_id);

        match existing {
            None => {
                let (handle, seq) = self.store.create(event.clone()).await?;
                let body_hash = event.body_hash;

                self.adrs_by_id
                    .lock()
                    .expect("adrs_by_id mutex not poisoned")
                    .insert(adr_id.clone(), handle);
                self.next_seq
                    .lock()
                    .expect("next_seq mutex not poisoned")
                    .insert(adr_id.clone(), seq);
                self.latest_body_hash
                    .lock()
                    .expect("latest_body_hash mutex not poisoned")
                    .insert(adr_id, body_hash);

                {
                    let mut guard = corpus.lock().expect("corpus mutex not poisoned");
                    guard.apply_event(&event);
                }
                Ok(IngestOutcome::Created)
            }
            Some(handle) => {
                let latest = self
                    .latest_body_hash
                    .lock()
                    .expect("latest_body_hash mutex not poisoned")
                    .get(&adr_id)
                    .copied();
                if latest == Some(event.body_hash) {
                    return Ok(IngestOutcome::Unchanged);
                }
                let expected_seq = self
                    .next_seq
                    .lock()
                    .expect("next_seq mutex not poisoned")
                    .get(&adr_id)
                    .copied()
                    .unwrap_or(NonZeroU64::MIN);
                let new_body_hash = event.body_hash;
                let seq = self
                    .store
                    .append(handle, expected_seq, event.clone())
                    .await?;
                self.next_seq
                    .lock()
                    .expect("next_seq mutex not poisoned")
                    .insert(adr_id.clone(), seq);
                self.latest_body_hash
                    .lock()
                    .expect("latest_body_hash mutex not poisoned")
                    .insert(adr_id, new_body_hash);
                {
                    let mut guard = corpus.lock().expect("corpus mutex not poisoned");
                    guard.apply_event(&event);
                }
                Ok(IngestOutcome::Appended)
            }
        }
    }
}
