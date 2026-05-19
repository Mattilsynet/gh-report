//! # cherry-pit-pardosa
//!
//! Pardosa-backed [`EventStore`] adapter for cherry-pit, with opt-in
//! capability traits per CHE-0057. The adapter wraps a
//! [`pardosa::Dragline`] holding cherry-pit [`EventEnvelope`]s as its
//! domain-event payload, so envelope metadata (event-id `UUIDv7`,
//! correlation, causation, timestamp, sequence) is preserved across
//! the substrate layer without re-fabrication on load (CHE-0042:R1,
//! CHE-0033:R1).
//!
//! ## Public surface (CHE-0030 flat re-export)
//!
//! - [`PardosaEventStore`] — concrete adapter.
//!
//! Trait impls — [`EventStore`], [`PurgeableEventStore`],
//! [`HashChainedEventStore`], [`SingleWriterEventStore`] — are
//! implemented on `PardosaEventStore` and surface via the bounds in
//! `cherry-pit-core`.
//!
//! ## Governing ADRs
//!
//! - CHE-0057 (extension-trait composition policy)
//! - CHE-0059 ([`PurgeableEventStore`] — real impl via pardosa
//!   `migrate_fiber(_, Purge)` + `create_reuse`)
//! - CHE-0060 ([`HashChainedEventStore`] — rollout stub per
//!   CHE-0060:R3; removed when PAR-0021 lands)
//! - CHE-0061 ([`SingleWriterEventStore`] — marker, substrate-level
//!   single-writer guarantee per PAR-0004)
//! - CHE-0042 (envelope construction at store layer)
//! - PAR-0001 (fiber state machine — `Purged → Defined` severs
//!   logical continuity by setting `precursor = Index::NONE`)
//! - PAR-0021 (per-stream BLAKE3 hash chain — substrate-pending)
//!
//! ## Concurrency model
//!
//! A single `std::sync::Mutex<State>` serialises all access to the
//! inner [`pardosa::Dragline`]. The lock is released before each
//! async return; no lock is held across an `.await` (PAR-0008:R3).
//! No `parking_lot`, no `dashmap` — std primitives only.

#![forbid(unsafe_code)]

mod file_store;

pub use file_store::PardosaFileEventStore;

use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroU64;
use std::sync::Mutex;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, HashChainedEventStore,
    PurgeableEventStore, SingleWriterEventStore, StoreCreateResult, StoreError,
};
use pardosa::{DomainId, Dragline, MigrationPolicy};

/// Pardosa-backed [`EventStore`] adapter.
///
/// Wraps a [`Dragline`] whose domain-event payload is the cherry-pit
/// [`EventEnvelope`] itself. This keeps envelope identity stable
/// across `append → load` cycles (CHE-0042:R1) — the substrate stores
/// what the store produced, no transcoding round-trip.
///
/// ## ID mapping
///
/// Cherry-pit [`AggregateId`] (`NonZeroU64`, starts at 1) maps to
/// pardosa [`DomainId`] (`u64`, starts at 0) by `aggregate_id =
/// domain_id + 1`. The mapping is preserved across recreate so the
/// caller-observable `AggregateId` is stable.
///
/// ## What this impl does NOT cover (SM-5 minimum-viable)
///
/// - Persistence: in-memory only; backed by `Dragline::new()` on
///   construction. The NATS/file substrate work lands in SM-7.
/// - Concurrent writers across processes: single-process only at
///   this milestone. The [`SingleWriterEventStore`] marker reflects
///   substrate-level intent (PAR-0004), not yet enforced fencing.
pub struct PardosaEventStore<E: DomainEvent> {
    state: Mutex<State<E>>,
}

