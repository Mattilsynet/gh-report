//! Shared persist-side helpers — lifted from `gh-report`'s
//! `app::services::shared` (cite: original at
//! `crates/gh-report/src/app/services/shared.rs:1..280`).
//!
//! The four helpers consolidate the load → handle → create-or-append
//! → publish-or-trace pattern. They are private to the crate
//! (`pub(crate)`) — the public surface is the [`Merger`] +
//! [`MergerArm`] pair, not the individual steps. Consumers who need
//! finer-grained control implement their own [`MergerArm`].
//!
//! ## I1 TOCTOU resolution
//!
//! This is the canonical doctrine, lifted from the original module
//! docs at gh-report shared.rs:34-73 and amended for the
//! aggregate-agnostic surface:
//!
//! In isolation, [`lookup`] + [`create_or_append`] would form a
//! check-then-act sequence on `Arc<Mutex<HashMap<String,
//! AggregateId>>>`: two concurrent merger calls on the same
//! `domain_key` could in principle both observe `lookup → None`,
//! both call [`EventStore::create`], and both call
//! `index.entry(...).or_insert(...)`. The second `or_insert` would
//! be a no-op (correct routing preserved) but its
//! [`EventStore::create`] would produce an **orphan aggregate
//! stream** on disk that the index never points to, with the
//! sequence tracker likewise recording an unreachable entry.
//!
//! The merger closes this window structurally: every call site for
//! [`lookup`] / [`create_or_append`] lives inside an arm of the
//! merger's `run` loop (see [`crate::Merger`]), a single-task
//! command processor that awaits each command's full triad before
//! dequeuing the next. Two concurrent same-`domain_key` callers
//! serialise at the [`mpsc`] front-door, so the second observer
//! always sees the first creator's index entry — exactly one
//! [`EventStore::create`] per key, no orphan stream. This is the
//! per-domain-key single-flight requirement at the coarsest
//! granularity the sole-writer assumption permits; sharding (per-key
//! locks, partitioned index) becomes interesting only once
//! contention on the single mpsc front-door is measured.
//!
//! Note that [`create_or_append`] still releases the brief
//! [`std::sync::Mutex`] guard on the routing index **before** the
//! `await` on [`EventStore::create`] — the lock is taken only to
//! perform the `or_insert`, never held across storage I/O.
//!
//! Regression pin: `tests/i1_toctou_pin.rs` fans out 32 concurrent
//! same-`domain_key` `MergerArm` calls and asserts exactly one
//! routing entry, one aggregate id, `N` envelopes with monotonic
//! sequences, and tracker `= N`. The test fails immediately if
//! anyone reintroduces a per-arm direct call path that bypasses the
//! merger.
//!
//! [`Merger`]: crate::Merger
//! [`MergerArm`]: crate::MergerArm
//! [`EventStore::create`]: cherry_pit_core::EventStore::create
//! [`mpsc`]: https://docs.rs/tokio/latest/tokio/sync/mpsc/index.html

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventBus, EventEnvelope, EventStore, StoreError,
};

/// Borrow-bundle of the three persistence handles each merger arm
/// shares: the store, the routing index, and the per-aggregate
/// sequence tracker. Carried through [`create_or_append`] to keep the
/// function signature within the `clippy::too_many_arguments` budget
/// without forcing call sites to repack on each invocation.
///
/// Both fields are borrowed (`&'a Arc<...>`); ownership stays with
/// the merger. The struct derives [`Copy`] so it can be threaded
/// through helpers without `clone()` ceremony.
#[derive(Clone, Copy)]
pub(crate) struct PersistHandles<'a, S, E>
where
    S: EventStore<Event = E>,
    E: DomainEvent,
{
    pub store: &'a Arc<S>,
    pub index: &'a Arc<Mutex<HashMap<String, AggregateId>>>,
    pub next_seq: &'a Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
}

/// Look up the [`AggregateId`] for `domain_key` from the routing
/// index. `None` ⇒ this is the first reference and the next
/// persistence step uses the create-path.
pub(crate) fn lookup(
    index: &Arc<Mutex<HashMap<String, AggregateId>>>,
    domain_key: &str,
) -> Option<AggregateId> {
    let guard = index
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.get(domain_key).copied()
}

