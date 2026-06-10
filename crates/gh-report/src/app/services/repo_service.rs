//! `RepoService` — `ApplicationService` for the [`Repo`] aggregate
//! (CHE-0054:R4).
//!
//! Post-Mission-H (`adr-fmt-cq7vb.11`, CHE-0069): the two public
//! methods are thin wrappers over
//! [`cherry_pit_merger::MergerHandle::dispatch`]. The triad —
//! load → handle → create-or-append → publish — lives inside the
//! lifted [`cherry_pit_merger::Merger`] consumed via
//! [`super::arms::RepoArm`]. Call-site signatures stay byte-identical
//! per CHE-0054:R10.
//!
//! [`Repo`]: crate::domain::aggregates::repo::Repo

use cherry_pit_core::CorrelationContext;
use cherry_pit_merger::MergerHandle;

use super::arms::{RepoArm, RepoCmd};
use crate::domain::aggregates::repo::{RecordEvaluation, RecordRemoval, Repo, RepoError};

/// `ApplicationService` for the [`Repo`] aggregate.
///
/// Holds a [`cherry_pit_merger::MergerHandle`] clone whose
/// underlying [`cherry_pit_merger::Merger`] task is the sole writer
/// to every `Repo` aggregate stream (CHE-0069:R4 single-task
/// front-door). Each public method translates its arguments into the
/// matching [`RepoCmd`] variant and awaits the merger's typed
/// [`RepoError`] reply.
///
/// [`Repo`]: crate::domain::aggregates::repo::Repo
#[derive(Debug)]
pub struct RepoService {
    handle: MergerHandle<Repo, RepoArm>,
}

impl RepoService {
    /// Construct a `RepoService` wired to the shared
    /// [`cherry_pit_merger::MergerHandle`].
    #[must_use]
    pub fn with_handle(handle: MergerHandle<Repo, RepoArm>) -> Self {
        Self { handle }
    }

    /// Record a repository evaluation.
    ///
    /// `domain_key` is the routing key — the merger arm uses it via
    /// [`PersistMode::CreateOrAppend`](cherry_pit_merger::PersistMode::CreateOrAppend)
    /// to resolve the `AggregateId` from the index (CHE-0054:R5),
    /// **lazily creating** a fresh aggregate the first time the key
    /// is seen. The command's own `domain_key` field is treated
    /// strictly as event-payload data; routing is the service's
    /// responsibility, separate from the command shape.
    ///
    /// # Errors
    ///
    /// - [`RepoError::AlreadyRemoved`] (CHE-0054:R2.c) when the
    ///   resolved aggregate is in the terminal `Removed` phase.
    /// - [`RepoError::Storage`] on
    ///   [`StoreError`](cherry_pit_core::StoreError) lifted via the
    ///   arm's `From<StoreError>` impl.
    pub async fn record_evaluation(
        &self,
        domain_key: &str,
        cmd: RecordEvaluation,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        self.handle
            .dispatch(
                RepoCmd::Evaluate {
                    domain_key: domain_key.to_owned(),
                    cmd: Box::new(cmd),
                },
                ctx.clone(),
            )
            .await
    }

    /// Record a repository removal (terminal).
    ///
    /// `domain_key` is the routing key — see
    /// [`record_evaluation`](Self::record_evaluation). A
    /// `RecordRemoval` for a never-evaluated repo lazily creates the
    /// aggregate (allowed per CHE-0054:R2 — webhook-driven removal
    /// can arrive without prior local evaluation).
    ///
    /// # Errors
    ///
    /// - [`RepoError::AlreadyRemoved`] when already in `Removed`.
    /// - [`RepoError::Storage`] on
    ///   [`StoreError`](cherry_pit_core::StoreError) lifted via the
    ///   arm's `From<StoreError>` impl.
    pub async fn record_removal(
        &self,
        domain_key: &str,
        cmd: RecordRemoval,
        ctx: &CorrelationContext,
    ) -> Result<(), RepoError> {
        self.handle
            .dispatch(
                RepoCmd::Remove {
                    domain_key: domain_key.to_owned(),
                    cmd,
                },
                ctx.clone(),
            )
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
    use cherry_pit_core::{AggregateId, EventEnvelope, EventStore};
    use tempfile::TempDir;

    use crate::app::services::merger::MergerHandles;
    use crate::app::state::EventStoreImpl;
    use crate::domain::events::DomainEvent;

    /// Build a Mission-H-shaped [`RepoService`] backed by three
    /// [`cherry_pit_merger::Merger`] tasks spawned via
    /// [`MergerHandles::spawn`] over a shared tempdir
    /// [`MsgpackFileStore`] + [`InProcessEventBus`] + the routing
    /// indices + sequence tracker.
    #[expect(clippy::unused_async, reason = "preserves .await callers")]
    async fn build_service() -> (
        TempDir,
        Arc<EventStoreImpl>,
        Arc<InProcessEventBus<DomainEvent>>,
        Arc<Mutex<HashMap<String, AggregateId>>>,
        Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        RepoService,
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
            Arc::clone(&repos_by_key),
            Arc::clone(&tracker),
        );
        let svc = RepoService::with_handle(handles.repo);
        (dir, store, bus, repos_by_key, tracker, svc)
    }

    #[tokio::test]
    async fn with_handle_constructs_service() {
        let (_dir, _store, _bus, _index, _tracker, _svc) = build_service().await;
    }

