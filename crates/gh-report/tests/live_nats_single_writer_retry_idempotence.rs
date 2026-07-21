//! Pins PGN-0016:R2/R11 — a single writer's retry of an already-succeeded
//! append, computed against the now-stale expected-subject-sequence, is
//! fenced (`BackendError::ConcurrencyConflict`, NATS err 10071/10164) and
//! never lands a second authoritative message. This is distinct from
//! `live_nats_two_writer_fence.rs` / `live_nats_n_writer_fence_property.rs`,
//! which pin the same underlying mechanism but frame it as two *distinct*
//! writer handles racing; this test pins the same-writer retry-with-stale-
//! expected-sequence framing that adr-fmt-upfoa Q2 found absent.
use gh_report::app::state::AppState;
use gh_report::config::runtime::{NatsStoreConfig, PardosaBackend};
use gh_report::event::DomainEvent;
use gh_report::store::StoreError;
use pardosa::store::{BackendError, PardosaError};
use pardosa_nats::test_support::LiveNatsServer;
use pardosa_schema::{NonEmptyEventString, Timestamp as EventTimestamp};
use std::error::Error;
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
    format!("single-writer-retry-{pid}-{nanos}")
}

fn event(domain_key: &str, repo_name: &str, nanos: u64) -> DomainEvent {
    DomainEvent::RepositoryStateCaptured {
        domain_key: NonEmptyEventString::try_new(domain_key).expect("domain key"),
        repo_name: NonEmptyEventString::try_new(repo_name).expect("repo name"),
        timestamp: EventTimestamp::from_nanos(nanos).expect("timestamp"),
        evidence: None,
    }
}

async fn open_state(tmp: &std::path::Path, nats: NatsStoreConfig) -> Arc<AppState> {
    AppState::with_stores(&tmp.join("events"), PardosaBackend::Nats, nats)
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

async fn subject_message_count(nats_url: &str, stream_name: &str) -> u64 {
    let client = async_nats::connect(nats_url).await.expect("connect");
    let js = async_nats::jetstream::new(client);
    let stream = js.get_stream(stream_name).await.expect("get stream");
    stream.get_info().await.expect("stream info").state.messages
}

#[test]
fn append_is_idempotent_under_fence_single_writer_stale_expected_sequence() {
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
        let domain_key = "fence/single-writer-retry";
        let payload = event(domain_key, "single-writer-retry", 1);

        let first_tmp = tempfile::tempdir().expect("first tempdir");
        let first = open_state(first_tmp.path(), nats.clone()).await;

        let retry_tmp = tempfile::tempdir().expect("retry tempdir");
        let retry = open_state(retry_tmp.path(), nats.clone()).await;

        first
            .event_store
            .record(domain_key, payload.clone())
            .expect("original append succeeds and advances the subject tip");

        let retry_error = retry.event_store.record(domain_key, payload).expect_err(
            "retry computed against the pre-append expected-sequence must be fenced, \
                 not silently re-appended (PGN-0016:R2/R11)",
        );
        let StoreError::ConcurrencyConflict { source, .. } = &retry_error else {
            panic!(
                "retry rejection must be the typed StoreError::ConcurrencyConflict variant, \
                 got {retry_error:?}"
            );
        };
        assert!(
            is_pardosa_conflict(source.as_ref()),
            "retry rejection source chain must preserve BackendError::ConcurrencyConflict \
             (err 10071/10164), got {source:?}"
        );

        let authoritative = first.event_store.events().expect("authoritative events");
        assert_eq!(
            authoritative.len(),
            1,
            "the fenced retry must not double-append: exactly one authoritative event remains"
        );
        assert_eq!(
            subject_message_count(server.url(), &nats.stream_name).await,
            1,
            "JetStream subject carries exactly the original append, not a retried duplicate"
        );
    });
}