/// Load the per-aggregate event stream for `existing_id` (or return
/// `(vec![], None)` when no [`AggregateId`] is known yet — lazy
/// create-path on first reference). Caller folds the returned
/// envelopes through its concrete aggregate state via
/// [`cherry_pit_core::Aggregate::apply`].
///
/// Returns the loaded envelopes alongside the last-applied sequence
/// (for caller-tracked CAS per CHE-0042:R3).
///
/// # Errors
///
/// Returns [`StoreError`] when [`EventStore::load`] fails or when an
/// indexed [`AggregateId`] resolves to zero envelopes
/// ([`StoreError::CorruptData`] — the routing index referenced a
/// stream that no longer exists).
pub(crate) async fn load_envelopes_or_empty<S, E>(
    store: &Arc<S>,
    existing_id: Option<AggregateId>,
) -> Result<(Vec<EventEnvelope<E>>, Option<NonZeroU64>), StoreError>
where
    S: EventStore<Event = E>,
    E: DomainEvent,
{
    let Some(id) = existing_id else {
        return Ok((Vec::new(), None));
    };
    let envelopes = store.load(id).await?;
    let last_seq = envelopes
        .last()
        .map(EventEnvelope::sequence)
        .ok_or_else(|| {
            StoreError::CorruptData(
                format!("indexed AggregateId {id} has zero envelopes (routing index stale)").into(),
            )
        })?;
    Ok((envelopes, Some(last_seq)))
}