struct State<E: DomainEvent> {
    /// Inner pardosa substrate. The `EventEnvelope<E>` payload makes
    /// pardosa events round-trip-stable across the cherry-pit boundary.
    dragline: Dragline<EventEnvelope<E>>,
    /// `AggregateId` → `DomainId` mapping. Populated on `create` and
    /// `recreate`; consulted by every other operation.
    by_aggregate: BTreeMap<AggregateId, DomainId>,
    /// Last-sequence cache per aggregate, mirroring pardosa fiber
    /// length. Used for optimistic-concurrency comparison on `append`
    /// without re-walking the fiber's history.
    last_sequence: BTreeMap<AggregateId, NonZeroU64>,
}

impl<E: DomainEvent> Default for PardosaEventStore<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: DomainEvent> PardosaEventStore<E> {
    /// Create an empty store. The first `create` call allocates
    /// `AggregateId(1)`, mirroring `InMemoryEventStore` and
    /// CHE-0020:R2 (store assigns from 1).
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                dragline: Dragline::new(),
                by_aggregate: BTreeMap::new(),
                last_sequence: BTreeMap::new(),
            }),
        }
    }
}

// ─── Helpers ──────────────────────────────────────────────────────

/// Convert pardosa `DomainId` (`u64`, 0-origin) to cherry-pit
/// `AggregateId` (`NonZeroU64`, 1-origin). The `+ 1` shift is the
/// store's bridge contract; reversed in [`aggregate_to_domain`].
fn domain_to_aggregate(domain_id: DomainId) -> Result<AggregateId, StoreError> {
    let raw = domain_id.value().checked_add(1).ok_or_else(|| {
        StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
            "aggregate id overflow translating from DomainId",
        ))
    })?;
    let nz = NonZeroU64::new(raw).ok_or_else(|| {
        StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
            "aggregate id translated to zero — unreachable post-add",
        ))
    })?;
    Ok(AggregateId::new(nz))
}

/// Convert cherry-pit `AggregateId` to pardosa `DomainId`. Inverse
/// of [`domain_to_aggregate`].
fn aggregate_to_domain(aggregate_id: AggregateId) -> DomainId {
    // AggregateId is NonZeroU64 (≥ 1); subtraction is safe.
    DomainId::new(aggregate_id.get() - 1)
}

/// Wrap a pardosa error as a `StoreError::Infrastructure`. Categorised
/// retryable per CHE-0046:R1 — pardosa errors at this layer are
/// infrastructure failures, not data corruption.
fn pardosa_err(e: pardosa::PardosaError) -> StoreError {
    StoreError::Infrastructure(Box::new(e))
}

/// Allocate envelopes for an append. Mirrors `build_envelopes` in
/// `cherry-pit-core::testing` to keep envelope construction shape
/// uniform across in-memory and substrate-backed stores
/// (CHE-0042:R1, CHE-0033:R1 — `UUIDv7` event-ids generated here at
/// the store layer, not by callers).
fn build_envelopes<E: DomainEvent>(
    aggregate_id: AggregateId,
    start_sequence: u64,
    events: Vec<E>,
    context: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError> {
    let timestamp = jiff::Timestamp::now();
    let mut envelopes = Vec::with_capacity(events.len());
    for (i, payload) in events.into_iter().enumerate() {
        let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
        let raw = start_sequence
            .checked_add(i_u64)
            .and_then(|s| s.checked_add(1))
            .ok_or_else(|| {
                StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                    "sequence overflow",
                ))
            })?;
        let sequence = NonZeroU64::new(raw).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "sequence must be non-zero",
            ))
        })?;
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            aggregate_id,
            sequence,
            timestamp,
            context.correlation_id(),
            context.causation_id(),
            payload,
        )
        .map_err(|e| StoreError::Infrastructure(Box::new(e)))?;
        envelopes.push(envelope);
    }
    Ok(envelopes)
}

// ─── EventStore impl ──────────────────────────────────────────────

impl<E: DomainEvent> EventStore for PardosaEventStore<E> {
    type Event = E;

