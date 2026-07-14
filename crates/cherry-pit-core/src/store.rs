use std::future::Future;
use std::num::NonZeroU64;

use crate::aggregate_id::AggregateId;
use crate::correlation::CorrelationContext;
use crate::error::{StoreCreateResult, StoreError};
use crate::event::{DomainEvent, EventEnvelope};

/// Port for loading and persisting a single aggregate's event streams
/// (CHE-0005:R1, CHE-0016:R1–R2, CHE-0018:R2, CHE-0019:R1, CHE-0020:R2–R3,
/// CHE-0025:R1–R2, CHE-0033:R1–R3).
///
/// Single source of truth for aggregate state: each history is an
/// ordered, gap-free, duplicate-free sequence of [`EventEnvelope`]s keyed
/// by `(AggregateId, sequence)`. The `Event` associated type binds one
/// store to one domain event type (compile-time cross-aggregate
/// deserialization proof). Secondary (driven) port; concrete
/// implementations (in-memory, Pardosa-backed, PostgreSQL-backed) live in
/// infrastructure crates.
///
/// The store, not the caller, creates [`EventEnvelope`]s and assigns new
/// `AggregateId`s via [`create`](Self::create) (CHE-0016:R1–R2,
/// CHE-0020). Single-writer aggregates are assumed; `expected_sequence`
/// on [`append`](Self::append) is defense-in-depth, not the primary
/// guard.
///
/// # Distributed failure model (COM-0025:R1)
///
/// Crash/recovery are trait-level (they straddle individual calls);
/// timeout, cancellation, retry, duplicate-delivery, and replay are
/// documented per-method below. Implementors MUST be crash-safe: a kill
/// between or mid-call MUST NOT violate the invariants on
/// [`load`](Self::load) — atomicity of [`append`](Self::append) is the
/// load-bearing primitive. Recovery is an operational runbook documented
/// per implementation; after recovery the store MUST satisfy every
/// invariant documented here.
///
/// Doctest type-checks the trait surface without `.await` — RPITIT
/// (CHE-0018:R2) needs a runtime, and this crate has zero async-runtime
/// deps (CHE-0029:R4).
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::{AggregateId, CorrelationContext, EventStore};
///
/// fn _type_check<S: EventStore>(
///     store: &S,
///     id: AggregateId,
///     events: Vec<S::Event>,
///     ctx: CorrelationContext,
///     last_seq: NonZeroU64,
/// ) {
///     let _load = store.load(id);
///     let _create = store.create(events.clone(), ctx.clone());
///     let _append = store.append(id, last_seq, events, ctx);
/// }
/// ```
pub trait EventStore: Send + Sync + 'static {
    /// The single domain event type this store persists.
    type Event: DomainEvent;

    /// Load all events for an aggregate, ordered by sequence.
    ///
    /// Empty `Vec` if none exist — not an error; the aggregate was
    /// never created (CHE-0019:R1).
    ///
    /// # Errors
    ///
    /// [`StoreError::CorruptData`](crate::StoreError::CorruptData)
    /// ([`Terminal`](crate::ErrorCategory::Terminal), never retry — see
    /// trait-level "Recovery") on non-contiguous sequences or a
    /// cross-aggregate envelope.
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// or [`StoreError::StoreLocked`](crate::StoreError::StoreLocked)
    /// on timeout (both [`Retryable`](crate::ErrorCategory::Retryable),
    /// CHE-0046:R1); `load` has no side effects, so retry is always
    /// safe.
    ///
    /// # Replay
    ///
    /// The replay primitive (COM-0025:R1): complete, ordered, gap-free,
    /// duplicate-free history for the requested `AggregateId`. Callers
    /// MAY re-invoke for a fresh snapshot; the
    /// [`EventBus`](crate::EventBus) is not a replay source.
    ///
    /// # Cancellation / duplicate delivery
    ///
    /// Drop-based cancellation is always safe (`load` is read-only).
    /// Duplicate delivery is an append-side concern (see
    /// [`append`](Self::append)'s `expected_sequence`).
    fn load(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;

    /// Create a new aggregate — the store assigns the next ID.
    ///
    /// Auto-increments a `u64` counter for the ID, creates
    /// [`EventEnvelope`]s from the raw domain events, and persists
    /// them. Returns the assigned [`AggregateId`] and the created
    /// envelopes.
    ///
    /// # Errors
    ///
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// if `events` is empty (an aggregate needs ≥1 event) or on timeout
    /// ([`Retryable`](crate::ErrorCategory::Retryable), CHE-0046:R1) —
    /// see "Retry" for the duplicate-creation hazard.
    ///
    /// # Cancellation
    ///
    /// Dropping the future MAY cancel `create` at any point; if dropped
    /// post-persistence the caller MUST treat the aggregate as
    /// potentially-created. Implementors MUST NOT leave a partial
    /// stream; atomicity matches [`append`](Self::append).
    ///
    /// # Retry / duplicate delivery
    ///
    /// NOT idempotent — each success produces a fresh `AggregateId`,
    /// and no concurrency-token parameter exists, so a naive retry or a
    /// duplicate call MAY produce a second aggregate. Callers needing
    /// safe retry MUST supply an
    /// [`IdempotencyKey`](crate::IdempotencyKey) at the command-bus
    /// layer (CHE-0041) rather than retrying `create` directly.
    fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = StoreCreateResult<Self::Event>> + Send;

    /// Append new events to an existing aggregate's stream.
    ///
    /// The aggregate must already exist via [`create`](Self::create).
    /// Empty `events` is a no-op returning `Ok(vec![])`. Persists
    /// atomically — either all events persist, or none do. Returns the
    /// created envelopes.
    ///
    /// `expected_sequence` is the caller's last-loaded sequence
    /// ([`NonZeroU64`], always ≥1 since [`create`](Self::create)
    /// produces ≥1 event). A mismatch against the actual last sequence
    /// rejects the append with `StoreError::ConcurrencyConflict` — this
    /// is also the duplicate-delivery defence (CHE-0041:R3): a
    /// re-submitted duplicate append fails this check rather than
    /// extending the stream.
    ///
    /// # Errors
    ///
    /// `StoreError::Infrastructure` if the aggregate was never created,
    /// or on timeout ([`Retryable`](crate::ErrorCategory::Retryable),
    /// CHE-0046:R1).
    /// [`StoreError::ConcurrencyConflict`](crate::StoreError::ConcurrencyConflict)
    /// on `expected_sequence` mismatch (also `Retryable`).
    ///
    /// # Cancellation
    ///
    /// Dropping the future MUST NOT leave a partial append observable
    /// to any subsequent [`load`](Self::load) — resolves to "all
    /// persisted" or "none persisted", never a prefix.
    ///
    /// # Retry
    ///
    /// Idempotent under correct `expected_sequence` use: a retry
    /// observing the same expected-vs-actual sequence succeeds once. A
    /// retry after a successful-but-lost response sees the new
    /// sequence and is rejected with `ConcurrencyConflict`; the caller
    /// reloads via [`load`](Self::load) and retries or surfaces it.
    fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;
}

