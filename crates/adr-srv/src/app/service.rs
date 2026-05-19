//! `ApplicationService` — load → handle → append cycle owner for the
//! `AdrDocument` aggregate.
//!
//! ## CHE-0054:R8/R10 carve-out
//!
//! adr-srv does NOT depend on `cherry-pit-agent` or `cherry-pit-gateway`.
//! The `ApplicationService` consumes `cherry_pit_core::EventStore`
//! directly via `PardosaFileEventStore<AdrIngested>`. Indices
//! (`adrs_by_id`) and per-aggregate sequence tracker (`next_seq`)
//! are owned here rather than in a separate `App<...>` per CHE-0054:R8.
//!
//! Shape adapted from `crates/gh-report/src/app/state.rs` (`RunService`
//! / `RepoService` / `WebhookService` pattern, B7'b wiring). Indices
//! use `std::sync::Mutex<HashMap<...>>` placeholder per gh-report's
//! B7'a shape — typed-key newtypes / `DashMap` upgrade deferred until
//! call sites in M1.3 constrain the choice (gh-report L255-260
//! rationale).
//!
//! ## M1.3 surface
//!
//! - `ingest_if_changed(event)` — body-hash idempotency check per
//!   AFM-0027:R4: if no prior aggregate exists for `event.id`, call
//!   `store.create`; if one exists and `body_hash` matches the latest
//!   projection, return `Unchanged` with zero new events; if `body_hash`
//!   differs, call `store.append` with the tracked `expected_sequence`.
//! - `new_with_replay(store)` — replay-on-boot constructor: enumerates
//!   on-disk aggregates via `PardosaFileEventStore::list_aggregates`,
//!   folds each stream via `AdrDocument::from_first` + `apply`, and
//!   populates `adrs_by_id` + `next_seq` so a re-scrape against the
//!   same store is idempotent.
//! - `lookup(&AdrId) -> Option<AggregateId>` — small accessor used by
//!   the scrape pipeline (and tests) to confirm an ADR file's
//!   aggregate is known after ingest.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, CorrelationContext, EventStore, Projection, StoreError};
use cherry_pit_pardosa::PardosaFileEventStore;

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

/// `ApplicationService` for the `AdrDocument` aggregate.
pub struct AdrService {
    /// Event store. `Arc` so `AppState::clone` is cheap.
    store: Arc<PardosaFileEventStore<AdrIngested>>,
    /// Per-aggregate last-applied-sequence tracker (CHE-0054:R6 /
    /// CHE-0042:R3) — the `expected_sequence` passed to
    /// `store.append`.
    next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    /// `AdrId` → `AggregateId` index (CHE-0054:R5). Populated by
    /// `ingest_if_changed` and `new_with_replay`.
    adrs_by_id: Arc<Mutex<HashMap<AdrId, AggregateId>>>,
    /// Latest projected `body_hash` per aggregate, used for
    /// `ingest_if_changed`'s idempotency check (AFM-0027:R4). Kept
    /// in lock-step with `next_seq` so a single mutex pair covers
    /// the per-aggregate read-modify-write cycle.
    latest_body_hash: Arc<Mutex<HashMap<AggregateId, crate::domain::body_hash::BodyHash>>>,
}