    fn load(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        // Acquire the lock synchronously, do the read under the lock,
        // drop the lock, then return a ready future. PAR-0008:R3 —
        // no lock held across `.await`.
        let result = self.load_sync(id);
        async move { result }
    }

    fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = StoreCreateResult<Self::Event>> + Send {
        let result = self.create_sync(events, &context);
        async move { result }
    }

    fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        let result = self.append_sync(id, expected_sequence, events, &context);
        async move { result }
    }
}

impl<E: DomainEvent> PardosaEventStore<E> {
    fn load_sync(&self, id: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let state = self.state.lock().expect("PardosaEventStore mutex poisoned");
        let stream: Vec<EventEnvelope<E>> = match state.by_aggregate.get(&id) {
            None => Vec::new(), // CHE-0019:R1 — unknown aggregate, empty vec.
            Some(&domain_id) => state
                .dragline
                .history(domain_id)
                .map_err(pardosa_err)?
                .into_iter()
                .map(|e| e.domain_event().clone())
                .collect(),
        };
        // CHE-0042:R4 — honour the conformance shape even though
        // in-process construction guarantees structural validity.
        EventEnvelope::validate_stream(id, &stream)
            .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
        Ok(stream)
    }

    fn create_sync(&self, events: Vec<E>, context: &CorrelationContext) -> StoreCreateResult<E> {
        if events.is_empty() {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(
                "cannot create aggregate with zero events",
            )));
        }
        let mut state = self.state.lock().expect("PardosaEventStore mutex poisoned");

        // Peek pardosa's next DomainId and translate to AggregateId
        // BEFORE allocating envelopes so the envelope's aggregate_id
        // is final at construction time (CHE-0042:R1).
        let next_domain = state.dragline.next_domain_id();
        let aggregate_id = domain_to_aggregate(next_domain)?;
        let envelopes = build_envelopes(aggregate_id, 0, events, context)?;

        // First event uses Dragline::create; remaining envelopes
        // are appended via Dragline::update under the same domain_id.
        let timestamp = envelopes[0].timestamp().as_microsecond();
        let first = envelopes[0].clone();
        let result = state
            .dragline
            .create(timestamp, first)
            .map_err(pardosa_err)?;
        debug_assert_eq!(result.domain_id, next_domain);

        for env in envelopes.iter().skip(1) {
            let ts = env.timestamp().as_microsecond();
            state
                .dragline
                .update(next_domain, ts, env.clone())
                .map_err(pardosa_err)?;
        }

        state.by_aggregate.insert(aggregate_id, next_domain);
        let last_seq = envelopes
            .last()
            .expect("non-empty by checks above")
            .sequence();
        state.last_sequence.insert(aggregate_id, last_seq);

        Ok((aggregate_id, envelopes))
    }

    fn append_sync(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<E>,
        context: &CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new()); // CHE store contract: empty append is no-op.
        }
        let mut state = self.state.lock().expect("PardosaEventStore mutex poisoned");

        let Some(&domain_id) = state.by_aggregate.get(&id) else {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(format!(
                "cannot append to aggregate {id}: not created (use create() first)"
            ))));
        };

        let actual_sequence = state.last_sequence.get(&id).map_or(0, |s| s.get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let envelopes = build_envelopes(id, expected_sequence.get(), events, context)?;
        for env in &envelopes {
            let ts = env.timestamp().as_microsecond();
            state
                .dragline
                .update(domain_id, ts, env.clone())
                .map_err(pardosa_err)?;
        }
        let new_last = envelopes
            .last()
            .expect("non-empty by checks above")
            .sequence();
        state.last_sequence.insert(id, new_last);
        Ok(envelopes)
    }
}

