//! Shared helpers for `ApplicationService` triad bodies (CHE-0054:R10).
//!
//! Consolidates the load → handle → create-or-append → publish
//! pattern duplicated across [`RunService::start_sweep`] and the two
//! [`RepoService`] methods (B7'b Inc 2 + Inc 4). Each helper is a
//! free function generic over the concrete store/bus types per
//! CHE-0005:R1 (no `Box<dyn>`); shared mutable state (the routing
//! index, the sequence tracker) is passed by `&Arc<Mutex<...>>`
//! reference so callers retain ownership.
//!
//! ## Why free functions, not a trait
//!
//! The four service structs ([`RunService`], [`RepoService`],
//! [`WebhookService`]) each have the same field shape but different
//! aggregate state types ([`Run`], [`Repo`], [`WebhookDelivery`])
//! and different error types. A trait would either need an
//! associated `State`/`Error` type forcing every impl to provide a
//! fold + handle hook, or it would force the state fold into the
//! helper itself (raising the trait surface and the `Aggregate`
//! bound into the wrong layer). Free functions keep the fold step
//! at the call site where the concrete state type is known and
//! avoid leaking the routing/persistence shape into the aggregate
//! API.
//!
//! ## Why `WebhookService` is excluded
//!
//! [`WebhookService::ingest`] uses fresh-per-delivery semantics:
//! every call mints a new `AggregateId` via `EventStore::create`
//! unconditionally — there is no lazy index lookup, no fold, no
//! append branch. The structural exemption from the I1 TOCTOU
//! window (lazy lookup followed by `or_insert`) was confirmed by
//! linus on Inc 5. `WebhookService` still adopts [`publish_or_trace`]
//! for the publish step.
//!
//! ## I1 TOCTOU resolution (Track 4.0/3a Merger)
//!
//! In isolation, [`lookup`] + [`create_or_append`] would form a
//! check-then-act sequence on `Arc<Mutex<HashMap<String,
//! AggregateId>>>`: two concurrent service calls on the same
//! `domain_key` could in principle both observe `lookup → None`,
//! both call `EventStore::create`, and both call
//! `index.entry(...).or_insert(...)`. The second `or_insert` would
//! be a no-op (correct routing preserved) but its
//! `EventStore::create` would produce an **orphan aggregate stream**
//! on disk that the index never points to, with `next_seq` likewise
//! recording an unreachable entry.
//!
//! At Track 4.0/3a this window was closed structurally: every call
//! site for [`lookup`] / [`create_or_append`] now lives inside an arm
//! of [`super::merger::Merger::run`], a single-task command
//! processor that awaits each command's full triad (load → handle →
//! create-or-append → publish) before dequeuing the next. Two
//! concurrent same-`domain_key` service calls serialise at the
//! [`tokio::sync::mpsc`] front-door, so the second observer always
//! sees the first creator's index entry — exactly one
//! `EventStore::create` per key, no orphan stream. This is the
//! "per-domain-key single-flight / equivalent" requirement at the
//! coarsest granularity the current sole-writer SMI invariant
//! permits; finer keying (per-key `tokio::sync::Mutex` side map)
//! becomes interesting only if the Merger is later sharded.
//!
//! Note that [`create_or_append`] still releases the brief
//! `std::sync::Mutex` guard on the routing index **before** the
//! `await` on `EventStore::create` — the lock is taken only to
//! perform the `or_insert`, never held across storage I/O.
//!
//! Regression pin: see
//! `super::repo_service::tests::concurrent_same_domain_key_evaluations_create_exactly_one_aggregate`
//! — fans out 32 concurrent same-`domain_key` `record_evaluation`
//! calls and asserts exactly one routing entry, one `.msgpack` file,
//! `N` envelopes with monotonic sequences, and tracker `= N`. The
//! test fails immediately if anyone reintroduces a per-service
//! direct call path that bypasses the Merger.
//!
//! [`RunService::start_sweep`]: super::run_service::RunService::start_sweep
//! [`RepoService`]: super::repo_service::RepoService
//! [`WebhookService`]: super::webhook_service::WebhookService
//! [`WebhookService::ingest`]: super::webhook_service::WebhookService::ingest
//! [`Run`]: crate::domain::aggregates::run::Run
//! [`Repo`]: crate::domain::aggregates::repo::Repo
//! [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, CorrelationContext, EventBus, EventEnvelope, EventStore, StoreError};

use crate::domain::events::DomainEvent;

/// Borrow-bundle of the three persistence handles each
/// `ApplicationService` holds — the store, the routing index, and the
/// per-aggregate sequence tracker. Carried through
/// [`create_or_append`] (and any future shared persist-side helper)
/// to keep the function signature inside the
/// `clippy::too_many_arguments` budget without forcing call sites
/// to repack on each invocation.
///
/// Both fields are borrowed (`&'a Arc<...>`); ownership stays with
/// the service. The struct is `Copy` so it can be threaded through
/// helpers without `clone()` ceremony.
#[derive(Clone, Copy)]
pub(super) struct PersistHandles<'a, S>
where
    S: EventStore<Event = DomainEvent>,
{
    pub store: &'a Arc<S>,
    pub index: &'a Arc<Mutex<HashMap<String, AggregateId>>>,
    pub next_seq: &'a Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
}