/// Persist `new_events` via either the create-path
/// (`existing_id == None`) or the append-path with caller-tracked
/// CAS on `last_seq` (CHE-0042:R3). Updates the routing index
/// (create-path only, via `or_insert`) and the sequence tracker
/// (both paths).
///
/// **I1 TOCTOU**: [`lookup`] followed by this helper is a
/// check-then-act sequence on the routing index. Safe in this
/// crate because every call site lives inside an arm of the
/// single-task merger which serialises commands per the
/// module-level "I1 TOCTOU resolution" docs. The brief
/// [`std::sync::Mutex`] guard taken to perform `or_insert` is
/// released before any `await` on storage I/O.
///
/// # Errors
///
/// Returns [`StoreError`] when [`EventStore::create`] or
/// [`EventStore::append`] fails, or [`StoreError::CorruptData`]
/// when called with the `(Some, None)` shape (indexed
/// [`AggregateId`] without a tracked `last_seq` — a routing/load
/// bug that [`load_envelopes_or_empty`] normally surfaces first).
///
/// [`EventStore::create`]: cherry_pit_core::EventStore::create
/// [`EventStore::append`]: cherry_pit_core::EventStore::append
pub(crate) async fn create_or_append<S, E>(
    handles: PersistHandles<'_, S, E>,
    domain_key: &str,
    existing_id: Option<AggregateId>,
    last_seq: Option<NonZeroU64>,
    new_events: Vec<E>,
    ctx: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError>
where
    S: EventStore<Event = E>,
    E: DomainEvent,
{
    match (existing_id, last_seq) {
        (None, _) => {
            let (assigned_id, envs) = handles.store.create(new_events, ctx.clone()).await?;
            {
                let mut guard = handles
                    .index
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.entry(domain_key.to_owned()).or_insert(assigned_id);
            }
            if let Some(env) = envs.last() {
                let seq = env.sequence();
                let mut guard = handles
                    .next_seq
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.insert(assigned_id, seq);
            }
            Ok(envs)
        }
        (Some(id), Some(seq)) => {
            let envs = handles
                .store
                .append(id, seq, new_events, ctx.clone())
                .await?;
            if let Some(env) = envs.last() {
                let next = env.sequence();
                let mut guard = handles
                    .next_seq
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.insert(id, next);
            }
            Ok(envs)
        }
        (Some(id), None) => Err(StoreError::CorruptData(
            format!(
                "indexed AggregateId {id} without last_seq; \
                 load_envelopes_or_empty should have surfaced this first"
            )
            .into(),
        )),
    }
}

/// Append `new_events` to `id` under caller-tracked CAS `last_seq`,
/// then update the sequence tracker. No routing-index touch (used by
/// [`crate::arm::PersistMode::AppendStrict`] paths where the key is
/// already known to be indexed).
///
/// # Errors
///
/// Returns [`StoreError`] when [`EventStore::append`] fails.
///
/// [`EventStore::append`]: cherry_pit_core::EventStore::append
pub(crate) async fn append_and_track<S, E>(
    store: &Arc<S>,
    next_seq: &Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    id: AggregateId,
    last_seq: NonZeroU64,
    new_events: Vec<E>,
    ctx: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError>
where
    S: EventStore<Event = E>,
    E: DomainEvent,
{
    let envs = store.append(id, last_seq, new_events, ctx.clone()).await?;
    if let Some(env) = envs.last() {
        let next = env.sequence();
        let mut guard = next_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(id, next);
    }
    Ok(envs)
}

/// Create a fresh aggregate from `new_events` (no domain-key
/// lookup), then index the assigned id under `domain_key` if
/// provided and update the sequence tracker.
///
/// Used by [`crate::arm::PersistMode::Create`] paths. `domain_key`
/// is `Option<&str>` because some create-paths (webhook ingest)
/// route by a key that lives *inside* the command but is not the
/// routing-index key — the original gh-report Merger
/// `handle_ingest_webhook` populates `deliveries_by_id` from
/// `cmd.delivery_id`, lifted here as the optional second argument.
///
/// # Errors
///
/// Returns [`StoreError`] when [`EventStore::create`] fails.
///
/// [`EventStore::create`]: cherry_pit_core::EventStore::create
pub(crate) async fn create_fresh<S, E>(
    handles: PersistHandles<'_, S, E>,
    domain_key: Option<&str>,
    new_events: Vec<E>,
    ctx: &CorrelationContext,
) -> Result<Vec<EventEnvelope<E>>, StoreError>
where
    S: EventStore<Event = E>,
    E: DomainEvent,
{
    let (assigned_id, envs) = handles.store.create(new_events, ctx.clone()).await?;
    if let Some(key) = domain_key {
        let mut guard = handles
            .index
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.entry(key.to_owned()).or_insert(assigned_id);
    }
    if let Some(env) = envs.last() {
        let seq = env.sequence();
        let mut guard = handles
            .next_seq
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.insert(assigned_id, seq);
    }
    Ok(envs)
}

/// Publish to the bus, absorbing failure via a structured
/// `tracing::error!` emission per envelope.
///
/// Per CHE-0024:R1 publication failure is non-fatal — events are
/// already durable on the [`EventStore`]; per CHE-0024:R3 consumers
/// reconcile via checkpointed replay. The per-envelope emission
/// satisfies COM-0019:R1 (structured emission at the absorb point),
/// COM-0019:R4 (`correlation_id` flows through the observability
/// boundary), and COM-0019:R7 (`EventBus` retry-absorb telemetry —
/// `error!` severity makes the absorbed failure operator-actionable).
///
/// The `event_label` argument is the static per-command label
/// supplied by [`crate::MergerArm::publish_label`].
///
/// [`EventStore`]: cherry_pit_core::EventStore
pub(crate) async fn publish_or_trace<B, E>(
    bus: &Arc<B>,
    envelopes: &[EventEnvelope<E>],
    event_label: &'static str,
) where
    B: EventBus<Event = E>,
    E: DomainEvent,
{
    if let Err(bus_err) = bus.publish(envelopes).await {
        for env in envelopes {
            tracing::error!(
                target: "cherry_pit_merger",
                event_id = %env.event_id(),
                correlation_id = ?env.correlation_id(),
                causation_id = ?env.causation_id(),
                aggregate_id = %env.aggregate_id(),
                event = event_label,
                error = ?bus_err,
                "EventBus::publish failed; events persisted, will be replayed by tracking processors"
            );
        }
    }
}
