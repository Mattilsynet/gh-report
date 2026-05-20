//! `WebhookService` — `ApplicationService` for the [`WebhookDelivery`]
//! aggregate (CHE-0054:R4), rerouted through the [`Merger`] task at
//! Track 4.0/5.
//!
//! Symmetric to [`super::run_service::RunService`] at Track 4.0/3b
//! and [`super::repo_service::RepoService`] at Track 4.0/4: the
//! [`ingest`](WebhookService::ingest) method preserves its
//! pre-step-5 signature verbatim — call sites at `webhook/mod.rs`
//! did not move — but the body is now a thin wrapper that builds a
//! [`MergerCommand::IngestWebhook`] + [`oneshot::channel`] reply,
//! sends through the Merger's [`mpsc::Sender`], and awaits the
//! reply. The fresh-per-delivery create-path
//! (`EventStore::create` → routing index entry → `next_seq`
//! entry → bus publish) lives inside the Merger arm now — see
//! [`super::merger`] module docs.
//!
//! ## Generic-port discipline (Track 4.0/5 departure)
//!
//! Pre-step-5 the service was generic over `S: EventStore<Event =
//! DomainEvent>` + `B: EventBus<Event = DomainEvent>` per
//! CHE-0005:R1. Post-step-5 the service holds only a
//! [`mpsc::Sender<MergerCommand>`] and no longer touches either port
//! directly, so the generics are dropped (Option A — symmetric to
//! `RunService` at 3b and `RepoService` at step 4). The [`Merger`] binds
//! the concrete types at the composition root.
//!
//! ## Fresh-per-delivery semantics (unchanged contract)
//!
//! Every call to `ingest` mints a **fresh `AggregateId`** via
//! `EventStore::create` inside the Merger arm — there is no lazy
//! index lookup, no routing-key reuse. The `delivery_id` is recorded
//! into the routing index (and the assigned id into the
//! `next_seq`) for symmetry with the other services and so any
//! future cache-based dedup can read this surface, but **routing
//! does not gate persistence**: `WebhookDelivery` is a degenerate
//! single-event terminal aggregate (CHE-0054:R3) and idempotency
//! against duplicate `delivery_id`s stays at the call-site
//! (`webhook/mod.rs` `seen_deliveries` cache).
//!
//! [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
//! [`Merger`]: super::merger::Merger
//! [`MergerCommand`]: super::merger::MergerCommand
//! [`MergerCommand::IngestWebhook`]: super::merger::MergerCommand::IngestWebhook
//! [`mpsc::Sender`]: tokio::sync::mpsc::Sender
//! [`oneshot::channel`]: tokio::sync::oneshot::channel

use cherry_pit_core::CorrelationContext;
use tokio::sync::{mpsc, oneshot};

use super::merger::MergerCommand;
use crate::domain::aggregates::webhook::{RecordDelivery, WebhookError};

/// `ApplicationService` for the [`WebhookDelivery`] aggregate.
///
/// Post-step-5 channel handle: a thin wrapper over the [`Merger`]
/// task's [`mpsc::Sender`]. [`ingest`](Self::ingest) builds
/// [`MergerCommand::IngestWebhook`] with a [`oneshot::Sender`]
/// reply, sends it through the merger queue, and awaits the typed
/// `Result<(), WebhookError>` response.
///
/// ## SMI invariant carry (Track 4.0/5)
///
/// Routing the `ingest` write path through `merger_tx` promotes the
/// *sole-writer* invariant from latent to **structurally enforced**
/// for the [`WebhookDelivery`] aggregate. `RunService` closed the
/// analogous gap for the [`Run`] aggregate at Track 4.0/3b and
/// `RepoService` for the [`Repo`] aggregate at Track 4.0/4; with this
/// step every successful append to any of the three aggregates
/// flows through the single Merger task and the TOCTOU window
/// between aggregate load and event-stream append is closed by
/// construction. The remaining work (step 6) is the deletion of the
/// `*Concrete` alias shims and any vestigial composition-root
/// scaffolding.
///
/// [`Run`]: crate::domain::aggregates::run::Run
/// [`Repo`]: crate::domain::aggregates::repo::Repo
/// [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
/// [`Merger`]: super::merger::Merger
/// [`mpsc::Sender`]: tokio::sync::mpsc::Sender
#[derive(Debug)]
pub struct WebhookService {
    /// Producer end of the Merger command channel
    /// (`adr-fmt-nnn3` — Track 4.0/5).
    merger_tx: mpsc::Sender<MergerCommand>,
}

