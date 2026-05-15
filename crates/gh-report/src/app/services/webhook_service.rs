//! `WebhookService` — ApplicationService for the [`WebhookDelivery`]
//! aggregate (CHE-0054:R4).
//!
//! Owns the single `ingest` use case for the degenerate
//! WebhookDelivery aggregate. Resolves `delivery_id → AggregateId`
//! via the shared `Arc<Mutex<HashMap<String, AggregateId>>>` index
//! handle (CHE-0054:R5); a duplicate delivery_id command lands on
//! the already-terminal aggregate and returns
//! [`WebhookError::AlreadyReceived`](crate::domain::aggregates::webhook::WebhookError::AlreadyReceived)
//! — the event-sourced replacement for `webhook/mod.rs`'s in-memory
//! `seen_deliveries` cache (post-WU-7 migration target).
//!
//! ## Method body status
//!
//! `WebhookService::ingest` is wired (Inc B7'b-5). B7'c migrates the
//! 4 production publish sites in `webhook/mod.rs` to this call.
//!
//! ## Fresh-per-delivery semantics
//!
//! Unlike `RunService` / `RepoService`, every call to `ingest` mints
//! a **fresh `AggregateId`** via `EventStore::create` — there is no
//! lazy index lookup, no routing-key reuse. The `delivery_id` is
//! recorded into the routing index (and the assigned id into the
//! sequence_tracker) for symmetry with the other services and so any
//! future cache-based dedup can read this surface, but **routing
//! does not gate persistence**: WebhookDelivery is a degenerate
//! single-event terminal aggregate (CHE-0054:R3) and idempotency
//! against duplicate delivery_ids stays at the call-site
//! (`webhook/mod.rs` `seen_deliveries` cache) for B7'b. The
//! aggregate-level `WebhookError::AlreadyReceived` invariant only
//! fires on a routing-cache miss against a re-loaded existing
//! aggregate, which is unreachable from this fresh-per-delivery
//! create-path.
//!
//! [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, CorrelationContext, EventBus, EventStore};

use crate::domain::aggregates::webhook::{RecordDelivery, WebhookError};
use crate::domain::events::DomainEvent;

/// ApplicationService for the [`WebhookDelivery`] aggregate.
///
/// Generic over the concrete [`EventStore`] and [`EventBus`] per
/// CHE-0005:R1 — see [`RunService`](super::run_service::RunService)
/// docs for routing/CAS rationale.
///
/// [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
#[derive(Debug)]
pub struct WebhookService<S, B>
where
    S: EventStore<Event = DomainEvent>,
    B: EventBus<Event = DomainEvent>,
{
    /// Durable per-aggregate event store.
    store: Arc<S>,
    /// Synchronous in-process event bus.
    bus: Arc<B>,
    /// `delivery_id` → `AggregateId` routing index (CHE-0054:R5).
    /// Populated as a side-effect of every successful `ingest`; the
    /// fresh-per-delivery create-path does **not** read this index
    /// (see module docs).
    index: Arc<Mutex<HashMap<String, AggregateId>>>,
    /// Last-applied sequence per aggregate (CHE-0054:R6). For the
    /// degenerate WebhookDelivery aggregate this is always `1` after
    /// the create-path completes.
    sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
}