impl AdrService {
    /// Construct a new `AdrService` over an open event store. Indices
    /// start empty.
    ///
    /// Use [`Self::new_with_replay`] when the store directory may
    /// already contain aggregates from a prior process — `new`
    /// assumes a virgin store.
    #[must_use]
    pub fn new(store: Arc<PardosaFileEventStore<AdrIngested>>) -> Self {
        Self {
            store,
            next_seq: Arc::new(Mutex::new(HashMap::new())),
            adrs_by_id: Arc::new(Mutex::new(HashMap::new())),
            latest_body_hash: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Construct a new `AdrService` and replay every aggregate already
    /// in the store, populating `adrs_by_id`, `next_seq`,
    /// `latest_body_hash`, AND the supplied `AdrCorpus` projection.
    ///
    /// The corpus is owned by `AppState` and threaded in here so that
    /// the boot-time replay seeds the read model AND the per-aggregate
    /// indices in one pass (CHE-0048:R5 in-memory replay election).
    ///
    /// # Errors
    /// Surfaces `StoreError` from `store.load(id)` for any aggregate
    /// listed by `PardosaFileEventStore::list_aggregates` that fails
    /// to load. A `list_aggregates` IO failure is wrapped in
    /// `StoreError::Infrastructure`.
    ///
    /// # Panics
    /// Panics on a poisoned `adrs_by_id` / `next_seq` /
    /// `latest_body_hash` / `corpus` mutex. Mutexes are private to
    /// this service (or single-owner in `AppState`) and only held for
    /// short index updates; poisoning indicates a prior panic and is
    /// treated as non-recoverable.
    pub async fn new_with_replay(
        store: Arc<PardosaFileEventStore<AdrIngested>>,
        corpus: &Arc<Mutex<AdrCorpus>>,
    ) -> Result<Self, StoreError> {
        let service = Self::new(Arc::clone(&store));

        let ids = store.list_aggregates().map_err(|e| {
            StoreError::Infrastructure(format!("list_aggregates failed during replay: {e}").into())
        })?;

        for id in ids {
            let envelopes = store.load(id).await?;
            // `load` returns `Ok(empty)` only for unknown aggregates,
            // but list_aggregates surfaced this id from disk — empty
            // here would be a corrupt-stream condition. Skip rather
            // than synthesise state.
            let Some(first) = envelopes.first() else {
                continue;
            };
            let mut doc = AdrDocument::from_first(first.payload());
            for env in envelopes.iter().skip(1) {
                doc = doc.apply(env.payload());
            }
            let last_seq = envelopes
                .last()
                .expect("envelopes non-empty here")
                .sequence();

            service
                .adrs_by_id
                .lock()
                .expect("adrs_by_id mutex not poisoned")
                .insert(doc.id.clone(), id);
            service
                .next_seq
                .lock()
                .expect("next_seq mutex not poisoned")
                .insert(id, last_seq);
            service
                .latest_body_hash
                .lock()
                .expect("latest_body_hash mutex not poisoned")
                .insert(id, doc.body_hash);

            // Project the full stream into the AdrCorpus. Held lock
            // is per-aggregate-stream, not for the whole replay loop,
            // to minimise contention if a future caller queries
            // mid-replay (none today; defensive).
            {
                let mut guard = corpus.lock().expect("corpus mutex not poisoned");
                for env in &envelopes {
                    guard.apply(env);
                }
            }
        }

        Ok(service)
    }

    /// Access the underlying event store. Used by replay-on-boot
    /// paths and by tests.
    #[must_use]
    pub fn store(&self) -> &Arc<PardosaFileEventStore<AdrIngested>> {
        &self.store
    }

    /// Look up the `AggregateId` for an `AdrId`. Returns `None` when
    /// the ADR has never been ingested by this process (or by a prior
    /// process whose store was not replayed via
    /// [`Self::new_with_replay`]).
    ///
    /// # Panics
    /// Panics on a poisoned `adrs_by_id` mutex. The mutex is private
    /// to this service and only held for short index reads; poisoning
    /// indicates a prior panic inside the service.
    #[must_use]
    pub fn lookup(&self, id: &AdrId) -> Option<AggregateId> {
        self.adrs_by_id
            .lock()
            .expect("adrs_by_id mutex not poisoned")
            .get(id)
            .copied()
    }

    /// Ingest a parsed `AdrIngested` event with body-hash idempotency
    /// (AFM-0027:R4), updating the supplied `AdrCorpus` projection
    /// in lock-step on every appended/created envelope.
    ///
    /// - No prior aggregate for `event.id` → `store.create`,
    ///   [`IngestOutcome::Created`].
    /// - Prior aggregate, `event.body_hash` matches latest projection
    ///   → no-op, [`IngestOutcome::Unchanged`] (corpus untouched).
    /// - Prior aggregate, `event.body_hash` differs → `store.append`
    ///   with tracked `expected_sequence`, [`IngestOutcome::Appended`].
    ///
    /// # Errors
    /// Surfaces `StoreError` from the underlying event store; in
    /// particular `StoreError::ConcurrencyConflict` on a stale
    /// `expected_sequence` (CHE-0054:R6 optimistic concurrency).
    ///
    /// # Panics
    /// Panics on a poisoned `adrs_by_id` / `next_seq` /
    /// `latest_body_hash` / `corpus` mutex (see [`Self::new_with_replay`]
    /// § Panics). Also panics on the documented invariant that
    /// `store.create` and `store.append` return a non-empty envelope
    /// vector; an empty return would indicate a broken substrate.
    pub async fn ingest_if_changed(
        &self,
        event: AdrIngested,
        corpus: &Arc<Mutex<AdrCorpus>>,
    ) -> Result<IngestOutcome, StoreError> {
        let adr_id = event.id.clone();
        let existing = self.lookup(&adr_id);

        match existing {
            None => {
                let (agg_id, envelopes) = self
                    .store
                    .create(vec![event], CorrelationContext::none())
                    .await?;
                let last_seq = envelopes
                    .last()
                    .expect("create returned non-empty envelopes")
                    .sequence();
                let payload = envelopes
                    .last()
                    .expect("create returned non-empty envelopes")
                    .payload();
                let body_hash = payload.body_hash;

                self.adrs_by_id
                    .lock()
                    .expect("adrs_by_id mutex not poisoned")
                    .insert(adr_id, agg_id);
                self.next_seq
                    .lock()
                    .expect("next_seq mutex not poisoned")
                    .insert(agg_id, last_seq);
                self.latest_body_hash
                    .lock()
                    .expect("latest_body_hash mutex not poisoned")
                    .insert(agg_id, body_hash);

                // Project AFTER per-service indices to minimise lock
                // interleaving (corpus lock acquired only once the
                // per-aggregate locks are released).
                {
                    let mut guard = corpus.lock().expect("corpus mutex not poisoned");
                    for env in &envelopes {
                        guard.apply(env);
                    }
                }
                Ok(IngestOutcome::Created)
            }
            Some(agg_id) => {
                let latest = self
                    .latest_body_hash
                    .lock()
                    .expect("latest_body_hash mutex not poisoned")
                    .get(&agg_id)
                    .copied();
                if latest == Some(event.body_hash) {
                    return Ok(IngestOutcome::Unchanged);
                }
                let expected_seq = self
                    .next_seq
                    .lock()
                    .expect("next_seq mutex not poisoned")
                    .get(&agg_id)
                    .copied()
                    .ok_or_else(|| {
                        StoreError::Infrastructure(
                            format!(
                                "adrs_by_id holds agg {agg_id:?} for {adr_id} but next_seq does not"
                            )
                            .into(),
                        )
                    })?;
                let new_body_hash = event.body_hash;
                let envelopes = self
                    .store
                    .append(
                        agg_id,
                        expected_seq,
                        vec![event],
                        CorrelationContext::none(),
                    )
                    .await?;
                let last_seq = envelopes
                    .last()
                    .expect("append returned non-empty envelopes")
                    .sequence();
                self.next_seq
                    .lock()
                    .expect("next_seq mutex not poisoned")
                    .insert(agg_id, last_seq);
                self.latest_body_hash
                    .lock()
                    .expect("latest_body_hash mutex not poisoned")
                    .insert(agg_id, new_body_hash);
                {
                    let mut guard = corpus.lock().expect("corpus mutex not poisoned");
                    for env in &envelopes {
                        guard.apply(env);
                    }
                }
                Ok(IngestOutcome::Appended)
            }
        }
    }
}