// ─── Replay (file-backed wrapper consumes this) ──────────────────
//
// `replay_envelopes` repopulates the inner Dragline from already-built
// envelopes. Used by `PardosaFileEventStore::open` to restore in-memory
// state from on-disk logs without re-fabricating envelope identity
// (CHE-0042:R1 — envelopes survive append→load byte-identical, so
// replay must not regenerate event_id/timestamp/sequence).
//
// Pre-condition: `by_aggregate` keys are dense from `AggregateId(1)`
// upward in sorted order (1, 2, 3, ...) — the file-store enumerator
// guarantees this via sorted dir-walk of `{id}.pardosa` files. Gaps
// (a purged-and-deleted aggregate id) would break pardosa's
// `next_domain_id()` allocation lock-step. v0.1 file-backed store
// never deletes aggregate files, so gaps cannot arise organically;
// any operator-induced gap is detected here and rejected.
impl<E: DomainEvent> PardosaEventStore<E> {
    /// Replay envelopes into the inner Dragline at construction time.
    ///
    /// # Errors
    ///
    /// - [`StoreError::CorruptData`] if `by_aggregate` keys are not
    ///   dense from `AggregateId(1)` (gap = corrupt restore).
    /// - [`StoreError::Infrastructure`] for any pardosa-substrate
    ///   failure during replay.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (unrecoverable
    /// invariant violation from another thread).
    pub fn replay_envelopes(
        &self,
        by_aggregate: BTreeMap<AggregateId, Vec<EventEnvelope<E>>>,
    ) -> Result<(), StoreError> {
        let mut state = self.state.lock().expect("PardosaEventStore mutex poisoned");
        // Walk in BTreeMap sorted order. Assert dense-from-1 — see the
        // module-level rationale above.
        let mut expected = NonZeroU64::new(1).expect("1 is non-zero");
        for (id, envelopes) in by_aggregate {
            if id.get() != expected.get() {
                return Err(StoreError::CorruptData(Box::<
                    dyn std::error::Error + Send + Sync,
                >::from(format!(
                    "replay aggregate-id gap: expected {expected}, got {id} \
                     (file-backed store does not delete aggregate files)",
                ))));
            }
            let Some(first) = envelopes.first().cloned() else {
                // Empty file — skip; allocation lock-step preserved
                // because we don't call next_domain_id.
                expected = expected
                    .checked_add(1)
                    .expect("AggregateId overflow during replay");
                continue;
            };

            // Verify the on-disk stream is structurally valid before
            // touching the substrate (CHE-0042:R4).
            EventEnvelope::validate_stream(id, &envelopes)
                .map_err(|e| StoreError::CorruptData(Box::new(e)))?;

            let next_domain = state.dragline.next_domain_id();
            let alloc_id = domain_to_aggregate(next_domain)?;
            if alloc_id != id {
                return Err(StoreError::CorruptData(Box::<
                    dyn std::error::Error + Send + Sync,
                >::from(format!(
                    "replay allocation mismatch: pardosa would assign {alloc_id} \
                     but on-disk aggregate is {id}",
                ))));
            }

            let timestamp = first.timestamp().as_microsecond();
            state
                .dragline
                .create(timestamp, first)
                .map_err(pardosa_err)?;

            for env in envelopes.iter().skip(1) {
                let ts = env.timestamp().as_microsecond();
                state
                    .dragline
                    .update(next_domain, ts, env.clone())
                    .map_err(pardosa_err)?;
            }

            state.by_aggregate.insert(id, next_domain);
            let last_seq = envelopes
                .last()
                .expect("non-empty by checks above")
                .sequence();
            state.last_sequence.insert(id, last_seq);

            expected = expected
                .checked_add(1)
                .expect("AggregateId overflow during replay");
        }
        Ok(())
    }
}

// ─── PurgeableEventStore impl (real per CHE-0059, PAR-0001) ───────

impl<E: DomainEvent> PurgeableEventStore for PardosaEventStore<E> {
    fn load_history(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        // load_history MUST return events even for Purged fibers
        // (CHE-0059:R1 — that is precisely the trait's purpose).
        // Pardosa retains events in `read_line()` after purge; the
        // `history()` API requires a live fiber. We therefore walk
        // `read_line()` and filter by DomainId.
        let result = self.load_history_sync(id);
        async move { result }
    }

