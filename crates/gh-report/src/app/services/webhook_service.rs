//! `WebhookService` — `ApplicationService` for the
//! [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
//! aggregate (CHE-0054:R4).
//!
//! Post-Mission-H (`adr-fmt-cq7vb.11`, CHE-0069): the public
//! [`ingest`](WebhookService::ingest) method is a thin wrapper over
//! [`cherry_pit_merger::MergerHandle::dispatch`]. The
//! fresh-per-delivery create-path (`EventStore::create` → bus publish)
//! lives inside the lifted [`cherry_pit_merger::Merger`] consumed via
//! [`super::arms::WebhookArm`]. Call-site signatures stay
//! byte-identical per CHE-0054:R10.
//!
//! ## Fresh-per-delivery semantics (unchanged contract)
//!
//! Every call to `ingest` mints a **fresh `AggregateId`** via
//! `EventStore::create` inside the merger arm — there is no lazy
//! index lookup, no routing-key reuse, and (Mission H change) no
//! `deliveries_by_id` routing-index update either. See
//! [`super::arms`] module docs for the rationale (zero production
//! readers; rebuild from events impossible per the documented
//! `bootstrap_replay_state` gap). The
//! [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
//! aggregate remains a degenerate single-event terminal aggregate
//! (CHE-0054:R3); idempotency against duplicate `delivery_id`s stays
//! at the call-site (`webhook/mod.rs` `seen_deliveries` cache).

use cherry_pit_core::CorrelationContext;
use cherry_pit_merger::MergerHandle;

use super::arms::{WebhookArm, WebhookCmd};
use crate::domain::aggregates::webhook::{RecordDelivery, WebhookDelivery, WebhookError};

/// `ApplicationService` for the
/// [`WebhookDelivery`](crate::domain::aggregates::webhook::WebhookDelivery)
/// aggregate.
///
/// Holds a [`cherry_pit_merger::MergerHandle`] clone whose
/// underlying [`cherry_pit_merger::Merger`] task is the sole writer
/// to every `WebhookDelivery` stream (CHE-0069:R4 single-task
/// front-door). [`ingest`](Self::ingest) builds [`WebhookCmd::Ingest`]
/// and awaits the merger's typed [`WebhookError`] reply.
#[derive(Debug)]
pub struct WebhookService {
    handle: MergerHandle<WebhookDelivery, WebhookArm>,
}

impl WebhookService {
    /// Construct a `WebhookService` wired to the shared
    /// [`cherry_pit_merger::MergerHandle`].
    #[must_use]
    pub fn with_handle(handle: MergerHandle<WebhookDelivery, WebhookArm>) -> Self {
        Self { handle }
    }