impl WebhookService {
    /// Construct a `WebhookService` wired to the [`Merger`] command
    /// channel.
    ///
    /// The supplied `merger_tx` is shared with [`AppState`] and the
    /// two other `ApplicationService` surfaces — [`RunService`]
    /// (rerouted at 3b) and [`RepoService`] (rerouted at step 4).
    /// Cloning the [`mpsc::Sender`] is cheap (refcount bump) and
    /// keeps the channel open for the process lifetime of
    /// [`AppState`].
    ///
    /// [`AppState`]: crate::app::state::AppState
    /// [`Merger`]: super::merger::Merger
    /// [`RunService`]: super::run_service::RunService
    /// [`RepoService`]: super::repo_service::RepoService
    /// [`mpsc::Sender`]: tokio::sync::mpsc::Sender
    #[must_use]
    pub fn with_merger_tx(merger_tx: mpsc::Sender<MergerCommand>) -> Self {
        Self { merger_tx }
    }

    /// Ingest a single GitHub webhook delivery.
    ///
    /// Routes [`MergerCommand::IngestWebhook`] through the Merger
    /// task. The Merger arm executes the fresh-per-delivery
    /// create-path (`EventStore::create` → routing index entry →
    /// `next_seq` entry → bus publish) atomically with respect
    /// to other Merger commands; the SMI sole-writer invariant for
    /// the [`WebhookDelivery`] aggregate is enforced by the Merger
    /// task being the only holder of the store write handle.
    ///
    /// `EventBus::publish` failure remains **non-fatal** per
    /// CHE-0024:R1 — semantics preserved from the pre-step-5 service
    /// body (the failure logging happens inside the Merger arm via
    /// `shared::publish_or_trace`).
    ///
    /// [`WebhookDelivery`]: crate::domain::aggregates::webhook::WebhookDelivery
    ///
    /// # Errors
    ///
    /// Returns [`WebhookError`] when the aggregate's
    /// [`HandleCommand`](cherry_pit_core::HandleCommand) impl
    /// rejects the command. The fresh-per-delivery create-path of
    /// Track 4.0 cannot reach
    /// [`WebhookError::AlreadyReceived`](crate::domain::aggregates::webhook::WebhookError::AlreadyReceived)
    /// — see module docs.
    ///
    /// # Panics
    ///
    /// Panics if the Merger task has shut down before the reply
    /// arrives — this can only happen at process teardown after
    /// [`AppState`] has been dropped, so a panic on the receiver
    /// surfaces a misuse-after-shutdown bug rather than a recoverable
    /// runtime condition.
    ///
    /// [`AppState`]: crate::app::state::AppState
    pub async fn ingest(
        &self,
        cmd: RecordDelivery,
        ctx: &CorrelationContext,
    ) -> Result<(), WebhookError> {
        let (reply, rx) = oneshot::channel();
        self.merger_tx
            .send(MergerCommand::IngestWebhook {
                cmd,
                ctx: ctx.clone(),
                reply,
            })
            .await
            .expect("Merger task alive for AppState lifetime");
        rx.await.expect("Merger arm always replies before drop")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::{Arc, Mutex};

    use cherry_pit_agent::InProcessEventBus;
    use cherry_pit_core::{AggregateId, EventEnvelope, EventStore, ListableEventStore};
    use tempfile::TempDir;

    use crate::app::services::merger::Merger;
    use crate::app::state::EventStoreImpl;
    use crate::domain::events::DomainEvent;
    use pardosa_eventstore::PardosaLogEventStore;

    /// Build a Track 4.0/5-shaped `WebhookService` backed by a
    /// [`Merger`] task spawned over a shared tempdir
    /// [`PardosaLogEventStore`] + [`InProcessEventBus`] + the three
    /// routing indices + sequence tracker. Symmetric to the
    /// `RunService` 3b and `RepoService` step-4 test harnesses.
    ///
    /// Returns the tempdir (kept alive for the test — drop releases
    /// the CHE-0043:R1 flock on `{dir}/.lock`), the durable handles
    /// for direct inspection, and the [`WebhookService`] under test.
    /// The Merger
    /// [`tokio::task::JoinHandle`] is intentionally dropped — the
    /// task is kept alive by the [`mpsc::Sender`] inside the service.
    async fn build_service() -> (
        TempDir,
        Arc<EventStoreImpl>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<String, AggregateId>>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        WebhookService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            PardosaLogEventStore::<DomainEvent>::open(dir.path())
                .await
                .expect("open test event store"),
        );
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let runs_by_key = Arc::new(Mutex::new(HashMap::new()));
        let repos_by_key = Arc::new(Mutex::new(HashMap::new()));
        let deliveries_by_id = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let (merger_tx, _merger_handle) = Merger::spawn(
            Arc::clone(&store),
            Arc::clone(&bus),
            runs_by_key,
            repos_by_key,
            Arc::clone(&deliveries_by_id),
            Arc::clone(&tracker),
        );
        let svc = WebhookService::with_merger_tx(merger_tx);
        (dir, store, bus, deliveries_by_id, tracker, svc)
    }

