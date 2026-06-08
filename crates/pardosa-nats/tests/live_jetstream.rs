//! Live `JetStream` tests for `append` / `sync`.
//!
//! `#[ignore]` by default (Phase 1.5 §8.4). CI runs via
//! `cargo test -p pardosa-nats --test live_jetstream -- --ignored`.
//! Requires pinned `nats-server` on `PATH`.
//!
//! Pinned invariants: monotonic `JetStreamAckPosition` aligned to
//! `PubAck.seq` (ADR-0022 §D2); byte round-trip identity (§D5);
//! re-publish dedup via `Nats-Msg-Id` (Phase 1.5 §6.4); idempotent
//! `sync` returning the latest ack-position.
mod support;
use pardosa_nats::{JetStreamBackend, JetStreamConfig, RuntimeHandle};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use support::LiveNatsServer;
use tokio::runtime::Runtime;
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
        .stream_name(format!("PARDOSA_LIVE_{tag}"))
        .subject(format!("pardosa.live.{tag}"))
        .durable_consumer(format!("pardosa-c-{tag}"))
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
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_append_returns_monotonic_seq() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let a = handle.append(b"event-a").expect("append a");
    let b = handle.append(b"event-b").expect("append b");
    let c = handle.append(b"event-c").expect("append c");
    assert!(a < b, "first seq < second");
    assert!(b < c, "second seq < third");
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_append_carries_canonical_bytes_unchanged() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let payload = b"alpha-beta-gamma";
    let ack = handle.append(payload).expect("append");
    let records = handle.replay_all().expect("replay_all");
    assert_eq!(records.len(), 1, "single append yields a single record");
    assert_eq!(
        records[0].payload.as_ref(),
        payload,
        "bytes round-trip byte-identical through the handle-owned replay surface (ADR-0022 §D5)"
    );
    assert_eq!(
        records[0].ack, ack,
        "replay record carries the ack-position minted by append"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_duplicate_publish_collapses_via_nats_msg_id() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let payload = b"deduplicate-me";
    let first = handle.append(payload).expect("first publish");
    let dup = handle.append(payload).expect("second publish (duplicate)");
    assert_eq!(
        first, dup,
        "byte-identical re-publish collapses to the same seq via Nats-Msg-Id dedup (Phase 1.5 §6.4)"
    );
    let different = handle
        .append(b"a-different-payload")
        .expect("third publish");
    assert!(
        different > first,
        "distinct payload yields a fresh, later seq"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_sync_is_idempotent_no_op_barrier() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let ack = handle.append(b"to-be-fenced").expect("append");
    let s1 = handle.sync().expect("first sync");
    let s2 = handle.sync().expect("second sync");
    assert_eq!(s1, ack, "sync returns the most recent ack-position");
    assert_eq!(s1, s2, "no-op sync is idempotent");
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_sync_before_any_append_returns_zero_position() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let pos = handle.sync().expect("sync with no prior append is vacuous");
    assert_eq!(
        pos.as_u64(),
        0,
        "vacuous sync returns the zero position (no bytes preceded)"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_replay_all_returns_payloads_in_publish_order() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let payloads: &[&[u8]] = &[b"first-event", b"second-event", b"third-event"];
    let mut ack_positions = Vec::new();
    for p in payloads {
        let ack = handle.append(p).expect("append");
        ack_positions.push(ack);
    }
    let records = handle.replay_all().expect("replay_all");
    assert_eq!(
        records.len(),
        payloads.len(),
        "one record per append (no dedup collisions across distinct payloads)"
    );
    for (i, record) in records.iter().enumerate() {
        assert_eq!(
            record.payload.as_ref(),
            payloads[i],
            "record {i} payload byte-identical to the {i}th append"
        );
        assert_eq!(
            record.ack, ack_positions[i],
            "record {i} ack-position matches the ack returned by the {i}th append"
        );
    }
    for w in records.windows(2) {
        assert!(
            w[0].ack < w[1].ack,
            "replay records are strictly monotonic in ack-position"
        );
    }
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_replay_all_on_empty_stream_returns_empty_vec() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    rt.block_on(async {
        use async_nats::jetstream::stream::{
            Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
        };
        let client = async_nats::connect(server.url()).await.expect("connect");
        let js = async_nats::jetstream::new(client);
        let stream_cfg = StreamConfig {
            name: stream_name.clone(),
            subjects: vec![format!("pardosa.live.{tag}")],
            storage: StorageType::File,
            num_replicas: 1,
            retention: RetentionPolicy::Limits,
            discard: DiscardPolicy::New,
            ..Default::default()
        };
        js.get_or_create_stream(stream_cfg)
            .await
            .expect("provision empty stream");
    });
    let records = handle.replay_all().expect("replay_all on empty stream");
    assert!(records.is_empty(), "empty stream replays as an empty Vec");
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-recovery-03-live-harness)"]
fn live_replay_all_collapses_duplicate_publishes() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let payload = b"deduplicate-me-via-replay";
    let first = handle.append(payload).expect("first publish");
    let dup = handle.append(payload).expect("duplicate publish");
    assert_eq!(
        first, dup,
        "byte-identical re-publish collapses (Phase 1.5 §6.4)"
    );
    let records = handle.replay_all().expect("replay_all");
    assert_eq!(
        records.len(),
        1,
        "dedup-collapsed duplicate appears once in the replay (Phase 1.5 §6.4)"
    );
    assert_eq!(
        records[0].ack, first,
        "single record carries the original ack-position"
    );
    assert_eq!(
        records[0].payload.as_ref(),
        payload,
        "single record carries the canonical payload bytes"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}
#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-followups-jetstream-open-06)"]
fn live_handle_recovers_prior_appends_after_drop_and_reopen() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let payloads: &[&[u8]] = &[
        b"recovery-payload-1",
        b"recovery-payload-2",
        b"recovery-payload-3",
    ];
    let mut original_acks = Vec::new();
    {
        let cfg = build_live_config(&tag, &rt, &server);
        let handle = JetStreamBackend::open(cfg);
        for p in payloads {
            let ack = handle.append(p).expect("append on writer handle");
            original_acks.push(ack);
        }
        let _ = handle.sync().expect("durability fence on writer handle");
    }
    let reopened_handle = {
        let cfg = build_live_config(&tag, &rt, &server);
        JetStreamBackend::open(cfg)
    };
    let records = reopened_handle
        .replay_all()
        .expect("replay_all on reopened handle must surface prior appends");
    assert_eq!(
        records.len(),
        payloads.len(),
        "reopened handle recovers every event the prior handle synced \
         (ADR-0022 §D2 sync-as-fence; substrate-level open/rehydrate \
         through the opaque handle path, mission \
         nats-followups-jetstream-open-06)"
    );
    for (i, record) in records.iter().enumerate() {
        assert_eq!(
            record.payload.as_ref(),
            payloads[i],
            "record {i} after reopen is byte-identical to the {i}th payload \
             written by the now-dropped handle (ADR-0022 §D5)"
        );
        assert_eq!(
            record.ack, original_acks[i],
            "record {i} ack-position after reopen matches the ack-position \
             returned by the prior handle's {i}th append — the opaque \
             positional primitive survives handle drop / reopen"
        );
    }
    for w in records.windows(2) {
        assert!(
            w[0].ack < w[1].ack,
            "reopened replay records remain strictly monotonic in ack-position"
        );
    }
    rt.block_on(teardown_stream(&server, &stream_name));
}