impl<S, B> WebhookService<S, B>
where
    S: EventStore<Event = DomainEvent>,
    B: EventBus<Event = DomainEvent>,
{
    /// Construct a `WebhookService` wired to the given store, bus, and
    /// shared routing/sequence handles.
    #[must_use]
    pub fn with_stores(
        store: Arc<S>,
        bus: Arc<B>,
        index: Arc<Mutex<HashMap<String, AggregateId>>>,
        sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
    ) -> Self {
        Self {
            store,
            bus,
            index,
            sequence_tracker,
        }
    }

    /// Read access to the store handle (for diagnostics / tests).
    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    /// Read access to the bus handle (for diagnostics / tests).
    #[must_use]
    pub fn bus(&self) -> &Arc<B> {
        &self.bus
    }

    /// Ingest a single GitHub webhook delivery.
    ///
    /// **Fresh-per-delivery create-path** (CHE-0054:R10,
    /// CHE-0024:R3): handle the command on a fresh
    /// `WebhookDelivery::default()`, persist the resulting single
    /// `WebhookReceived` event via `EventStore::create` (the store
    /// assigns the `AggregateId`), record the
    /// `delivery_id → AggregateId` routing (CHE-0054:R5) and the
    /// `assigned_id → NonZeroU64(1)` sequence (CHE-0054:R6), then
    /// publish the envelope synchronously via the in-process bus.
    ///
    /// `EventBus::publish` failure is **non-fatal** per CHE-0024:R1;
    /// persistence is the source of truth, a publish error is logged
    /// at `warn!` and swallowed.
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError::AlreadyReceived`] only when a future
    /// caller wires routing-based dedup against this surface; the
    /// fresh-per-delivery create-path of B7'b cannot reach this
    /// branch (see module docs). Persistence failures currently
    /// panic via `expect`; B7'c enriches `WebhookError`
    /// (`#[non_exhaustive]`) with typed variants.
    ///
    /// # Panics
    ///
    /// Panics on `EventStore::create` failure (B7'b interim
    /// posture). B7'c enriches [`WebhookError`] with typed variants
    /// and propagates instead of panicking.
    pub async fn ingest(
        &self,
        cmd: RecordDelivery,
        ctx: &CorrelationContext,
    ) -> Result<(), WebhookError> {
        use cherry_pit_core::HandleCommand;

        let delivery_id = cmd.delivery_id.clone();

        // 1+2. Empty-state load: fresh aggregate per delivery —
        //      no index lookup, no fold (CHE-0054:R3 degenerate).
        let state = crate::domain::aggregates::webhook::WebhookDelivery::default();

        // 3. Handle (pure). Always emits a single WebhookReceived on
        //    a default state.
        let new_events = state.handle(cmd)?;

        // 4. Persist via create — store assigns AggregateId and
        //    sequence 1.
        let (assigned_id, new_envelopes) = self
            .store
            .create(new_events, ctx.clone())
            .await
            .expect("EventStore::create failure path enriched in B7'c");

        // 5a. Record routing (delivery_id → AggregateId) for any
        //     future cache-based dedup at this surface. Scope-block
        //     guards the MutexGuard across the .await above and the
        //     bus publish below (canonical fix per Rust 1.95
        //     NLL/MIR; see linus L1 verdict on Inc 4).
        {
            let mut guard = self
                .index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.entry(delivery_id).or_insert(assigned_id);
        }

        // 5b. Record per-aggregate sequence (always 1 for the
        //     degenerate WebhookDelivery aggregate).
        if let Some(env) = new_envelopes.last() {
            let seq = env.sequence();
            let mut guard = self
                .sequence_tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.insert(assigned_id, seq);
        }

        // 6. Publish — non-fatal per CHE-0024:R1.
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "WebhookReceived").await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use cherry_pit_agent::InProcessEventBus;
    use cherry_pit_core::{EventEnvelope, EventStore};
    use cherry_pit_gateway::MsgpackFileStore;
    use tempfile::TempDir;

    #[allow(
        clippy::type_complexity,
        reason = "test helper returns the four shared handles plus the service; factoring would obscure the wiring under test"
    )]
    fn build_service() -> (
        TempDir,
        Arc<MsgpackFileStore<DomainEvent>>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<String, AggregateId>>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        WebhookService<MsgpackFileStore<DomainEvent>, InProcessEventBus<DomainEvent>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::<DomainEvent>::new(dir.path()));
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let index = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let svc = WebhookService::with_stores(
            Arc::clone(&store),
            Arc::clone(&bus),
            Arc::clone(&index),
            Arc::clone(&tracker),
        );
        (dir, store, bus, index, tracker, svc)
    }

    #[test]
    fn with_stores_constructs_service() {
        let (_dir, _store, _bus, _index, _tracker, svc) = build_service();
        let _: &Arc<MsgpackFileStore<DomainEvent>> = svc.store();
        let _: &Arc<InProcessEventBus<DomainEvent>> = svc.bus();
    }

    /// Inc 5 (B7'b-5) — single ingest produces one aggregate with one
    /// `WebhookReceived` event at sequence 1.
    ///
    /// Asserts:
    ///   1. Stream contains 1 envelope at sequence 1 with payload
    ///      `WebhookReceived { action, repo, .. }`.
    ///   2. Bus subscriber captured exactly one envelope.
    ///   3. Routing index populated: delivery_id → assigned_id.
    ///   4. Sequence tracker == NonZeroU64(1).
    ///   5. Single per-aggregate file (CHE-0036:R1).
    #[tokio::test]
    async fn ingest_create_path_single_event() {
        let (dir, store, bus, index, tracker, svc) = build_service();

        let captured: Arc<Mutex<Vec<EventEnvelope<DomainEvent>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let captured_for_handler = Arc::clone(&captured);
        bus.register(move |env: &EventEnvelope<DomainEvent>| {
            captured_for_handler
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(env.clone());
        });

        let ctx = CorrelationContext::none();
        let delivery_id = "delivery-abc-123";

        svc.ingest(
            RecordDelivery {
                delivery_id: delivery_id.into(),
                action: "enqueue".into(),
                repo: Some("octocat/hello".into()),
                timestamp: "2026-05-10T12:00:00Z".into(),
            },
            &ctx,
        )
        .await
        .expect("ingest");

        // (3) Routing index resolves.
        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(delivery_id)
                .expect("index should map delivery_id")
        };

        // (1) Stream contents — single envelope at sequence 1.
        let loaded = store.load(assigned_id).await.expect("load");
        assert_eq!(loaded.len(), 1, "single ingest yields single envelope");
        assert_eq!(loaded[0].sequence().get(), 1);
        match loaded[0].payload() {
            DomainEvent::WebhookReceived { action, repo, .. } => {
                assert_eq!(action, "enqueue");
                assert_eq!(repo.as_deref(), Some("octocat/hello"));
            }
            other => panic!("expected WebhookReceived, got {other:?}"),
        }

        // (2) Bus capture.
        {
            let captured_envs = captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(captured_envs.len(), 1);
            assert_eq!(captured_envs[0].sequence().get(), 1);
        }

        // (4) Sequence tracker == 1.
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("sequence_tracker entry")
        };
        assert_eq!(tracked_seq.get(), 1);

        // (5) Single per-aggregate file (CHE-0036:R1).
        let store_file = dir.path().join(format!("{assigned_id}.msgpack"));
        assert!(store_file.exists());
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "msgpack"))
            .collect();
        assert_eq!(entries.len(), 1);
    }

    /// Inc 5 — fresh-per-delivery semantics: two ingests with the
    /// **same** delivery_id mint **two distinct** aggregates (no
    /// routing-key reuse).
    ///
    /// This documents the explicit B7'b contract: idempotency
    /// against duplicate delivery_ids is a call-site concern
    /// (`webhook/mod.rs` `seen_deliveries` cache), NOT a service
    /// invariant. Two msgpack files exist, each with one event at
    /// sequence 1. The index records the **first** assignment
    /// (`or_insert` semantics) and the sequence_tracker records both
    /// assigned ids.
    #[tokio::test]
    async fn ingest_fresh_per_delivery_does_not_dedupe_on_delivery_id() {
        let (dir, store, _bus, index, tracker, svc) = build_service();

        let ctx = CorrelationContext::none();
        let delivery_id = "duplicate-delivery";

        svc.ingest(
            RecordDelivery {
                delivery_id: delivery_id.into(),
                action: "enqueue".into(),
                repo: Some("a/b".into()),
                timestamp: "2026-05-10T12:00:00Z".into(),
            },
            &ctx,
        )
        .await
        .expect("first ingest");

        svc.ingest(
            RecordDelivery {
                delivery_id: delivery_id.into(),
                action: "enqueue".into(),
                repo: Some("a/b".into()),
                timestamp: "2026-05-10T12:00:01Z".into(),
            },
            &ctx,
        )
        .await
        .expect("second ingest with same delivery_id");

        // Two distinct on-disk aggregate files exist (CHE-0036:R1).
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "msgpack"))
            .collect();
        assert_eq!(
            entries.len(),
            2,
            "fresh-per-delivery: each ingest mints a distinct aggregate file"
        );

        // Index records the first assignment only (or_insert).
        let indexed_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(delivery_id).expect("index entry")
        };
        let loaded = store.load(indexed_id).await.expect("load indexed");
        assert_eq!(loaded.len(), 1);

        // Sequence tracker records BOTH assigned ids (each at seq 1).
        let tracker_len = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.len()
        };
        assert_eq!(tracker_len, 2, "tracker entries for both assigned ids");
    }
}