    fn recreate(
        &self,
        id: AggregateId,
        tombstone: Self::Event,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        let result = self.recreate_sync(id, tombstone, events, &context);
        async move { result }
    }
}

impl<E: DomainEvent> PardosaEventStore<E> {
    fn load_history_sync(&self, id: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let state = self.state.lock().expect("PardosaEventStore mutex poisoned");
        // CHE-0019:R1 / CHE-0059:R5 — genuinely unknown aggregates
        // (never created on this store) return an empty vec.
        let Some(&domain_id) = state.by_aggregate.get(&id) else {
            return Ok(Vec::new());
        };
        // Walk the entire line and filter by DomainId. This includes
        // events for currently-Purged fibers, satisfying CHE-0059:R1.
        //
        // CHE-0042:R4 honoured per-incarnation: an aggregate's full
        // history may span multiple incarnations (each recreate severs
        // continuity per CHE-0059:R4 + PAR-0001, so post-recreate
        // sequences restart at 1). We segment the concatenated stream
        // at substrate-level incarnation boundaries (pardosa marks
        // these with `precursor = Index::NONE` via `create` /
        // `create_reuse`) and validate each segment as its own
        // contiguous 1..N stream. A flat `validate_stream` on the
        // concatenation would incorrectly reject the legitimate
        // sequence-restart at each incarnation boundary.
        let mut segments: Vec<Vec<EventEnvelope<E>>> = Vec::new();
        let mut current: Vec<EventEnvelope<E>> = Vec::new();
        for ev in state.dragline.read_line() {
            if ev.domain_id() != domain_id {
                continue;
            }
            if ev.precursor().is_none() && !current.is_empty() {
                segments.push(std::mem::take(&mut current));
            }
            current.push(ev.domain_event().clone());
        }
        if !current.is_empty() {
            segments.push(current);
        }
        for segment in &segments {
            EventEnvelope::validate_stream(id, segment)
                .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
        }
        Ok(segments.into_iter().flatten().collect())
    }

