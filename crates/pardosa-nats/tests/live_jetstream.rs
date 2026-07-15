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
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
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
    assert!(a.ack < b.ack, "first seq < second");
    assert!(b.ack < c.ack, "second seq < third");
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
        records[0].ack, ack.ack,
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
        first.ack, dup.ack,
        "byte-identical re-publish collapses to the same seq via Nats-Msg-Id dedup (Phase 1.5 §6.4)"
    );
    assert!(
        !first.duplicate,
        "the original publish is not itself a dedup hit"
    );
    assert!(
        dup.duplicate,
        "I8: re-publish of a byte-identical payload must surface \
         PublishAck.duplicate=true as the dedup-hit signal"
    );
    let different = handle
        .append(b"a-different-payload")
        .expect("third publish");
    assert!(
        different.ack > first.ack,
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
    assert_eq!(s1, ack.ack, "sync returns the most recent ack-position");
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
        ack_positions.push(ack.ack);
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
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission nats-purge-guard-1749816000)"]
fn live_replay_all_on_purged_stream_returns_empty_vec_promptly() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let ack = handle
        .append(b"purge-before-replay")
        .expect("append before purge");
    assert!(
        ack.ack.as_u64() > 0,
        "seed append establishes a non-zero JetStream last_sequence before purge"
    );
    rt.block_on(async {
        let client = async_nats::connect(server.url()).await.expect("connect");
        let js = async_nats::jetstream::new(client);
        let stream = js
            .get_stream(stream_name.as_str())
            .await
            .expect("get stream");
        stream.purge().await.expect("purge stream");
        let info = stream.get_info().await.expect("read purged stream state");
        assert_eq!(
            info.state.messages, 0,
            "purge leaves the stream logically empty for replay"
        );
        assert!(
            info.state.last_sequence >= ack.ack.as_u64(),
            "purge retains the prior sequence frontier that used to defeat the empty-stream guard"
        );
    });
    let started = Instant::now();
    let records = handle
        .replay_all()
        .expect("replay_all on purged stream returns promptly");
    let elapsed = started.elapsed();
    assert!(records.is_empty(), "purged stream replays as an empty Vec");
    assert!(
        elapsed < Duration::from_secs(5),
        "purged stream replay returns before the 30s backend operation timeout; elapsed {elapsed:?}"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}

#[test]
fn live_update_stream_description_round_trips_on_populated_markerless_stream() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let subject = format!("pardosa.live.{tag}");
    let marker = "0123456789abcdef0123456789abcdef".to_owned();
    let read_back = rt.block_on(async {
        use async_nats::jetstream::stream::{
            Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
        };
        let client = async_nats::connect(server.url()).await.expect("connect");
        let js = async_nats::jetstream::new(client);
        let mut stream_cfg = StreamConfig {
            name: stream_name.clone(),
            subjects: vec![subject.clone()],
            storage: StorageType::File,
            num_replicas: 1,
            retention: RetentionPolicy::Limits,
            discard: DiscardPolicy::New,
            duplicate_window: Duration::from_mins(2),
            ..Default::default()
        };
        js.get_or_create_stream(stream_cfg.clone())
            .await
            .expect("provision markerless stream");
        let publish_ack = js
            .publish(
                subject.clone(),
                bytes::Bytes::from_static(b"gap-a-populated"),
            )
            .await
            .expect("publish accepted by JetStream")
            .await
            .expect("publish ack received");
        assert!(
            publish_ack.sequence > 0,
            "publish ack sequence proves the markerless stream is populated"
        );
        stream_cfg.description = Some(marker.clone());
        let update_info = js
            .update_stream(stream_cfg.clone())
            .await
            .expect("update stream description");
        assert_eq!(
            update_info.config.description.as_deref(),
            Some(marker.as_str()),
            "update_stream response carries the written description"
        );
        let second_update_info = js
            .update_stream(stream_cfg)
            .await
            .expect("repeat stream description update");
        assert_eq!(
            second_update_info.config.description.as_deref(),
            Some(marker.as_str()),
            "identical update_stream remains conflict-free"
        );
        let stream = js
            .get_stream(stream_name.as_str())
            .await
            .expect("get stream");
        let info = stream.get_info().await.expect("read stream info");
        let read_back = info.config.description;
        eprintln!(
            "GAP_A_READ_BACK_DESCRIPTION={}",
            read_back.as_deref().unwrap_or("<none>")
        );
        read_back
    });
    rt.block_on(teardown_stream(&server, &stream_name));
    assert_eq!(
        read_back.as_deref(),
        Some(marker.as_str()),
        "NATS 2.14.3 must read back a description on a populated stream"
    );
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
        first.ack, dup.ack,
        "byte-identical re-publish collapses (Phase 1.5 §6.4)"
    );
    let records = handle.replay_all().expect("replay_all");
    assert_eq!(
        records.len(),
        1,
        "dedup-collapsed duplicate appears once in the replay (Phase 1.5 §6.4)"
    );
    assert_eq!(
        records[0].ack, first.ack,
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
            original_acks.push(ack.ack);
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

#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
fn live_custom_header_survives_jetstream_store_and_replay() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let subject = format!("pardosa.live.{tag}");
    rt.block_on(async {
        use async_nats::jetstream::consumer::{
            AckPolicy, DeliverPolicy, ReplayPolicy, pull::Config as PullConfig,
        };
        use async_nats::jetstream::stream::{
            Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
        };
        use futures_util::StreamExt;
        let client = async_nats::connect(server.url()).await.expect("connect");
        let js = async_nats::jetstream::new(client);
        let stream = js
            .get_or_create_stream(StreamConfig {
                name: stream_name.clone(),
                subjects: vec![subject.clone()],
                storage: StorageType::File,
                num_replicas: 1,
                retention: RetentionPolicy::Limits,
                discard: DiscardPolicy::New,
                duplicate_window: Duration::from_mins(2),
                ..Default::default()
            })
            .await
            .expect("provision stream");
        let header_name = "Pardosa-Envelope-Hash";
        let header_value = "0123456789abcdef0123456789abcdef";
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(header_name, header_value);
        let publish_ack = js
            .publish_with_headers(
                subject.clone(),
                headers,
                bytes::Bytes::from_static(b"header-survival-probe"),
            )
            .await
            .expect("publish accepted by JetStream")
            .await
            .expect("publish ack received");
        assert!(
            publish_ack.sequence > 0,
            "publish ack sequence proves the header probe reached JetStream"
        );
        let consumer = stream
            .create_consumer(PullConfig {
                deliver_policy: DeliverPolicy::All,
                ack_policy: AckPolicy::None,
                filter_subject: subject.clone(),
                replay_policy: ReplayPolicy::Instant,
                inactive_threshold: Duration::from_secs(30),
                ..Default::default()
            })
            .await
            .expect("create replay consumer");
        let mut messages = consumer.messages().await.expect("open replay messages");
        let msg = messages
            .next()
            .await
            .expect("one replayed message")
            .expect("replayed message ok");
        let replayed_headers = msg
            .headers
            .as_ref()
            .expect("replayed message carries headers");
        let replayed_value = replayed_headers
            .get(header_name)
            .expect("replayed message carries custom header")
            .to_string();
        assert_eq!(
            replayed_value, header_value,
            "custom header survives JetStream store-and-replay verbatim"
        );
    });
    rt.block_on(teardown_stream(&server, &stream_name));
}

