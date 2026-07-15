//! F4 property: N writers race the real `JetStream` OCC fence.
//!
//! Roadmap `adr-fmt-2ysyq` §C/§D Seq 2: author (not reuse) the property
//! "N writers race on a seq → exactly one wins → the loser surfaces
//! `FencedConflict`/`ConcurrencyConflict` and aborts cleanly (no
//! retry-to-win, per the `341188b` amendment) → a single handle never
//! self-fences (`Semaphore(1)`)". Ports the `i1_toctou_pin.rs` fan-out
//! shape onto the real `JetStream` fence (not `InMemoryEventStore`).
//!
//! Each property case spawns `n` independent [`AppState`]s over the
//! same fresh org/domain-key baseline and races their `record` calls
//! from `n` OS threads (`record` is a synchronous facade call — no
//! `tokio::spawn` needed to force genuine concurrent contention at the
//! server). Self-spawns [`LiveNatsServer`], matching the
//! `live_nats_two_writer_fence.rs` sibling test's convention (no
//! `#[ignore]`; requires no external server).
use gh_report::app::state::AppState;
use gh_report::config::runtime::{NatsStoreConfig, PardosaBackend};
use gh_report::event::DomainEvent;
use gh_report::store::StoreError;
use pardosa::store::BackendError;
use pardosa_nats::test_support::LiveNatsServer;
use pardosa_schema::{NonEmptyEventString, Timestamp as EventTimestamp};
use proptest::prelude::*;
use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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

fn unique_tag() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("nwriter-{pid}-{nanos}-{seq}")
}

fn event(domain_key: &str, repo_name: &str, nanos: u64) -> DomainEvent {
    DomainEvent::RepositoryStateCaptured {
        domain_key: NonEmptyEventString::try_new(domain_key).expect("domain key"),
        repo_name: NonEmptyEventString::try_new(repo_name).expect("repo name"),
        timestamp: EventTimestamp::from_nanos(nanos + 1).expect("timestamp"),
        evidence: None,
    }
}

async fn open_state(tmp: &Path, nats: NatsStoreConfig) -> Arc<AppState> {
    AppState::with_stores(&tmp.join("events"), PardosaBackend::Nats, nats)
        .await
        .expect("AppState::with_stores over live NATS")
}

fn is_fence_conflict(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if matches!(
            error.downcast_ref::<StoreError>(),
            Some(StoreError::ConcurrencyConflict { .. })
        ) || matches!(
            error.downcast_ref::<BackendError>(),
            Some(BackendError::ConcurrencyConflict { .. })
        ) {
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

/// N-writer race: spawn `n` independently-opened [`AppState`]s at the
/// same (empty) baseline and race their first `record` on a shared
/// domain key from `n` OS threads. Asserts exactly one wins, every
/// loser surfaces `ConcurrencyConflict` and aborts cleanly (the call
/// returns the error immediately — no in-band retry-to-win per the
/// `341188b` amendment), and the server durably holds exactly one
/// message.
fn n_writers_race_real_fence(n: usize) {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let org = format!("fence-race-{tag}");
    let nats = NatsStoreConfig::for_org(&org, server.url().to_owned()).expect("nats config");
    let _repo_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.stream_name.clone(),
    };
    let _org_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.org_events().stream_name,
    };
    let domain_key = format!("fence-race/{tag}");

    let mut tempdirs = Vec::with_capacity(n);
    let states: Vec<Arc<AppState>> = rt.block_on(async {
        let mut states = Vec::with_capacity(n);
        for _ in 0..n {
            let tmp = tempfile::tempdir().expect("writer tempdir");
            states.push(open_state(tmp.path(), nats.clone()).await);
            tempdirs.push(tmp);
        }
        states
    });

    let handles: Vec<_> = states
        .into_iter()
        .enumerate()
        .map(|(i, state)| {
            let domain_key = domain_key.clone();
            let repo_name = format!("repo-{tag}-{i}");
            std::thread::spawn(move || {
                let nanos = u64::try_from(i).expect("small index");
                state
                    .event_store
                    .record(&domain_key, event(&domain_key, &repo_name, nanos))
            })
        })
        .collect();

    let results: Vec<Result<(), StoreError>> = handles
        .into_iter()
        .map(|h| h.join().expect("writer thread must not panic"))
        .collect();

    let wins = results.iter().filter(|r| r.is_ok()).count();
    let loss_count = results.iter().filter(|r| r.is_err()).count();

    assert_eq!(
        wins, 1,
        "exactly one of {n} racing writers must win the fence; got {wins} winners, results={results:?}"
    );
    assert_eq!(
        loss_count,
        n - 1,
        "every non-winner must surface a distinct loss (no swallow, no silent success)"
    );
    for result in &results {
        if let Err(loss) = result {
            assert!(
                is_fence_conflict(loss),
                "loser must abort cleanly with ConcurrencyConflict, not some other error: {loss:?}"
            );
        }
    }

    rt.block_on(async {
        assert_eq!(
            subject_message_count(server.url(), &nats.stream_name).await,
            1,
            "JetStream subject must durably hold exactly the single winner's message"
        );
    });
}

/// I2b self-fence check: a single handle issuing two sequential
/// appends from two racing threads must never self-fence — the
/// `append_gate` `Semaphore(1)` (handle.rs:203-208) serializes intra-
/// handle appends so the second thread observes the first's updated
/// `last_ack_seq` before publishing. Both must succeed.
#[test]
fn single_handle_two_racing_appends_never_self_fence() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_tag();
    let org = format!("self-fence-{tag}");
    let nats = NatsStoreConfig::for_org(&org, server.url().to_owned()).expect("nats config");
    let _repo_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.stream_name.clone(),
    };
    let _org_cleanup = StreamCleanup {
        nats_url: server.url().to_owned(),
        stream_name: nats.org_events().stream_name,
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let state = rt.block_on(open_state(tmp.path(), nats.clone()));

    let handles: Vec<_> = (0..2u64)
        .map(|i| {
            let state = Arc::clone(&state);
            let domain_key = format!("self-fence/{tag}-{i}");
            let repo_name = format!("repo-self-{tag}-{i}");
            std::thread::spawn(move || {
                state
                    .event_store
                    .record(&domain_key, event(&domain_key, &repo_name, i))
            })
        })
        .collect();

    let results: Vec<Result<(), StoreError>> = handles
        .into_iter()
        .map(|h| h.join().expect("writer thread must not panic"))
        .collect();

    for (i, r) in results.iter().enumerate() {
        assert!(
            r.is_ok(),
            "single handle's own concurrent appends must never self-fence (result {i}): {r:?}"
        );
    }

    rt.block_on(async {
        assert_eq!(
            subject_message_count(server.url(), &nats.stream_name).await,
            2,
            "both intra-handle appends must land durably, serialized by append_gate"
        );
    });
}

#[test]
fn two_writers_race_real_fence_smoke() {
    n_writers_race_real_fence(2);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4, ..ProptestConfig::default() })]
    #[test]
    fn n_writers_race_real_fence_property(n in 2usize..=4) {
        n_writers_race_real_fence(n);
    }
}
