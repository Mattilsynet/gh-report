use gh_report::app::state::{AppState, EventStoreImpl, OrgEventStoreImpl};
use gh_report::config::runtime::{DEFAULT_NATS_URL, NatsStoreConfig, PardosaBackend};
use gh_report::domain::auth::{AuthMode, TokenTier};
use gh_report::domain::evidence::OrgStateSnapshot;
use gh_report::domain::metrics::{OrgAlertSummary, RepoAlertSummary};
use gh_report::domain::status::CollectionStatus;
use gh_report::event::DomainEvent;
use pardosa::store::JetStreamBackend as PardosaJetStreamBackend;
use pardosa_nats::test_support::LiveNatsServer;
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use pardosa_schema::{NonEmptyEventString, Timestamp as EventTimestamp};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

struct StreamCleanup {
    nats_url: String,
    stream_name: String,
}

impl Drop for StreamCleanup {
    fn drop(&mut self) {
        let Ok(rt) = Runtime::new() else {
            return;
        };
        let nats_url = self.nats_url.clone();
        let stream_name = self.stream_name.clone();
        rt.block_on(async move {
            let Ok(client) = async_nats::connect(nats_url).await else {
                return;
            };
            let js = async_nats::jetstream::new(client);
            let _ = js.delete_stream(stream_name).await;
        });
    }
}

fn live_nats_url() -> String {
    std::env::var("GH_REPORT_LIVE_NATS_URL").unwrap_or_else(|_| DEFAULT_NATS_URL.to_string())
}

fn unique_org() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("m5-live-{pid}-{nanos}")
}

fn jetstream_backend(
    nats: NatsStoreConfig,
    handle: tokio::runtime::Handle,
) -> PardosaJetStreamBackend {
    let cfg = JetStreamConfig::builder()
        .stream_name(nats.stream_name)
        .subject(nats.subject)
        .durable_consumer(nats.durable_consumer)
        .nats_url(nats.nats_url)
        .runtime_handle(RuntimeHandle::from_tokio(handle))
        .build()
        .expect("config valid");
    let substrate = SubstrateJetStreamBackend::open(cfg);
    PardosaJetStreamBackend::open(substrate)
}

async fn open_state(events_dir: &Path, nats: NatsStoreConfig) -> Arc<AppState> {
    AppState::with_stores(events_dir, PardosaBackend::Nats, nats)
        .await
        .expect("AppState::with_stores with Nats backend")
}

fn test_event() -> DomainEvent {
    DomainEvent::RepositoryStateCaptured {
        domain_key: NonEmptyEventString::try_new("m5/repo").expect("key"),
        repo_name: NonEmptyEventString::try_new("repo").expect("repo"),
        timestamp: EventTimestamp::from_nanos(1_781_136_000_000_000_000).expect("timestamp"),
        evidence: None,
    }
}

fn assert_loaded_event(event: &DomainEvent) {
    match event {
        DomainEvent::RepositoryStateCaptured {
            domain_key,
            repo_name,
            ..
        } => {
            assert_eq!(domain_key.as_str(), "m5/repo");
            assert_eq!(repo_name.as_str(), "repo");
        }
    }
}