    fn recreate_sync(
        &self,
        id: AggregateId,
        tombstone: E,
        events: Vec<E>,
        context: &CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        if events.is_empty() {
            return Err(StoreError::Infrastructure(Box::<
                dyn std::error::Error + Send + Sync,
            >::from(
                "cannot recreate aggregate with zero events",
            )));
        }
        let mut state = self.state.lock().expect("PardosaEventStore mutex poisoned");

        let &domain_id = state.by_aggregate.get(&id).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "cannot recreate aggregate {id}: not previously created"
            )))
        })?;
        let alt_domain = aggregate_to_domain(id);
        debug_assert_eq!(alt_domain, domain_id);

        // PAR-0001 fiber state machine forbids `Migrate(Purge)` from
        // `Defined`; the legal path is `Defined → Detach → Detached
        // → Migrate(Purge) → Purged` (oracle adjudication
        // adr-fmt-1clv). `Dragline::detach` requires a domain-event
        // payload, which we cannot fabricate over the generic E —
        // CHE-0059:R2 therefore requires the caller to supply a
        // tombstone variant. We wrap it in an EventEnvelope at
        // last_sequence + 1 (the natural continuation of the pre-
        // purge stream) so the audit trail remains structurally
        // valid up to and including the detach marker.
        let last_seq = state.last_sequence.get(&id).copied().ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                "cannot recreate aggregate {id}: no last sequence cached"
            )))
        })?;
        let tombstone_seq_raw = last_seq.get().checked_add(1).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "tombstone sequence overflow",
            ))
        })?;
        let tombstone_seq = NonZeroU64::new(tombstone_seq_raw).ok_or_else(|| {
            StoreError::Infrastructure(Box::<dyn std::error::Error + Send + Sync>::from(
                "tombstone sequence must be non-zero",
            ))
        })?;
        let tombstone_ts = jiff::Timestamp::now();
        let tombstone_env = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            tombstone_seq,
            tombstone_ts,
            context.correlation_id(),
            context.causation_id(),
            tombstone,
        )
        .map_err(|e| StoreError::Infrastructure(Box::new(e)))?;

        // Defined → Detached.
        state
            .dragline
            .detach(domain_id, tombstone_ts.as_microsecond(), tombstone_env)
            .map_err(pardosa_err)?;

        // Detached → Purged.
        state
            .dragline
            .migrate_fiber(domain_id, MigrationPolicy::Purge)
            .map_err(pardosa_err)?;

        // Build envelopes for the fresh incarnation. Sequence
        // restarts at 1 — recreate is logical discontinuity
        // (CHE-0059:R4, PAR-0001).
        let envelopes = build_envelopes(id, 0, events, context)?;

        // Reuse the same DomainId via create_reuse (state machine:
        // Purged → Defined). Precursor is Index::NONE — severs the
        // hash chain per CHE-0059:R4 / PAR-0001.
        let first = envelopes[0].clone();
        let timestamp = first.timestamp().as_microsecond();
        state
            .dragline
            .create_reuse(domain_id, timestamp, first)
            .map_err(pardosa_err)?;

        for env in envelopes.iter().skip(1) {
            let ts = env.timestamp().as_microsecond();
            state
                .dragline
                .update(domain_id, ts, env.clone())
                .map_err(pardosa_err)?;
        }

        let last_seq = envelopes
            .last()
            .expect("non-empty by checks above")
            .sequence();
        state.last_sequence.insert(id, last_seq);
        Ok(envelopes)
    }
}

// ─── HashChainedEventStore impl (ROLLOUT STUB per CHE-0060:R3) ───
//
// PAR-0021 (per-stream BLAKE3 hash chain) is Accepted but unimplemented
// in pardosa source at HEAD. CHE-0057:R3 + CHE-0060:R3 carve out an
// always-failing rollout stub for exactly this case: the API surface
// is committed now so downstream callers can compile against it;
// failure is surfaced explicitly so callers cannot mistake the stub
// for a working impl.
//
// **REMOVE THIS ENTIRE impl BLOCK WHEN PAR-0021 LANDS** and replace
// with a real impl bridging pardosa's frontier + verify_precursor_chains.
// This is not a trait-level default impl (forbidden by CHE-0060) —
// it lives here on the concrete type where it is reviewable.

impl<E: DomainEvent> HashChainedEventStore for PardosaEventStore<E> {
    fn frontier_hash(&self) -> [u8; 32] {
        // PAR-0021 sentinel: all-zero hash signals "frontier not yet
        // computed by substrate." Callers requiring tamper evidence
        // bound on `HashChainedEventStore` and route the verification
        // path through `verify_chain`, which surfaces the rollout
        // stub failure (CHE-0060:R3).
        [0u8; 32]
    }

    async fn verify_chain(&self) -> Result<(), StoreError> {
        Err(StoreError::Infrastructure(Box::<
            dyn std::error::Error + Send + Sync,
        >::from(
            "HashChainedEventStore rollout stub (CHE-0060:R3): PAR-0021 \
             not yet implemented in pardosa substrate",
        )))
    }
}

// ─── SingleWriterEventStore impl (marker per CHE-0061) ────────────
//
// Zero-method marker — CHE-0061:R2 / R5. PardosaEventStore qualifies
// on the strength of PAR-0004:R1 substrate-level fencing (NATS
// Expected-Last-Subject-Sequence). At this milestone the in-memory
// dragline is single-process; the marker reflects substrate intent.

impl<E: DomainEvent> SingleWriterEventStore for PardosaEventStore<E> {}