    #[tokio::test]
    async fn with_merger_tx_constructs_service() {
        let (_dir, _store, _bus, _index, _tracker, _svc) = build_service().await;
    }

    /// Step-5 — single ingest produces one aggregate with one
    /// `WebhookReceived` event at sequence 1.
    ///
    /// Asserts the same five properties the pre-step-5 ingest
    /// create-path test asserted (stream contents + bus capture +
    /// routing index populated + sequence tracker advance + single
    /// per-aggregate file) — proving the channel-reroute is
    /// observably equivalent at the `EventStore` / `EventBus` boundary
    /// for the `WebhookDelivery` aggregate.
    ///
    /// Contract enforcer for SMI invariants 1 (sole-writer: the
    /// Merger is the only writer to `WebhookDelivery` streams) and 5
    /// (post-append publish: every appended envelope arrives on the
    /// bus before the reply resolves) for the `WebhookDelivery`
    /// aggregate.
    #[tokio::test]
    async fn ingest_create_path_single_event_through_merger() {
        let (dir, store, bus, index, tracker, svc) = build_service().await;

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

        // Routing index resolves.
        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(delivery_id)
                .expect("index should map delivery_id")
        };

        // Stream contents — single envelope at sequence 1.
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

        // Bus capture.
        {
            let captured_envs = captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(captured_envs.len(), 1);
            assert_eq!(captured_envs[0].sequence().get(), 1);
        }

        // Sequence tracker == 1.
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("next_seq entry")
        };
        assert_eq!(tracked_seq.get(), 1);

        // CHE-0036:R1 — exactly one `<assigned_id>.log` file exists
        // under `dir` after the first append.
        let expected = dir.path().join(format!("{}.log", assigned_id.get()));
        assert!(
            expected.exists(),
            "expected `{}` to exist after first append",
            expected.display(),
        );
    }

    /// Step-5 — fresh-per-delivery semantics: two ingests with the
    /// **same** `delivery_id` mint **two distinct** aggregates (no
    /// routing-key reuse).
    ///
    /// Documents the explicit Track-4.0 contract: idempotency
    /// against duplicate `delivery_id`s is a call-site concern
    /// (`webhook/mod.rs` `seen_deliveries` cache), NOT a service
    /// invariant. Two pardosa files exist, each with one event at
    /// sequence 1. The index records the **first** assignment
    /// (`or_insert` semantics inside the Merger arm) and the
    /// `next_seq` records both assigned ids.
    #[tokio::test]
    async fn ingest_fresh_per_delivery_does_not_dedupe_on_delivery_id_through_merger() {
        let (_dir, store, _bus, index, tracker, svc) = build_service().await;

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

        // Two distinct aggregates exist (CHE-0036:R1): each ingest mints a
        // distinct aggregate file under `dir`.
        let aggregates = store.list_aggregates().expect("list_aggregates");
        assert_eq!(
            aggregates.len(),
            2,
            "fresh-per-delivery: each ingest mints a distinct aggregate"
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