    /// Ingest a single GitHub webhook delivery.
    ///
    /// Dispatches [`WebhookCmd::Ingest`] through the merger task.
    /// The merger arm executes the fresh-per-delivery create-path
    /// (`EventStore::create` → bus publish) atomically with respect
    /// to other merger commands.
    ///
    /// `EventBus::publish` failure remains **non-fatal** per
    /// CHE-0024:R1 — the failure logging happens inside the merger's
    /// [`shared::publish_or_trace`](cherry_pit_merger)
    /// (private; see CHE-0069:R7 for the structured-emission contract).
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError`] when the aggregate's
    /// [`HandleCommand`](cherry_pit_core::HandleCommand) impl rejects
    /// the command. The fresh-per-delivery create-path cannot reach
    /// [`WebhookError::AlreadyReceived`](crate::domain::aggregates::webhook::WebhookError::AlreadyReceived)
    /// — see module docs.
    pub async fn ingest(
        &self,
        cmd: RecordDelivery,
        ctx: &CorrelationContext,
    ) -> Result<(), WebhookError> {
        self.handle
            .dispatch(WebhookCmd::Ingest(cmd), ctx.clone())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::{Arc, Mutex};

    use cherry_pit_app::InProcessEventBus;
    use cherry_pit_core::{AggregateId, EventEnvelope, EventStore, ListableEventStore};
    use tempfile::TempDir;

    use crate::app::services::merger::MergerHandles;
    use crate::app::state::EventStoreImpl;
    use crate::domain::events::DomainEvent;

    /// Build a Mission-H-shaped [`WebhookService`] backed by three
    /// [`cherry_pit_merger::Merger`] tasks spawned via
    /// [`MergerHandles::spawn`] over a shared tempdir
    /// [`MsgpackFileStore`] + [`InProcessEventBus`] + the routing
    /// indices + sequence tracker.
    #[expect(clippy::unused_async, reason = "preserves .await callers")]
    async fn build_service() -> (
        TempDir,
        Arc<EventStoreImpl>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        WebhookService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(EventStoreImpl::create_pgno(&dir.path().join("events.pgno")).unwrap());
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let (handles, _joins) = MergerHandles::spawn(
            Arc::clone(&store),
            Arc::clone(&bus),
            runs_by_key,
            repos_by_key,
            Arc::clone(&tracker),
        );
        let svc = WebhookService::with_handle(handles.webhook);
        (dir, store, bus, tracker, svc)
    }

    #[tokio::test]
    async fn with_handle_constructs_service() {
        let (_dir, _store, _bus, _tracker, _svc) = build_service().await;
    }

    /// Mission H — single ingest produces one aggregate with one
    /// `WebhookReceived` event at sequence 1.
    ///
    /// Asserts the four observable properties for the
    /// fresh-per-delivery create-path:
    ///
    /// 1. The store holds exactly one aggregate.
    /// 2. That aggregate's stream has exactly one envelope at seq 1.
    /// 3. The bus captured exactly one envelope.
    /// 4. The sequence tracker records seq 1 for the assigned id.
    ///
    /// Mission-H change vs. pre-mission: no `deliveries_by_id`
    /// routing-index entry. The assigned id is discovered via
    /// `EventStore::list_aggregates`.
    #[tokio::test]
    async fn ingest_create_path_single_event_through_merger() {
        let (dir, store, bus, tracker, svc) = build_service().await;

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

        let aggregates = store.list_aggregates().await.expect("list_aggregates");
        assert_eq!(aggregates.len(), 1, "single ingest mints one aggregate");
        let assigned_id = aggregates[0];

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

        {
            let captured_envs = captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(captured_envs.len(), 1);
            assert_eq!(captured_envs[0].sequence().get(), 1);
        }

        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("next_seq entry")
        };
        assert_eq!(tracked_seq.get(), 1);

        let expected = dir.path().join("events.pgno");
        assert!(
            expected.exists(),
            "expected `{}` to exist after first append",
            expected.display(),
        );
        assert!(!dir.path().join("1.msgpack").exists());
    }

    /// Mission H — fresh-per-delivery semantics: two ingests with
    /// the **same** `delivery_id` mint **two distinct** aggregates
    /// (no routing-key reuse).
    ///
    /// Documents the explicit contract: idempotency against
    /// duplicate `delivery_id`s is a call-site concern
    /// (`webhook/mod.rs` `seen_deliveries` cache), NOT a service
    /// invariant. Two msgpack files exist, each with one event at
    /// sequence 1; the tracker records two ids.
    #[tokio::test]
    async fn ingest_fresh_per_delivery_does_not_dedupe_on_delivery_id_through_merger() {
        let (_dir, store, _bus, tracker, svc) = build_service().await;

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

        let aggregates = store.list_aggregates().await.expect("list_aggregates");
        assert_eq!(
            aggregates.len(),
            2,
            "fresh-per-delivery: each ingest mints a distinct aggregate"
        );

        for id in &aggregates {
            let loaded = store.load(*id).await.expect("load");
            assert_eq!(loaded.len(), 1);
        }

        let tracker_len = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.len()
        };
        assert_eq!(tracker_len, 2, "tracker entries for both assigned ids");
    }
}
