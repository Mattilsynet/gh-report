//! `RepoService` — ApplicationService for the [`Repo`] aggregate
//! (CHE-0054:R4).
//!
//! Owns the load → handle → append → publish triad for
//! repository-evaluation use cases. Resolves identity via the shared
//! `Arc<Mutex<HashMap<String, AggregateId>>>` index handle (CHE-0054:R5;
//! key is the `domain_key` carried on
//! [`RepoEvaluated`](crate::domain::events::DomainEvent::RepoEvaluated)
//! / [`RepoRemoved`](crate::domain::events::DomainEvent::RepoRemoved)).
//!
//! ## Method body status
//!
//! Both `RepoService` methods are wired (Inc B7'b-4). The 14
//! production publish sites migrate to these calls in **B7'c**.
//!
//! [`Repo`]: crate::domain::aggregates::repo::Repo

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use cherry_pit_core::{AggregateId, CorrelationContext, EventBus, EventStore};

use crate::domain::aggregates::repo::{RecordEvaluation, RecordRemoval, RepoError};
use crate::domain::events::DomainEvent;

/// ApplicationService for the [`Repo`] aggregate.
///
/// Generic over the concrete [`EventStore`] and [`EventBus`] per
/// CHE-0005:R1 — see [`RunService`](super::run_service::RunService)
/// docs for the routing/CAS rationale.
///
/// [`Repo`]: crate::domain::aggregates::repo::Repo
#[derive(Debug)]
pub struct RepoService<S, B>
where
    S: EventStore<Event = DomainEvent>,
    B: EventBus<Event = DomainEvent>,
{
    /// Durable per-aggregate event store.
    store: Arc<S>,
    /// Synchronous in-process event bus.
    bus: Arc<B>,
    /// `domain_key` → `AggregateId` routing index (CHE-0054:R5).
    index: Arc<Mutex<HashMap<String, AggregateId>>>,
    /// Last-applied sequence per aggregate (CHE-0054:R6).
    sequence_tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
}