/// Look up the `AggregateId` for `domain_key` from the routing
/// index (CHE-0054:R5). `None` ⇒ this is the first reference and
/// the next persistence step uses the create-path.
pub(super) fn lookup(
    index: &Arc<Mutex<HashMap<String, AggregateId>>>,
    domain_key: &str,
) -> Option<AggregateId> {
    let guard = index
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.get(domain_key).copied()
}

/// Load the per-aggregate event stream for `existing_id` (or return
/// `(vec![], None)` when no `AggregateId` is known yet — lazy
/// create-path on first reference). The caller is responsible for
/// folding the returned envelopes through its concrete aggregate
/// state via [`cherry_pit_core::Aggregate::apply`].
///
/// Returns the loaded envelopes alongside the last-applied sequence
/// (for caller-tracked CAS, CHE-0054:R6).
///
/// # Errors
///
/// Returns [`StoreError`] when `EventStore::load` fails or when an
/// indexed `AggregateId` resolves to zero envelopes (`StoreError::CorruptData` —
/// the routing index referenced a stream that no longer exists).
pub(super) async fn load_envelopes_or_empty<S>(
    store: &Arc<S>,
    existing_id: Option<AggregateId>,
) -> Result<(Vec<EventEnvelope<DomainEvent>>, Option<NonZeroU64>), StoreError>
where
    S: EventStore<Event = DomainEvent>,
{
    let Some(id) = existing_id else {
        return Ok((Vec::new(), None));
    };
    let envelopes = store.load(id).await?;
    let last_seq = envelopes
        .last()
        .map(cherry_pit_core::EventEnvelope::sequence)
        .ok_or_else(|| {
            StoreError::CorruptData(
                format!("indexed AggregateId {id} has zero envelopes (routing index stale)")
                    .into(),
            )
        })?;
    Ok((envelopes, Some(last_seq)))
}

/// Persist `new_events` via either the create-path
/// (`existing_id == None`) or the append-path with caller-tracked
/// CAS on `last_seq` (CHE-0054:R6 / CHE-0042:R3). Updates the
/// routing index (create-path only, via `or_insert`) and the
/// sequence tracker (both paths).
///
/// **I1 TOCTOU**: [`lookup`] followed by this helper is a
/// check-then-act sequence on the routing index. Safe in this crate
/// because every call site lives inside an arm of the single-task
/// [`super::merger::Merger`] which serialises commands per the
/// module-level "I1 TOCTOU resolution" docs. The brief
/// `std::sync::Mutex` guard taken to perform `or_insert` is released
/// before any `await` on storage I/O.
///
/// # Errors
///
/// Returns [`StoreError`] when the underlying `EventStore::create`
/// or `EventStore::append` call fails, or `StoreError::CorruptData`
/// when called with the `(Some, None)` shape (indexed `AggregateId`
/// without a tracked `last_seq` — a routing/load bug that
/// [`load_envelopes_or_empty`] normally surfaces first).
pub(super) async fn create_or_append<S>(
    handles: PersistHandles<'_, S>,
    domain_key: &str,
    existing_id: Option<AggregateId>,
    last_seq: Option<NonZeroU64>,
    new_events: Vec<DomainEvent>,
    ctx: &CorrelationContext,
) -> Result<Vec<EventEnvelope<DomainEvent>>, StoreError>
where
    S: EventStore<Event = DomainEvent>,
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

/// Publish to the bus, absorbing failure via a structured
/// `tracing::error!` emission **per envelope** on failure.
///
/// Per CHE-0024:R1 publication failure is non-fatal — events are
/// already durable on the `EventStore`; per CHE-0024:R3 consumers
/// reconcile via checkpointed replay from `EventStore::load`. The
/// per-envelope emission satisfies COM-0019:R1 (structured emission
/// at the absorb point), COM-0019:R4 (`correlation_id` flows through
/// the observability boundary), and COM-0019:R7 (`EventBus`
/// retry-absorb telemetry — `error!` severity makes the absorbed
/// failure operator-actionable).
///
/// `aggregate_kind` is not an [`EventEnvelope`] accessor on this
/// crate's surface (M2.a' brief D1' permits substitution); we emit
/// `aggregate_id` and keep the existing `event` static label
/// (`"SweepStarted"`, `"RepoEvaluated"`, etc.) which carries the
/// aggregate kind semantically and routes log analysis. Adding a
/// dedicated `aggregate_kind` accessor on `EventEnvelope` is tracked
/// for a future cherry-pit-core surface change; not in M2.a' scope.
pub(super) async fn publish_or_trace<B>(
    bus: &Arc<B>,
    envelopes: &[EventEnvelope<DomainEvent>],
    event_label: &'static str,
) where
    B: EventBus<Event = DomainEvent>,
{
    if let Err(bus_err) = bus.publish(envelopes).await {
        for env in envelopes {
            tracing::error!(
                target: "gh_report.eda",
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
