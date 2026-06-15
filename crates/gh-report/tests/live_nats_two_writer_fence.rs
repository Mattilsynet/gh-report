use gh_report::app::state::AppState;
use gh_report::config::runtime::{NatsStoreConfig, PardosaBackend};
use gh_report::domain::checks::{
    BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus, CodeownersResult,
    CodeownersStatus, DependabotResult, DependabotStatus, RepositoryChecks, SecretScanningResult,
    SecretScanningStatus, SecurityPolicyEvidence, SecurityPolicyResult, SecurityPolicyStatus,
};
use gh_report::domain::evidence::RepositoryEvidence;
use gh_report::domain::repository::{Repository, Visibility};
use gh_report::error::{AppError, PersistenceError};
use gh_report::event::DomainEvent;
use pardosa::store::{BackendError, PardosaError};
use pardosa_nats::test_support::LiveNatsServer;
use pardosa_schema::{NonEmptyEventString, Timestamp as EventTimestamp};
use std::error::Error;
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

fn unique_org() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("fence-live-{pid}-{nanos}")
}

fn event(domain_key: &str, repo_name: &str, nanos: u64) -> DomainEvent {
    DomainEvent::RepositoryStateCaptured {
        domain_key: NonEmptyEventString::try_new(domain_key).expect("domain key"),
        repo_name: NonEmptyEventString::try_new(repo_name).expect("repo name"),
        timestamp: EventTimestamp::from_nanos(nanos).expect("timestamp"),
        evidence: None,
    }
}

fn repository_evidence(name: &str) -> RepositoryEvidence {
    let timestamp = "2026-06-15T00:00:00Z".to_string();
    RepositoryEvidence {
        repository: Repository {
            id: format!("id-{name}"),
            node_id: None,
            name: name.to_string(),
            visibility: Visibility::Public,
            language: None,
            default_branch: "main".to_string(),
            archived: false,
            inventory_key: format!("id-{name}"),
            updated_at: None,
            has_issues: true,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            html_url: None,
            topics: vec![],
            license_spdx: None,
        },
        checks: RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: SecurityPolicyStatus::Pass,
                evidence: SecurityPolicyEvidence::Setting,
                path: None,
                timestamp: timestamp.clone(),
            },
            secret_scanning: SecretScanningResult {
                status: SecretScanningStatus::Enabled,
                has_open_alerts: Some(false),
                alerts_observable: true,
                reason: None,
                timestamp: timestamp.clone(),
            },
            dependabot_security_updates: DependabotResult {
                status: DependabotStatus::Enabled,
                reason: None,
                timestamp: timestamp.clone(),
            },
            branch_protection: BranchProtectionResult {
                status: BranchProtectionStatus::Pass,
                details: BranchProtectionDetails {
                    default_branch: "main".to_string(),
                    has_pr: Some(true),
                    required_reviewers: Some(1),
                    has_status_checks: Some(true),
                    admin_equivalent: Some(true),
                    has_broad_bypass: Some(false),
                    reason: None,
                },
                timestamp: timestamp.clone(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp,
                parsed: None,
                truncation: None,
            },
        },
        last_commit: None,
    }
}

async fn open_state(tmp: &Path, nats: NatsStoreConfig) -> Arc<AppState> {
    AppState::with_stores(
        &tmp.join("events"),
        tmp.join("projections"),
        PardosaBackend::Nats,
        nats,
    )
    .await
    .expect("AppState::with_stores over live NATS")
}

fn is_pardosa_conflict(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if matches!(
            error.downcast_ref::<PardosaError>(),
            Some(PardosaError::ConcurrencyConflict { .. })
        ) || matches!(
            error.downcast_ref::<BackendError>(),
            Some(BackendError::ConcurrencyConflict { .. })
        ) {
            return true;
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
            && is_pardosa_conflict(inner)
        {
            return true;
        }
        current = error.source();
    }
    false
}

