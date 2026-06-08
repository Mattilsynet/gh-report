//! Live JetStream-authoritative recovery via the public
//! `EventStore::open_with_backend(JetStreamBackend)` path
//! (ADR-0022 §D1, §D11).
//!
//! Writes `LedgerEntry` events through `EventStore::writer()` on a
//! `JetStream`-backed store, drops it, reopens via
//! `open_with_backend(JetStreamBackend)`, asserts the recovered line
//! equals the committed line. Proves the §D2 reader-side seam
//! end-to-end against a live `nats-server` with no
//! `JetStreamRecoveryJournal` reach-through.
//!
//! Seeding: `from_pgno_bytes_unchecked` rejects zero bytes, so a
//! fresh stream needs the canonical empty-`.pgno`-for-`LedgerEntry`
//! blob pushed via `pardosa_nats::JetStreamHandle::append` before the
//! first reopen. Subsequent steps use public `pardosa::store::*`.
//!
//! `#[ignore]` by default; needs the pinned `nats-server`:
//!
//! ```text
//! cargo test -p pardosa-test-support-harness \
//!   --test live_jetstream_authoritative_recovery -- --ignored \
//!   --exact jetstream_authoritative_store_append_sync_reopen_recovers_events
//! ```
mod support_live_nats;
use pardosa::store::{EventStore, FiberId, GenomeSafe, HasEventSchemaSource, JetStreamBackend};
use pardosa_nats::{
    JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, JetStreamHandle, RuntimeHandle,
};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use support_live_nats::LiveNatsServer;
use tokio::runtime::Runtime;
#[derive(Debug, Clone, Copy, PartialEq, Eq, GenomeSafe)]
struct LedgerEntry {
    seq: u64,
    amount_cents: u64,
}
impl HasEventSchemaSource for LedgerEntry {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}
fn unique_stream_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("{pid}_{nanos}")
}
fn build_live_config(tag: &str, rt: &Runtime, server: &LiveNatsServer) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(format!("PARDOSA_JS_RECOVERY_{tag}"))
        .subject(format!("pardosa.js.recovery.{tag}"))
        .durable_consumer(format!("pardosa-js-recovery-c-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
        .nats_url(server.url().to_owned())
        .build()
        .expect("config valid")
}
async fn teardown_stream(server: &LiveNatsServer, stream_name: &str) {
    let Ok(client) = async_nats::connect(server.url()).await else {
        return;
    };
    let js = async_nats::jetstream::new(client);
    let _ = js.delete_stream(stream_name).await;
}
fn scratch_pgno_path(tag: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    p.push(format!(
        "pardosa-jetstream-recovery-seed-{}-{tag}-{nanos}.pgno",
        std::process::id()
    ));
    p
}
fn canonical_empty_pgno_bytes_for_ledger_entry(tag: &str) -> Vec<u8> {
    let scratch = scratch_pgno_path(tag);
    {
        let mut store: EventStore<LedgerEntry> =
            EventStore::<LedgerEntry>::create(&scratch).expect("create scratch .pgno seed");
        let _ = store.writer().sync().expect("sync scratch .pgno seed");
    }
    let bytes = std::fs::read(&scratch).expect("read scratch .pgno seed bytes");
    let _ = std::fs::remove_file(&scratch);
    assert!(
        !bytes.is_empty(),
        "canonical empty-dragline .pgno serialisation must be non-empty (header + index + footer)"
    );
    bytes
}
fn seed_empty_pgno_blob(handle: &JetStreamHandle, bytes: &[u8]) {
    let _ = handle
        .append(bytes)
        .expect("seed canonical empty .pgno blob to JetStream substrate");
    let _ = handle
        .sync()
        .expect("fence the seed publish under §D2 sync-as-fence");
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission prove-public-eventstore-jetstream-authoritative-01)"]
fn jetstream_authoritative_store_append_sync_reopen_recovers_events() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_JS_RECOVERY_{tag}");
    let committed: Vec<LedgerEntry> = (1..=5u64)
        .map(|seq| LedgerEntry {
            seq,
            amount_cents: seq * 1000,
        })
        .collect();
    let seed_bytes = canonical_empty_pgno_bytes_for_ledger_entry(&tag);
    {
        let cfg = build_live_config(&tag, &rt, &server);
        let handle = SubstrateJetStreamBackend::open(cfg);
        seed_empty_pgno_blob(&handle, &seed_bytes);
    }
    let captured_fiber_ids: Vec<FiberId> = {
        let cfg = build_live_config(&tag, &rt, &server);
        let handle = SubstrateJetStreamBackend::open(cfg);
        let backend = JetStreamBackend::open(handle);
        let mut store: EventStore<LedgerEntry> =
            EventStore::<LedgerEntry>::open_with_backend(backend).expect(
                "open_with_backend(JetStreamBackend) must succeed against a stream pre-seeded \
                 with the canonical empty .pgno blob for LedgerEntry (ADR-0022 §D2 reader-side \
                 seam admits the §D11 sealed substrate handle through the typed-backend \
                 admission seam)",
            );
        let mut writer = store.writer();
        let mut fiber_ids: Vec<FiberId> = Vec::with_capacity(committed.len());
        for entry in &committed {
            let receipt = writer.begin(*entry).expect("begin LedgerEntry fiber");
            fiber_ids.push(receipt.fiber().fiber_id());
        }
        let _lsn = writer.sync().expect(
            "writer().sync() must drive WriteStrategy::JetStreamBacked → \
             BackendSink::append + BackendSink::sync on the in-crate JetStream adapter, \
             publishing the .pgno blob to the live substrate and returning the \
             post-fence AckPosition mapped through to an Lsn (ADR-0022 §D2)",
        );
        fiber_ids
    };
    let reopened_store: EventStore<LedgerEntry> = {
        let cfg = build_live_config(&tag, &rt, &server);
        let handle = SubstrateJetStreamBackend::open(cfg);
        let backend = JetStreamBackend::open(handle);
        EventStore::<LedgerEntry>::open_with_backend(backend).expect(
            "EventStore::<LedgerEntry>::open_with_backend(JetStreamBackend) must \
             rehydrate from the JetStream-authoritative substrate after writer drop \
             — surfacing every previously-committed event via the §D2 reader-side \
             seam (latest-payload wins; the seed-blob is discarded by latest_payload \
             in favour of the writer-side sync blob carrying all committed entries)",
        )
    };
    let reader = reopened_store.reader();
    let recovered: Vec<LedgerEntry> = captured_fiber_ids
        .iter()
        .map(|fid| {
            let history = reader.fiber(*fid).iter().expect(
                "fiber history present after reopen via open_with_backend(JetStreamBackend)",
            );
            let events: Vec<LedgerEntry> = history.map(|e| *e.domain_event()).collect();
            assert_eq!(
                events.len(),
                1,
                "each fiber was opened with exactly one begin(...) event, so its \
                 history after recovery must hold exactly that one event \
                 (fiber {fid:?})"
            );
            events[0]
        })
        .collect();
    assert_eq!(
        recovered, committed,
        "recovered event line equals the originally committed entries — the public \
         EventStore writer → JetStream substrate → drop → public \
         EventStore::open_with_backend(JetStreamBackend) reopen → reader path \
         rehydrates the same events without any JetStreamRecoveryJournal reach-through \
         and without any async_nats reach-through from pardosa (ADR-0022 §D2 / §D11; \
         mission prove-public-eventstore-jetstream-authoritative-01)"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}