#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission g3-jetstream-schema-gate-exec)"]
fn live_append_with_replay_tag_surfaces_tag_on_replay() {
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = JetStreamBackend::open(cfg);
    let payload = b"tagged-payload";
    let replay_tag = "fedcba9876543210fedcba9876543210";
    let ack = handle
        .append_with_replay_tag(payload, replay_tag)
        .expect("append tagged payload");
    let records = handle.replay_all().expect("replay tagged payload");
    assert_eq!(records.len(), 1, "single tagged append yields one record");
    assert_eq!(records[0].ack, ack.ack, "tagged record keeps ack position");
    assert_eq!(
        records[0].payload.as_ref(),
        payload,
        "tagged record keeps canonical payload bytes verbatim"
    );
    assert_eq!(
        records[0].schema_tag.as_deref(),
        Some(replay_tag),
        "replay record surfaces the opaque per-message tag"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}

#[test]
#[ignore = "requires nats-server matching tools/.nats-server-version on PATH (mission pardosa-nats-fence-resync-v2)"]
fn live_same_handle_concurrent_appends_serialize_without_fencing() {
    const APPEND_COUNT: usize = 20;
    let server: Arc<LiveNatsServer> = LiveNatsServer::acquire();
    let rt = Runtime::new().expect("tokio runtime");
    let tag = unique_stream_tag();
    let stream_name = format!("PARDOSA_LIVE_{tag}");
    let cfg = build_live_config(&tag, &rt, &server);
    let handle = Arc::new(JetStreamBackend::open(cfg));
    let threads: Vec<_> = (0..APPEND_COUNT)
        .map(|i| {
            let handle = Arc::clone(&handle);
            thread::spawn(move || handle.append(format!("burst-{i}").as_bytes()))
        })
        .collect();
    let mut acks = Vec::new();
    for thread in threads {
        let ack = thread
            .join()
            .expect("append thread joins")
            .expect("same-handle concurrent append must not fence");
        acks.push(ack);
    }
    acks.sort();
    acks.dedup();
    assert_eq!(
        acks.len(),
        APPEND_COUNT,
        "every concurrent same-handle append succeeds with a distinct ack position"
    );
    let records = handle
        .replay_all()
        .expect("replay_all after concurrent burst");
    assert_eq!(
        records.len(),
        APPEND_COUNT,
        "subject tip advances exactly once per append; no append is silently dropped or duplicated"
    );
    rt.block_on(teardown_stream(&server, &stream_name));
}