fn assert_typed_fenced_conflict(error: &PersistenceError) {
    let PersistenceError::FencedConflict { source } = error else {
        panic!("expected typed PersistenceError::FencedConflict, got {error:?}");
    };
    assert!(
        is_pardosa_conflict(source.as_ref()),
        "fenced conflict source chain must preserve PardosaError::ConcurrencyConflict"
    );
}

async fn subject_message_count(nats_url: &str, stream_name: &str) -> u64 {
    let client = async_nats::connect(nats_url).await.expect("connect");
    let js = async_nats::jetstream::new(client);
    let stream = js.get_stream(stream_name).await.expect("get stream");
    stream.get_info().await.expect("stream info").state.messages
}

async fn purge_stream(nats_url: &str, stream_name: &str) {
    let client = async_nats::connect(nats_url).await.expect("connect");
    let js = async_nats::jetstream::new(client);
    let stream = js.get_stream(stream_name).await.expect("get stream");
    stream.purge().await.expect("purge stream");
    let info = stream.get_info().await.expect("stream info after purge");
    assert_eq!(info.state.messages, 0, "purge leaves stream empty");
}

#[test]
fn two_writer_fence_conflicts_loser_and_single_writer_handles_sync_and_purge() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let org = unique_org();
    let nats = NatsStoreConfig::for_org(&org, server.url().to_owned()).expect("nats config");
    let _repo_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.stream_name.clone(),
    };
    let _org_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.org_events().stream_name,
    };

    rt.block_on(async {
        let first_tmp = tempfile::tempdir().expect("first tempdir");
        let first = open_state(first_tmp.path(), nats.clone()).await;
        first
            .event_store
            .record("fence/repo-a", event("fence/repo-a", "repo-a", 1))
            .expect("single writer first sync succeeds");
        first
            .event_store
            .record("fence/repo-b", event("fence/repo-b", "repo-b", 2))
            .expect("single writer second sync uses updated expect seq");

        let second_tmp = tempfile::tempdir().expect("second tempdir");
        let second = open_state(second_tmp.path(), nats.clone()).await;
        first
            .event_store
            .record("fence/repo-c", event("fence/repo-c", "repo-c", 3))
            .expect("winner append succeeds");
        let loser = second
            .record_repo(
                "fence/repo-loser",
                repository_evidence("repo-loser"),
                "repo-loser",
                "2026-06-15T00:00:00Z",
            )
            .expect_err("stale second writer must be fenced");
        assert_typed_fenced_conflict(&loser);

        match AppError::Persistence(loser) {
            AppError::Persistence(PersistenceError::FencedConflict { .. }) => {}
            other => panic!("expected persistence fenced conflict, got {other:?}"),
        }

        let authoritative = first.event_store.events().expect("authoritative events");
        assert_eq!(
            authoritative.len(),
            3,
            "zero duplicate authoritative events"
        );
        assert!(
            authoritative.iter().any(|(_, event)| matches!(
                event,
                DomainEvent::RepositoryStateCaptured { domain_key, .. }
                if domain_key.as_str() == "fence/repo-c"
            )),
            "winner message remains authoritative"
        );
        assert!(
            authoritative.iter().all(|(_, event)| matches!(
                event,
                DomainEvent::RepositoryStateCaptured { domain_key, .. }
                if domain_key.as_str() != "fence/repo-loser"
            )),
            "loser event must not be authoritatively appended"
        );
        assert_eq!(
            subject_message_count(server.url(), &nats.stream_name).await,
            3,
            "JetStream subject contains exactly the winner's messages"
        );

        purge_stream(server.url(), &nats.stream_name).await;
        let after_purge_tmp = tempfile::tempdir().expect("post-purge tempdir");
        let after_purge = open_state(after_purge_tmp.path(), nats.clone()).await;
        after_purge
            .event_store
            .record(
                "fence/repo-after-purge",
                event("fence/repo-after-purge", "repo-after-purge", 4),
            )
            .expect("fresh post-purge writer uses subject baseline 0");
        assert_eq!(
            subject_message_count(server.url(), &nats.stream_name).await,
            1,
            "post-purge append re-establishes exactly one authoritative message"
        );
    });
}