impl<S, B> RepoService<S, B>
where
    S: EventStore<Event = DomainEvent>,
    B: EventBus<Event = DomainEvent>,
{
    /// Construct a `RepoService` wired to the given store, bus, and
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

    /// Record a repository evaluation.
    ///
    /// `domain_key` is the routing key — the service uses it to
    /// resolve the `AggregateId` from the index (CHE-0054:R5),
    /// **lazily creating** a fresh aggregate the first time the key
    /// is seen. The command's own `domain_key` field is treated
    /// strictly as event-payload data; routing is the
    /// ApplicationService's responsibility, separate from the
    /// command shape (mirrors the `batch_id` pattern on
    /// [`RunService`](super::run_service::RunService)).
    ///
    /// # Errors
    ///
    /// Returns [`RepoError::AlreadyRemoved`] when the resolved
    /// aggregate is in the terminal `Removed` phase (invariant c).
    pub async fn record_evaluation(
        &self,
        domain_key: &str,
        cmd: RecordEvaluation,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        use cherry_pit_core::{Aggregate, HandleCommand};

        let existing_id = super::shared::lookup(&self.index, domain_key);
        let (envelopes, last_seq) =
            super::shared::load_envelopes_or_empty(&self.store, existing_id).await;
        let mut state = crate::domain::aggregates::repo::Repo::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let new_events = state.handle(cmd)?;
        let new_envelopes = super::shared::create_or_append(
            super::shared::PersistHandles {
                store: &self.store,
                index: &self.index,
                sequence_tracker: &self.sequence_tracker,
            },
            domain_key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "RepoEvaluated").await;
        Ok(())
    }

    /// Record a repository removal (terminal).
    ///
    /// `domain_key` is the routing key — see
    /// [`record_evaluation`](Self::record_evaluation). A `RecordRemoval`
    /// for a never-evaluated repo lazily creates the aggregate (allowed
    /// per CHE-0054:R2 — webhook-driven removal can arrive without
    /// prior local evaluation).
    ///
    /// # Errors
    ///
    /// Returns [`RepoError::AlreadyRemoved`] when the resolved
    /// aggregate is already in the `Removed` phase.
    pub async fn record_removal(
        &self,
        domain_key: &str,
        cmd: RecordRemoval,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        use cherry_pit_core::{Aggregate, HandleCommand};

        let existing_id = super::shared::lookup(&self.index, domain_key);
        let (envelopes, last_seq) =
            super::shared::load_envelopes_or_empty(&self.store, existing_id).await;
        let mut state = crate::domain::aggregates::repo::Repo::default();
        for env in &envelopes {
            state.apply(env.payload());
        }
        let new_events = state.handle(cmd)?;
        let new_envelopes = super::shared::create_or_append(
            super::shared::PersistHandles {
                store: &self.store,
                index: &self.index,
                sequence_tracker: &self.sequence_tracker,
            },
            domain_key,
            existing_id,
            last_seq,
            new_events,
            ctx,
        )
        .await;
        super::shared::publish_or_trace(&self.bus, &new_envelopes, "RepoRemoved").await;
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
        RepoService<MsgpackFileStore<DomainEvent>, InProcessEventBus<DomainEvent>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::<DomainEvent>::new(dir.path()));
        let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
        let index = Arc::new(Mutex::new(HashMap::new()));
        let tracker = Arc::new(Mutex::new(HashMap::new()));
        let svc = RepoService::with_stores(
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

    /// Inc 4 (B7'b-4) — Repo lifecycle exercising lazy-create +
    /// append-on-subsequent on a single domain_key:
    /// `evaluate (create) → evaluate (append) → remove (append, terminal)`.
    ///
    /// Asserts:
    ///   1. Stream contains 3 envelopes at sequences 1..=3 with
    ///      payload variants in order
    ///      (RepoEvaluated, RepoEvaluated, RepoRemoved).
    ///   2. Bus subscriber captured all 3 in order.
    ///   3. Routing index populated: domain_key → AggregateId.
    ///   4. Sequence tracker == NonZeroU64(3).
    ///   5. Single per-aggregate file (CHE-0036:R1).
    ///   6. Subsequent record_evaluation rejects with
    ///      RepoError::AlreadyRemoved (CHE-0054:R2.c terminal).
    #[tokio::test]
    async fn repo_lifecycle_lazy_creates_then_appends_then_terminates() {
        use crate::domain::aggregates::repo::RepoError;

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
        let domain_key = "octocat/hello";

        // 1. evaluate (create-path on first reference).
        svc.record_evaluation(
            domain_key,
            RecordEvaluation {
                domain_key: domain_key.into(),
                repo_name: "hello".into(),
                success: true,
                source: "scheduled_batch".into(),
                duration_ms: 100,
                timestamp: "2026-05-10T12:00:00Z".into(),
                evidence: None,
            },
            &ctx,
        )
        .await
        .expect("first record_evaluation");

        // 2. evaluate (append-path on second reference).
        svc.record_evaluation(
            domain_key,
            RecordEvaluation {
                domain_key: domain_key.into(),
                repo_name: "hello".into(),
                success: false,
                source: "scheduled_batch".into(),
                duration_ms: 80,
                timestamp: "2026-05-10T12:01:00Z".into(),
                evidence: None,
            },
            &ctx,
        )
        .await
        .expect("second record_evaluation");

        // 3. remove (append-path, terminal).
        svc.record_removal(
            domain_key,
            RecordRemoval {
                domain_key: domain_key.into(),
                repo_name: "hello".into(),
                timestamp: "2026-05-10T12:02:00Z".into(),
            },
            &ctx,
        )
        .await
        .expect("record_removal");

        // (3) Routing index resolves.
        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(domain_key).expect("index should map domain_key")
        };

        // (1) Stream contents.
        let loaded = store.load(assigned_id).await.expect("load");
        assert_eq!(loaded.len(), 3, "3 envelopes after lifecycle");
        for (i, env) in loaded.iter().enumerate() {
            assert_eq!(
                env.sequence().get(),
                u64::try_from(i + 1).unwrap(),
                "envelope {i} should have sequence {}",
                i + 1
            );
        }
        assert!(matches!(
            loaded[0].payload(),
            DomainEvent::RepoEvaluated { success: true, .. }
        ));
        assert!(matches!(
            loaded[1].payload(),
            DomainEvent::RepoEvaluated { success: false, .. }
        ));
        assert!(matches!(
            loaded[2].payload(),
            DomainEvent::RepoRemoved { .. }
        ));

        // (2) Bus capture.
        {
            let captured_envs = captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(captured_envs.len(), 3);
            for (i, env) in captured_envs.iter().enumerate() {
                assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
            }
        }

        // (4) Sequence tracker == 3.
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("sequence_tracker entry")
        };
        assert_eq!(tracked_seq.get(), 3);

        // (5) Single per-aggregate file (CHE-0036:R1).
        let store_file = dir.path().join(format!("{assigned_id}.msgpack"));
        assert!(store_file.exists());
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "msgpack"))
            .collect();
        assert_eq!(entries.len(), 1);

        // (6) Post-removal rejection (CHE-0054:R2.c).
        let err = svc
            .record_evaluation(
                domain_key,
                RecordEvaluation {
                    domain_key: domain_key.into(),
                    repo_name: "hello".into(),
                    success: true,
                    source: "scheduled_batch".into(),
                    duration_ms: 1,
                    timestamp: "2026-05-10T12:03:00Z".into(),
                    evidence: None,
                },
                &ctx,
            )
            .await
            .expect_err("evaluate after remove must reject");
        assert_eq!(err, RepoError::AlreadyRemoved);
    }

    /// Inc 4 — covers the lazy-create branch on `record_removal`:
    /// webhook-driven removal arrives for a never-evaluated repo
    /// (allowed per CHE-0054:R2 — no pre-evaluation precondition).
    #[tokio::test]
    async fn repo_removal_lazy_creates_when_never_evaluated() {
        let (_dir, store, _bus, index, tracker, svc) = build_service();

        let ctx = CorrelationContext::none();
        let domain_key = "ghost/never-seen";

        svc.record_removal(
            domain_key,
            RecordRemoval {
                domain_key: domain_key.into(),
                repo_name: "never-seen".into(),
                timestamp: "2026-05-10T12:00:00Z".into(),
            },
            &ctx,
        )
        .await
        .expect("lazy create-on-removal");

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard
                .get(domain_key)
                .expect("index entry created lazily on removal")
        };
        let loaded = store.load(assigned_id).await.expect("load");
        assert_eq!(loaded.len(), 1);
        assert!(matches!(
            loaded[0].payload(),
            DomainEvent::RepoRemoved { .. }
        ));
        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("tracker entry")
        };
        assert_eq!(tracked_seq.get(), 1);
    }
}