    /// Mission H — full Repo lifecycle exercising both service
    /// surfaces routed through the lifted merger:
    /// `evaluate (create) → evaluate (append) → remove (append, terminal)`.
    #[tokio::test]
    async fn repo_lifecycle_lazy_creates_then_appends_then_terminates_through_merger() {
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
        let domain_key = "octocat/hello";

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

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(domain_key).expect("index should map domain_key")
        };

        let loaded = store.load(assigned_id).await.expect("load");
        assert_lifecycle_stream(&loaded);

        assert_captured_sequence(&captured, 3);

        assert_tracker_seq(&tracker, assigned_id, 3);

        assert_pardosa_pgno_file(&dir);

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

    fn assert_lifecycle_stream(loaded: &[EventEnvelope<DomainEvent>]) {
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
    }

    fn assert_captured_sequence(
        captured: &Arc<Mutex<Vec<EventEnvelope<DomainEvent>>>>,
        expected_len: usize,
    ) {
        let envs = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(envs.len(), expected_len);
        for (i, env) in envs.iter().enumerate() {
            assert_eq!(env.sequence().get(), u64::try_from(i + 1).unwrap());
        }
    }

    fn assert_tracker_seq(
        tracker: &Arc<Mutex<HashMap<AggregateId, NonZeroU64>>>,
        id: AggregateId,
        expected: u64,
    ) {
        let guard = tracker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let seq = *guard.get(&id).expect("tracker entry");
        assert_eq!(seq.get(), expected);
    }

    fn assert_pardosa_pgno_file(dir: &TempDir) {
        let expected = dir.path().join("events.pgno");
        assert!(
            expected.exists(),
            "expected `{}` to exist under {}",
            expected.display(),
            dir.path().display(),
        );
        assert!(!dir.path().join("1.msgpack").exists());
    }

    /// Mission H — covers the lazy-create branch on `record_removal`:
    /// webhook-driven removal arrives for a never-evaluated repo
    /// (allowed per CHE-0054:R2 — no pre-evaluation precondition).
    #[tokio::test]
    async fn repo_removal_lazy_creates_when_never_evaluated_through_merger() {
        let (_dir, store, _bus, index, tracker, svc) = build_service().await;

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

    /// **The I1 TOCTOU regression pin (consumer-side equivalence).**
    /// Fans out `N = 32` concurrent `record_evaluation` calls
    /// against the **same** `domain_key`, then asserts:
    ///
    /// 1. Exactly one routing-index entry materialises.
    /// 2. Exactly one per-aggregate `.msgpack` file lands.
    /// 3. The single stream contains exactly `N` envelopes with
    ///    monotonic sequences `1..=N`.
    /// 4. The next-sequence tracker records `N`.
    ///
    /// Serialization is provided by the lifted
    /// [`cherry_pit_merger::Merger`] single-task front door per
    /// CHE-0069:R4. The primitive's own
    /// `tests/i1_toctou_pin.rs` proptest covers the same property
    /// against an in-memory store with up to 48 concurrent
    /// dispatches; this test pins the gh-report
    /// `RepoService → MergerHandle` consumer wiring against the
    /// durable [`MsgpackFileStore`].
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_domain_key_evaluations_create_exactly_one_aggregate() {
        const N: usize = 32;

        let (dir, store, _bus, index, tracker, svc) = build_service().await;

        let svc = Arc::new(svc);
        let ctx = CorrelationContext::none();
        let domain_key = "octocat/concurrent-create";

        let mut handles = Vec::with_capacity(N);
        for i in 0..N {
            let svc = Arc::clone(&svc);
            let ctx = ctx.clone();
            let dk = domain_key.to_owned();
            handles.push(tokio::spawn(async move {
                svc.record_evaluation(
                    &dk,
                    RecordEvaluation {
                        domain_key: dk.clone(),
                        repo_name: "concurrent-create".into(),
                        success: i % 2 == 0,
                        source: "scheduled_batch".into(),
                        duration_ms: u64::try_from(i).unwrap(),
                        timestamp: "2026-05-31T00:00:00Z".into(),
                        evidence: None,
                    },
                    &ctx,
                )
                .await
            }));
        }
        for h in handles {
            h.await
                .expect("task join")
                .expect("record_evaluation under contention");
        }

        let assigned_id = {
            let guard = index
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(
                guard.len(),
                1,
                "routing index must have exactly one entry for the single domain_key, got {}",
                guard.len()
            );
            *guard.get(domain_key).expect("index should map domain_key")
        };

        let pgno_files: Vec<std::path::PathBuf> = std::fs::read_dir(dir.path())
            .expect("read tempdir")
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|ext| ext == "pgno")
            })
            .collect();
        assert_eq!(
            pgno_files.len(),
            1,
            "expected exactly one pardosa event-log file, found {pgno_files:?}"
        );

        let loaded = store.load(assigned_id).await.expect("load");
        assert_eq!(
            loaded.len(),
            N,
            "stream should contain exactly N envelopes (1 create + N-1 appends)"
        );
        for (i, env) in loaded.iter().enumerate() {
            assert_eq!(
                env.sequence().get(),
                u64::try_from(i + 1).unwrap(),
                "envelope {i} should have monotonic sequence {}",
                i + 1
            );
        }

        let tracked_seq = {
            let guard = tracker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard.get(&assigned_id).expect("tracker entry")
        };
        assert_eq!(
            tracked_seq.get(),
            u64::try_from(N).unwrap(),
            "tracker should reflect the last appended sequence"
        );
    }
}
