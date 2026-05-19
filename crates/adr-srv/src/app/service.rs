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
//! ## M1.2 vs M1.3
//!
//! This mission ships:
//!   - `AdrService::new(store)` constructor (verified by skeleton.rs)
//!   - `ingest()` signature (body is `todo!()` — M1.3 implements)
//!   - index-map fields (constructed empty)
//!
//! M1.3 implements `ingest()`: AdrId-keyed lookup → `store.create()`
//! for new aggregates / `store.append()` for existing, atomic index
//! update.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, StoreError};
use cherry_pit_pardosa::PardosaFileEventStore;

use crate::domain::adr_id::AdrId;
use crate::domain::events::AdrIngested;

/// `ApplicationService` for the `AdrDocument` aggregate.
pub struct AdrService {
    /// Event store. `Arc` so `AppState::clone` is cheap.
    store: Arc<PardosaFileEventStore<AdrIngested>>,
    /// Per-aggregate last-applied-sequence tracker (CHE-0054:R6 /
    /// CHE-0042:R3). M1.3 populates this on `ingest` to support
    /// optimistic-concurrency `append` calls.
    #[expect(
        dead_code,
        reason = "M1.3 (scrape pipeline) populates and reads this tracker; M1.2 pins the field shape only"
    )]
    next_seq: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    /// `AdrId` → `AggregateId` index (CHE-0054:R5). M1.3 populates
    /// this on `ingest` and during replay-on-boot.
    #[expect(
        dead_code,
        reason = "M1.3 (scrape pipeline) populates and reads this index; M1.2 pins the field shape only"
    )]
    adrs_by_id: Arc<Mutex<HashMap<AdrId, AggregateId>>>,
}

impl AdrService {
    /// Construct a new `AdrService` over an open event store. Indices
    /// start empty; M1.3's replay-on-boot path populates them from
    /// the store's history.
    #[must_use]
    pub fn new(store: Arc<PardosaFileEventStore<AdrIngested>>) -> Self {
        Self {
            store,
            next_seq: Arc::new(Mutex::new(HashMap::new())),
            adrs_by_id: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Access the underlying event store. Used by replay-on-boot
    /// paths in M1.3 and by tests.
    #[must_use]
    pub fn store(&self) -> &Arc<PardosaFileEventStore<AdrIngested>> {
        &self.store
    }

    /// Ingest a parsed `AdrIngested` event.
    ///
    /// M1.2 pins the signature; M1.3 implements the body
    /// (`adrs_by_id` lookup → `store.create()` for new aggregates,
    /// `store.append(id, expected_seq, [event], ctx)` for existing,
    /// atomic `next_seq` + `adrs_by_id` update).
    ///
    /// # Errors
    /// Surfaces `StoreError` from the underlying event store; in
    /// particular `StoreError::ConcurrencyConflict` on a stale
    /// `expected_seq` (CHE-0054:R6 optimistic concurrency).
    ///
    /// # Panics
    /// Currently panics with `todo!()` — implementation lands in
    /// M1.3 (`phase2-v2-m1.3-scrape-pipeline`).
    #[expect(
        clippy::unused_async,
        reason = "async signature is the M1.2 pinned shape; M1.3 implements with awaits on `store.create` / `store.append`"
    )]
    pub async fn ingest(&self, _event: AdrIngested) -> Result<(), StoreError> {
        todo!("M1.3 (scrape pipeline) implements; M1.2 pins signature only")
    }
}