fn org_snapshot() -> OrgStateSnapshot {
    OrgStateSnapshot {
        archived_repos: 11,
        assessment_metadata: gh_report::domain::evidence::AssessmentMetadata {
            date: "2026-06-14".to_string(),
            organization: "TestOrg".to_string(),
            schema_version: gh_report::config::EVIDENCE_SCHEMA_VERSION.to_string(),
            run_timestamp: "2026-06-14T00:00:00+00:00".to_string(),
            run_id: "live-nats-org-run".to_string(),
            token_tier: TokenTier::Full,
            token_scopes: "repo, read:org".to_string(),
            auth_mode: AuthMode::Pat,
            rate_limit_warnings: 0,
            unavailable_capabilities: vec![],
            inventory_fetched_at: None,
            warm_start: false,
        },
        alert_summary: OrgAlertSummary {
            collection_status: CollectionStatus::Success,
            collection_reason: None,
            per_repo: HashMap::<String, RepoAlertSummary>::new(),
            open_secret_alert_age_buckets: HashMap::new(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        },
    }
}

#[test]
#[ignore = "requires live nats-server at GH_REPORT_LIVE_NATS_URL or nats://localhost:4222"]
fn nats_backend_fresh_create_reopen_and_populated_route_preserves_events() {
    let rt = Runtime::new().expect("tokio runtime");
    let nats_url = live_nats_url();
    let org = unique_org();
    let nats = NatsStoreConfig::for_org(&org, nats_url.clone()).expect("nats config");
    let _cleanup = StreamCleanup {
        nats_url,
        stream_name: nats.stream_name.clone(),
    };

    rt.block_on(async {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        let fresh_state = open_state(&events_dir, nats.clone()).await;
        let fresh_store = Arc::clone(&fresh_state.event_store);
        assert!(
            fresh_store.latest_per_repo().expect("latest").is_empty(),
            "fresh Nats route must create an empty store"
        );
        fresh_store.record("m5/repo", test_event()).expect("record");
        drop(fresh_store);
        drop(fresh_state);

        let opened_via_adapter = tokio::task::spawn_blocking({
            let nats = nats.clone();
            let handle = tokio::runtime::Handle::current();
            move || EventStoreImpl::open_jetstream(jetstream_backend(nats, handle))
        })
        .await
        .expect("open_jetstream join")
        .expect("open_jetstream after write");
        let opened_latest = opened_via_adapter
            .latest_per_repo()
            .expect("latest indexed");
        assert_eq!(opened_latest.len(), 1);
        assert_loaded_event(&opened_latest[0].1);
        drop(opened_via_adapter);

        let populated_state = open_state(&events_dir, nats.clone()).await;
        let loaded = populated_state
            .event_store
            .latest_per_repo()
            .expect("load after populated reopen");
        assert_eq!(loaded.len(), 1);
        assert_loaded_event(&loaded[0].1);
    });
}

#[test]
fn nats_backend_uses_distinct_repo_and_org_streams() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let org = unique_org();
    let nats = NatsStoreConfig::for_org(&org, server.url().to_owned()).expect("nats config");
    let org_nats = nats.org_events();
    let _repo_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.stream_name.clone(),
    };
    let _org_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: org_nats.stream_name.clone(),
    };

    assert_ne!(nats.stream_name, org_nats.stream_name);
    assert_ne!(nats.subject, org_nats.subject);

    rt.block_on(async {
        let tmp = tempfile::tempdir().expect("tempdir");
        let events_dir = tmp.path().join("events");
        let state = open_state(&events_dir, nats.clone()).await;
        state
            .event_store
            .record("m5/repo", test_event())
            .expect("record repo event");
        state.record_org(org_snapshot()).expect("record org event");
        drop(state);

        let repo_open = tokio::task::spawn_blocking({
            let nats = nats.clone();
            let handle = tokio::runtime::Handle::current();
            move || EventStoreImpl::open_jetstream(jetstream_backend(nats, handle))
        })
        .await
        .expect("repo open join")
        .expect("repo stream opens");
        let org_open = tokio::task::spawn_blocking({
            let org_nats = org_nats.clone();
            let handle = tokio::runtime::Handle::current();
            move || OrgEventStoreImpl::open_jetstream(jetstream_backend(org_nats, handle))
        })
        .await
        .expect("org open join")
        .expect("org stream opens");

        assert_eq!(repo_open.latest_per_repo().expect("repo latest").len(), 1);
        let projection = org_open
            .fold_events(
                gh_report::projection::EvidenceProjection::default(),
                |projection, event| {
                    projection.apply_org_state(event.clone().into());
                },
            )
            .expect("fold org events");
        assert_eq!(projection.org_state.expect("org state").archived_repos, 11);
    });
}