/// Optional [`EventStore`] capability: read-only event-history replay and
/// causal-chain reconstruction from stored envelope metadata.
///
/// `EventHistoryEventStore` is the CHE-0076 capability introduced under
/// CHE-0057's extension-trait composition policy: it lives alongside
/// [`EventStore`], extends it as a supertrait, and preserves the inherited
/// single-aggregate [`EventStore::Event`] binding. Implementations return
/// stored [`EventEnvelope`]s only; callers interpret the envelopes' existing
/// `event_id`, `correlation_id`, and `causation_id` metadata themselves.
pub trait EventHistoryEventStore: EventStore {
    /// Load the ordered event history for one aggregate.
    ///
    /// This is a read-only wrapper over [`EventStore::load`] ordering.
    /// Unknown aggregates return `Ok(Vec::new())`, preserving CHE-0019:R1
    /// and CHE-0076:R6 boundary semantics.
    ///
    /// # Errors
    ///
    /// Returns the same [`StoreError`] values as [`EventStore::load`].
    fn history(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;

    /// Load the prefix of one aggregate's ordered history through `upto`.
    ///
    /// The prefix is selected from [`EventStore::load`] order by envelope
    /// [`EventEnvelope::sequence`]; no parallel ordering or index is part
    /// of this contract.
    ///
    /// # Errors
    ///
    /// Returns the same [`StoreError`] values as [`EventStore::load`].
    fn replay_until(
        &self,
        id: AggregateId,
        upto: NonZeroU64,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;

    /// Reconstruct the causal chain ending at `event_id` within one stream.
    ///
    /// Implementations walk the load-ordered stream at read time using only
    /// [`EventEnvelope::event_id`], [`EventEnvelope::correlation_id`], and
    /// [`EventEnvelope::causation_id`]. The returned envelopes are stored
    /// envelopes, ordered from the root cause through the requested event.
    ///
    /// # Errors
    ///
    /// Returns the same [`StoreError`] values as [`EventStore::load`].
    fn causal_chain(
        &self,
        id: AggregateId,
        event_id: uuid::Uuid,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;
}

/// Optional [`EventStore`] capability: the substrate supports physical
/// purge of an aggregate's event history followed by re-creation under
/// the same id with a fresh stream (e.g. pardosa's PGN-0002 fiber
/// state machine `Purged → Defined` transition, which severs logical
/// continuity by setting `precursor = Index::NONE`).
///
/// Substrates that cannot physically purge MUST NOT implement this
/// trait per CHE-0057:R3 — returning a not-implemented stub from
/// required methods is forbidden, and the rollout-stub carve-out in
/// CHE-0057:R3 does not apply to purge (CHE-0059:R3).
///
/// Governing ADR: CHE-0059. Citations: CHE-0019:R1 (`load_history`
/// returns `Ok(Vec::new())` for unknown aggregates), CHE-0039:R1
/// (recreate threads `CorrelationContext` — no `Default` fabrication),
/// PGN-0002 (substrate origin).
pub trait PurgeableEventStore: EventStore {
    /// Load the full event history of an aggregate, including
    /// aggregates whose current state is purged. Returns
    /// `Ok(Vec::new())` for genuinely unknown aggregates (CHE-0019:R1
    /// + CHE-0059:R5).
    ///
    /// The substrate distinguishes "purged, history retained" from
    /// "never created" — both arrive here, but the former returns its
    /// retained history, the latter returns the empty vec.
    fn load_history(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;

    /// Recreate an aggregate previously in the purged state with a
    /// fresh stream of events. The substrate MUST sever logical
    /// continuity with the prior incarnation (CHE-0059:R4 — CHE-0059
    /// is deprecated with no successor, retained as historical record;
    /// pardosa set the new precursor index to NONE per PAR-0001).
    ///
    /// `tombstone` is the final domain event recorded against the
    /// prior incarnation. Substrates recording detachment as an
    /// audit-trail event require a real payload; others MAY ignore it.
    /// The adapter cannot fabricate `Self::Event` (opaque at the port
    /// boundary), so callers supply the tombstone variant (CHE-0059:R4,
    /// oracle adjudication `adr-fmt-1clv`).
    ///
    /// `context` carries [`CorrelationContext`] per CHE-0039:R1 —
    /// callers MUST supply it explicitly; the substrate MUST NOT
    /// fabricate a default (CHE-0039:R2).
    fn recreate(
        &self,
        id: AggregateId,
        tombstone: Self::Event,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;
}

/// Optional [`EventStore`] capability: the substrate maintains a
/// per-stream BLAKE3 hash chain (PGN-0005 `precursor_hash` plus
/// `Dragline::frontier`) for cryptographic tamper evidence beyond
/// CHE-0016's structural envelope.
///
/// `PardosaEventStore` MAY implement this trait as an always-failing
/// rollout stub returning [`StoreError::Infrastructure`] from both
/// methods until PGN-0005 lands in pardosa source — this is the named
/// CHE-0057:R3 / CHE-0060:R3 carve-out. The stub MUST be documented
/// in the `impl` block and MUST be removed when PAR-0021 lands.
/// **No trait-level default impl** is permitted: a default would hide
/// the stub from review and silently survive PAR-0021's arrival.
///
/// Governing ADR: CHE-0060. Citations: CHE-0016 (structural envelope
/// baseline), PGN-0005 (substrate origin), SEC-0011 (deferred non-
/// repudiation consumer).
pub trait HashChainedEventStore: EventStore {
    /// 32-byte BLAKE3 frontier hash over all committed events in
    /// append order (PGN-0005:R3 + CHE-0060:R2).
    ///
    /// Returns the hash unconditionally (no `Result`): the substrate
    /// MUST be able to surface its current frontier. Rollout-stub
    /// implementations per CHE-0060:R3 surface failure through
    /// [`verify_chain`](Self::verify_chain) instead.
    fn frontier_hash(&self) -> [u8; 32];

    /// Verify the precursor-hash chain over the entire stream. Per
    /// PGN-0005:R5, rejects any event whose `precursor_hash` does not
    /// match the BLAKE3 hash of the referenced predecessor's canonical
    /// bytes.
    ///
    /// Sub-stream verification is not exposed today: pardosa's
    /// substrate API (`Dragline::verify_precursor_chains`) is whole-
    /// stream only. If a substrate later supports sub-stream
    /// verification a superseding ADR introduces the shape per
    /// CHE-0057:R5 — pre-committing a range parameter now would
    /// freeze the wrong shape under append-only.
    fn verify_chain(&self) -> impl Future<Output = Result<(), StoreError>> + Send;
}

/// Optional [`EventStore`] marker capability: the substrate guarantees
/// single-writer-per-aggregate-stream semantics at the substrate layer
/// (e.g. pardosa's PGN-0010:R6 backend stance).
///
/// Zero methods (CHE-0061:R2) — purely a type-system signal for
/// downstream code that requires single-writer guarantees. Adding
/// methods is forbidden per CHE-0061:R5 and would re-classify the
/// trait away from marker shape; any addition requires a superseding
/// ADR.
///
/// Governing ADR: CHE-0061. Citations: CHE-0006 (single-writer
/// architectural assumption it makes observable), PGN-0010
/// (substrate-level enforcement origin).
pub trait SingleWriterEventStore: EventStore {}

/// Optional [`EventStore`] capability: the substrate can enumerate every
/// known `AggregateId` cheaply.
///
/// Cherry-pit treats aggregate enumeration as a substrate-level
/// capability: not every store needs to expose it (CHE-0005:R1 — base
/// `EventStore` is the minimum port), but boot-time projection replay
/// (gh-report, adr-srv) needs to walk every aggregate stream.
///
/// File-backed substrates (PGNO and its predecessors) enumerate cheaply
/// by directory listing; in-process stores enumerate by reading the
/// keyset of their stream map. Stores that cannot enumerate without
/// `O(stream)` cost (future remote substrates) should not implement this
/// trait — callers must reach for an external index instead.
pub trait ListableEventStore: EventStore {
    /// Return every `AggregateId` known to the store, in unspecified
    /// order. Empty `Vec` for an empty store; never `Err` for an empty
    /// store. Errors reflect substrate I/O failure or task-join failure
    /// only.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] if the substrate fails to
    /// enumerate (e.g. filesystem I/O error on a file-backed store).
    /// Returns [`StoreError::JoinFailure`] if a `spawn_blocking` task
    /// invoked by the implementation fails to join (runtime shutdown or
    /// blocking-body panic). Per CHE-0070:R6 file-backed impls MUST wrap
    /// blocking syscalls in `tokio::task::spawn_blocking`.
    fn list_aggregates(&self) -> impl Future<Output = Result<Vec<AggregateId>, StoreError>> + Send;
}
