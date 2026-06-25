use pardosa::store::{
    Event, EventStore, FiberId, GenomeSafe, HasEventSchemaSource, JetStreamBackend,
};
use pardosa_nats::{JetStreamBackend as SubstrateJetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, GenomeSafe)]
struct LedgerEntry {
    seq: u64,
    amount_cents: u64,
}

impl HasEventSchemaSource for LedgerEntry {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

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
    std::env::var("PARDOSA_LIVE_NATS_URL").unwrap_or_else(|_| "nats://localhost:4222".to_string())
}

fn unique_stream_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pid = std::process::id();
    format!("{pid}_{nanos}")
}

fn build_live_config(tag: &str, rt: &Runtime, nats_url: &str) -> JetStreamConfig {
    JetStreamConfig::builder()
        .stream_name(format!("PARDOSA_CREATE_WITH_BACKEND_{tag}"))
        .subject(format!("pardosa.create_with_backend.{tag}"))
        .durable_consumer(format!("pardosa-create-with-backend-c-{tag}"))
        .runtime_handle(RuntimeHandle::from_tokio(rt.handle().clone()))
        .nats_url(nats_url.to_owned())
        .build()
        .expect("config valid")
}

fn jetstream_backend(tag: &str, rt: &Runtime, nats_url: &str) -> JetStreamBackend {
    let cfg = build_live_config(tag, rt, nats_url);
    let handle = SubstrateJetStreamBackend::open(cfg);
    JetStreamBackend::open(handle)
}

fn recovered_entries(store: &EventStore<LedgerEntry>, fiber_ids: &[FiberId]) -> Vec<LedgerEntry> {
    let reader = store.reader();
    fiber_ids
        .iter()
        .map(|fid| {
            let history = reader
                .fiber(*fid)
                .iter()
                .expect("fiber history present after reopen");
            let entries = history
                .map(|event| *event.domain_event())
                .collect::<Vec<_>>();
            assert_eq!(entries.len(), 1, "each fiber holds one committed entry");
            entries[0]
        })
        .collect()
}

#[test]
#[ignore = "requires live nats-server at PARDOSA_LIVE_NATS_URL or nats://localhost:4222"]
fn create_with_backend_fresh_write_reopen_and_idempotent_create_preserves_events() {
    let rt = Runtime::new().expect("tokio runtime");
    let nats_url = live_nats_url();
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_CREATE_WITH_BACKEND_{tag}");
    let _cleanup = StreamCleanup {
        nats_url: nats_url.clone(),
        stream_name,
    };

    let fresh_open =
        EventStore::<LedgerEntry>::open_with_backend(jetstream_backend(&tag, &rt, &nats_url));
    assert!(
        fresh_open.is_ok(),
        "fresh open_with_backend must admit an empty markerless JetStream stream under the stream-level gate"
    );

    let mut store =
        EventStore::<LedgerEntry>::create_with_backend(jetstream_backend(&tag, &rt, &nats_url))
            .expect("create_with_backend must seed a fresh JetStream stream");
    let empty_index = store
        .reader()
        .fiber_index(|_: &Event<LedgerEntry>| std::iter::empty::<u8>());
    assert_eq!(empty_index.key_count(), 0, "fresh create yields empty line");

    let committed = vec![
        LedgerEntry {
            seq: 1,
            amount_cents: 1000,
        },
        LedgerEntry {
            seq: 2,
            amount_cents: 2000,
        },
        LedgerEntry {
            seq: 3,
            amount_cents: 3000,
        },
    ];
    let captured_fiber_ids = {
        let mut writer = store.writer();
        let mut fiber_ids = Vec::with_capacity(committed.len());
        for entry in &committed {
            let receipt = writer.begin(*entry).expect("begin ledger entry fiber");
            fiber_ids.push(receipt.fiber().fiber_id());
        }
        let _lsn = writer.sync().expect("sync ledger entries to JetStream");
        fiber_ids
    };
    drop(store);

    let reopened =
        EventStore::<LedgerEntry>::open_with_backend(jetstream_backend(&tag, &rt, &nats_url))
            .expect("open_with_backend must rehydrate after create/write/sync");
    assert_eq!(
        recovered_entries(&reopened, &captured_fiber_ids),
        committed,
        "create_with_backend seed bytes must be consistent with writer sync bytes"
    );
    drop(reopened);

    let reopened_via_create =
        EventStore::<LedgerEntry>::create_with_backend(jetstream_backend(&tag, &rt, &nats_url))
            .expect("create_with_backend on a populated stream must not clobber");
    assert_eq!(
        recovered_entries(&reopened_via_create, &captured_fiber_ids),
        committed,
        "idempotent create guard must preserve populated stream events"
    );
    drop(reopened_via_create);

    let reopened_after_guard =
        EventStore::<LedgerEntry>::open_with_backend(jetstream_backend(&tag, &rt, &nats_url))
            .expect("open_with_backend must still rehydrate after guarded create");
    assert_eq!(
        recovered_entries(&reopened_after_guard, &captured_fiber_ids),
        committed,
        "populated reopen after create guard must not lose data"
    );
}
