use std::future::Future;
use std::num::NonZeroU64;

use crate::aggregate_id::AggregateId;
use crate::correlation::CorrelationContext;
use crate::error::{StoreCreateResult, StoreError};
use crate::event::{DomainEvent, EventEnvelope};

/// Port for loading and persisting a single aggregate's event streams.
/// (CHE-0005 R1: port bound to single aggregate type; CHE-0016 R1–R2:
/// store creates envelopes; CHE-0018 R2: async RPITIT; CHE-0019 R1:
/// empty vec for unknown aggregates; CHE-0020 R2–R3: store assigns ID;
/// CHE-0025 R1–R2: RPITIT, no dyn Future allocation;
/// CHE-0033 R1–R3: UUID v7 `event_id`, sequence ordering.)
///
/// The event store is the single source of truth for aggregate state
/// in an event-sourced system. Every aggregate's history is an ordered
/// sequence of `EventEnvelope`s keyed by `(AggregateId, sequence)`.
/// Stores validate that loaded streams are gap-free, duplicate-free,
/// ordered by contiguous sequence numbers, and scoped to the requested
/// `AggregateId` before returning events to callers.
///
/// Each event store instance is bound to exactly one domain event type
/// via the `Event` associated type. This gives compile-time proof that
/// every load/append operates on the correct event type — the caller
/// cannot accidentally deserialize one aggregate's events as another's.
///
/// # Envelope construction
///
/// The store creates [`EventEnvelope`]s — callers pass raw domain
/// events and a [`CorrelationContext`]. The store assigns `event_id`
/// (UUID v7), `aggregate_id`, `sequence`, and `timestamp`, and
/// stamps `correlation_id`/`causation_id` from the context. This
/// eliminates redundancy and makes malformed envelopes impossible
/// by construction.
///
/// # ID assignment
///
/// New aggregates get their ID from [`create`](Self::create), which
/// auto-increments a `u64` counter. Callers never invent IDs.
///
/// # Single-writer assumption
///
/// Cherry-pit assumes single-writer aggregates. Optimistic concurrency
/// (`expected_sequence` on `append`) serves as defense-in-depth within
/// the single writer process.
///
/// This is a secondary (driven) port — the domain tells infrastructure
/// when to load and persist. Concrete implementations (in-memory for
/// testing, Pardosa-backed, PostgreSQL-backed) live in infrastructure
/// crates.
///
/// # Distributed failure model (COM-0025:R1)
///
/// The four failure modes that the trait can only document at the
/// trait level — because they straddle, predate, or outlive any single
/// async call — are stated here. The three method-scoped semantics
/// (timeout, cancellation, retry, duplicate-delivery, and per-method
/// replay on [`load`](Self::load)) are documented on the individual
/// methods below.
///
/// ## Crash
///
/// Implementors MUST be crash-safe: a process kill between calls (or
/// mid-call) MUST NOT leave the persisted stream in a state that
/// violates the gap-free, contiguous, ID-scoped invariants documented
/// on [`load`](Self::load). On restart, [`load`](Self::load) MUST
/// return either the pre-crash prefix or the post-`append` suffix,
/// never an interleaved or partially-fsynced view. Atomicity of
/// [`append`](Self::append) is the load-bearing primitive here
/// (see its docstring's "Atomic" clause).
///
/// ## Recovery
///
/// Implementor-level recovery procedures (orphan temporary files,
/// stale single-writer locks, partially-applied migrations, dead-letter
/// drains) live outside the port surface — they are operational
/// runbooks, not async calls. Each concrete `EventStore` implementation
/// MUST document its recovery procedure in its own crate-level rustdoc
/// alongside its crash-safety guarantees. The port contract here is:
/// after running the documented recovery procedure, the store MUST
/// satisfy every other invariant in this trait.
///
/// # Doctest — contract type-check
///
/// The trait is async and uses return-position `impl Future` (RPITIT,
/// CHE-0018:R2), so a doctest cannot `.await` without a runtime — and
/// cherry-pit-core declares zero async-runtime dependencies
/// (CHE-0029:R4). The doctest below therefore constructs the futures
/// and discards them; the test verifies that the trait's surface
/// type-checks against its documented signatures.
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
    /// Returns an empty `Vec` if no events exist for this aggregate.
    /// This is not an error — it means the aggregate has never been
    /// created. Implementations reject corrupt streams with
    /// [`StoreError::CorruptData`](crate::StoreError::CorruptData) when
    /// sequences are not exactly contiguous from 1..=N or an envelope
    /// belongs to a different aggregate ID.
    ///
    /// # Replay
    ///
    /// `load` is the replay primitive (COM-0025:R1). The returned `Vec`
    /// is the complete, ordered history of the aggregate — every event
    /// from sequence `1` through the latest, with no gaps and no
    /// duplicates, all scoped to the requested `AggregateId`. Callers
    /// MAY re-invoke `load` arbitrarily often; each call returns a
    /// fresh, point-in-time snapshot. Replay is a property of `load`
    /// alone; the [`EventBus`](crate::EventBus) is not a replay source.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// carrying an implementation-defined timeout error after the
    /// implementor's deadline elapses. Timeouts are categorised
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    ///
    /// # Cancellation
    ///
    /// Cancellation is drop-based per Rust async convention. Dropping
    /// the returned future before completion MUST be safe — `load` is
    /// read-only, so cancellation never leaves observable side effects.
    ///
    /// # Retry
    ///
    /// `load` is naturally idempotent (no observable side effects). On
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// or [`StoreError::StoreLocked`](crate::StoreError::StoreLocked)
    /// — both [`Retryable`](crate::ErrorCategory::Retryable) — callers
    /// MAY retry. [`StoreError::CorruptData`](crate::StoreError::CorruptData)
    /// is [`Terminal`](crate::ErrorCategory::Terminal) and MUST NOT be
    /// retried; it indicates a stream-integrity violation requiring
    /// implementor recovery (see trait-level "Recovery").
    ///
    /// # Duplicate delivery
    ///
    /// Not applicable on `load` — duplicate delivery is an append-side
    /// concern (see [`append`](Self::append)'s `expected_sequence`).
    fn load(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;

    /// Create a new aggregate — the store assigns the next ID.
    ///
    /// The store auto-increments a `u64` counter to assign the ID,
    /// creates [`EventEnvelope`]s from the raw domain events (assigning
    /// `event_id`, `sequence`, and `timestamp`), and persists them.
    ///
    /// Returns the assigned [`AggregateId`] and the created envelopes.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::Infrastructure` if `events` is empty —
    /// an aggregate cannot exist without at least one event.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// carrying an implementation-defined timeout error after the
    /// implementor's deadline elapses. Timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1) —
    /// but see "Retry" below for the duplicate-creation hazard.
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future MAY cancel `create` at any point.
    /// If the future is dropped after the store has assigned an ID and
    /// persisted the initial envelopes, the caller MUST treat the
    /// aggregate as potentially-created — its ID is unobservable to
    /// the caller, but the stream exists. Implementors MUST NOT leave
    /// a partial stream (i.e. ID assigned but envelopes not durably
    /// persisted, or vice versa). Atomic write semantics — same as
    /// [`append`](Self::append) — are required.
    ///
    /// # Retry
    ///
    /// `create` is NOT idempotent — each successful call produces a
    /// fresh `AggregateId`. A naive retry on
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// MAY create a duplicate aggregate if the original call actually
    /// succeeded but the response was lost. Callers needing safe retry
    /// MUST supply an [`IdempotencyKey`](crate::IdempotencyKey) at the
    /// command-bus layer (CHE-0041) rather than retrying `create`
    /// directly. The error category remains
    /// [`Retryable`](crate::ErrorCategory::Retryable) for infrastructure
    /// failures, but the retry must be guarded.
    ///
    /// # Duplicate delivery
    ///
    /// `create` has no concurrency-token parameter — duplicate
    /// invocation will create distinct aggregates with distinct IDs.
    /// De-duplication is the caller's responsibility (see "Retry"
    /// above and [`IdempotencyKey`](crate::IdempotencyKey)).
    fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = StoreCreateResult<Self::Event>> + Send;

    /// Append new events to an existing aggregate's stream.
    ///
    /// The aggregate must have been created via [`create`](Self::create)
    /// before calling `append`. Appending to a never-created aggregate
    /// is an error — implementations return `StoreError::Infrastructure`.
    ///
    /// The store creates [`EventEnvelope`]s from the raw domain events
    /// (assigning `event_id`, `sequence`, and `timestamp`) and persists
    /// them. Returns the created envelopes.
    ///
    /// `expected_sequence` is the sequence number of the last event
    /// the caller loaded, as a [`NonZeroU64`]. Since
    /// [`create`](Self::create) always produces ≥1 event, the last
    /// sequence is always ≥1 — the `NonZeroU64` type enforces this
    /// invariant at compile time. If the store's actual last sequence
    /// does not match, the append is rejected with
    /// `StoreError::ConcurrencyConflict`.
    ///
    /// Empty `events` is a no-op — returns `Ok(vec![])`.
    ///
    /// Atomic — either all events persist, or none do.
    ///
    /// # Timeout
    ///
    /// The returned future MAY resolve to
    /// [`StoreError::Infrastructure`](crate::StoreError::Infrastructure)
    /// carrying an implementation-defined timeout error after the
    /// implementor's deadline elapses. Timeouts are
    /// [`Retryable`](crate::ErrorCategory::Retryable) (CHE-0046:R1).
    ///
    /// # Cancellation
    ///
    /// Dropping the returned future MUST NOT leave a partial append
    /// observable to any subsequent [`load`](Self::load). The atomicity
    /// guarantee above is binding under drop: a cancelled `append`
    /// resolves to one of "all events persisted" or "no events
    /// persisted", never a prefix.
    ///
    /// # Retry
    ///
    /// `append` is idempotent under correct use of `expected_sequence`:
    /// a retry that observes the same `expected_sequence` against the
    /// same actual sequence will succeed exactly once. A retry after a
    /// successful-but-lost response sees the new sequence and is
    /// rejected with
    /// [`StoreError::ConcurrencyConflict`](crate::StoreError::ConcurrencyConflict)
    /// ([`Retryable`](crate::ErrorCategory::Retryable)); the caller
    /// then reloads via [`load`](Self::load) and either retries or
    /// surfaces the conflict.
    ///
    /// # Duplicate delivery
    ///
    /// `expected_sequence` is the duplicate-delivery defence:
    /// implementors MUST reject any `append` whose `expected_sequence`
    /// does not match the store's actual last sequence with
    /// [`StoreError::ConcurrencyConflict`](crate::StoreError::ConcurrencyConflict).
    /// Duplicate appends (same payload re-submitted after success)
    /// therefore fail the optimistic-concurrency check rather than
    /// extending the stream. The [`NonZeroU64`] type encodes that the
    /// stream is non-empty (created via [`create`](Self::create)) at
    /// the type level.
    fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send;
}
