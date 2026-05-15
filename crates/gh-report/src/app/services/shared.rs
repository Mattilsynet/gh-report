//! Shared helpers for ApplicationService triad bodies (CHE-0054:R10).
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
//! ## Why WebhookService is excluded
//!
//! [`WebhookService::ingest`] uses fresh-per-delivery semantics:
//! every call mints a new `AggregateId` via `EventStore::create`
//! unconditionally — there is no lazy index lookup, no fold, no
//! append branch. The structural exemption from the I1 TOCTOU
//! window (lazy lookup followed by `or_insert`) was confirmed by
//! linus on Inc 5. WebhookService still adopts [`publish_or_trace`]
//! for the publish step.
//!
//! ## I1 TOCTOU carry (B7'b → B7'c+)
//!
//! Both [`lookup`] + [`create_or_append`] together form a
//! check-then-act sequence on `Arc<Mutex<HashMap<String,
//! AggregateId>>>`: two concurrent service calls on the same
//! `domain_key` can both observe `lookup → None`, then both call
//! `EventStore::create`, then both call `index.entry(...).or_insert(...)`.
//! The second call's `or_insert` is a no-op (correct routing
//! preserved), but its `EventStore::create` produces an **orphan
//! aggregate stream** on disk that the index never points to. The
//! `sequence_tracker` likewise records an unreachable entry. Not
//! exercised by current tests; cross-thread concurrency on a single
//! `RepoService` / `RunService` instance is not part of the B7'b
//! threat model. Tracked in bd `adr-fmt-1uwm`; resolution candidates:
//!
//! 1. Hold the index lock across `EventStore::create` (serialises
//!    create-path on the routing index — not the per-aggregate
//!    write path).
//! 2. Use `DashMap::entry` semantics natively (per the brief's
//!    original `Arc<DashMap<...>>` intent — current `HashMap`
//!    behind `Mutex` was the B7'b simplification).
//! 3. Per-`domain_key` `tokio::sync::Mutex` keyed in a side map
//!    (highest cost, finest granularity).
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

use cherry_pit_core::{AggregateId, CorrelationContext, EventBus, EventEnvelope, EventStore};

use crate::domain::events::DomainEvent;

/// Borrow-bundle of the three persistence handles each
/// ApplicationService holds — the store, the routing index, and the
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
    pub sequence_tracker: &'a Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
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
/// (for caller-tracked CAS, CHE-0054:R6). Panics on
/// `EventStore::load` failure (typed surface in B7'c) or when the
/// indexed aggregate has zero events (corrupt routing).
pub(super) async fn load_envelopes_or_empty<S>(
    store: &Arc<S>,
    existing_id: Option<AggregateId>,
) -> (Vec<EventEnvelope<DomainEvent>>, Option<NonZeroU64>)
where
    S: EventStore<Event = DomainEvent>,
{
    let Some(id) = existing_id else {
        return (Vec::new(), None);
    };
    let envelopes = store
        .load(id)
        .await
        .expect("EventStore::load failure path enriched in B7'c");
    let last_seq = envelopes
        .last()
        .map(cherry_pit_core::EventEnvelope::sequence)
        .expect("indexed AggregateId must have ≥1 envelope (corrupt routing otherwise)");
    (envelopes, Some(last_seq))
}

/// Persist `new_events` via either the create-path
/// (`existing_id == None`) or the append-path with caller-tracked
/// CAS on `last_seq` (CHE-0054:R6 / CHE-0042:R3). Updates the
/// routing index (create-path only, via `or_insert`) and the
/// sequence tracker (both paths).
///
/// Panics on `EventStore::create` / `EventStore::append` failure or
/// the `(Some, None)` shape (indexed but unloaded — caught earlier
/// by [`load_envelopes_or_empty`]).
///
/// **I1 TOCTOU**: `lookup` followed by this helper is a
/// check-then-act sequence on the index. See module docs for the
/// carry note (bd `adr-fmt-1uwm`).
pub(super) async fn create_or_append<S>(
    handles: PersistHandles<'_, S>,
    domain_key: &str,
    existing_id: Option<AggregateId>,
    last_seq: Option<NonZeroU64>,
    new_events: Vec<DomainEvent>,
    ctx: &CorrelationContext,
) -> Vec<EventEnvelope<DomainEvent>>
where
    S: EventStore<Event = DomainEvent>,
{
    match (existing_id, last_seq) {
        (None, _) => {
            let (assigned_id, envs) = handles
                .store
                .create(new_events, ctx.clone())
                .await
                .expect("EventStore::create failure path enriched in B7'c");
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
                    .sequence_tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.insert(assigned_id, seq);
            }
            envs
        }
        (Some(id), Some(seq)) => {
            let envs = handles
                .store
                .append(id, seq, new_events, ctx.clone())
                .await
                .expect("EventStore::append failure path enriched in B7'c");
            if let Some(env) = envs.last() {
                let next = env.sequence();
                let mut guard = handles
                    .sequence_tracker
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.insert(id, next);
            }
            envs
        }
        (Some(_), None) => {
            unreachable!(
                "indexed AggregateId without last_seq is a routing/load bug; \
                 load_envelopes_or_empty enforces ≥1 envelope when existing_id is Some"
            )
        }
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
/// the observability boundary), and COM-0019:R7 (EventBus
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
