use super::BackendSink;
use super::sealed;
use crate::authoritative::jetstream::JetStreamBackendAdapter;
use crate::durability::AckPosition;
use crate::error::{BackendError, BackendOp, RuntimeFailureKind};
use pardosa_nats::{JetStreamAckPosition, JetStreamRuntimeError};
fn map_position(pos: JetStreamAckPosition) -> AckPosition {
    AckPosition::from_u64(pos.as_u64())
}
fn map_runtime_error(err: JetStreamRuntimeError, op: BackendOp) -> BackendError {
    match err {
        JetStreamRuntimeError::Detached => BackendError::RuntimeFailure {
            kind: RuntimeFailureKind::RuntimeShutdown,
        },
        JetStreamRuntimeError::Timeout {
            elapsed,
            configured,
        } => BackendError::Timeout {
            op,
            elapsed,
            configured,
        },
        JetStreamRuntimeError::Publish { source } => BackendError::Publish { source },
        JetStreamRuntimeError::Connect { source } => BackendError::Connect { op, source },
        JetStreamRuntimeError::Replay { source } => BackendError::Replay { op, source },
        other => BackendError::Publish {
            source: Box::new(other),
        },
    }
}
impl sealed::Sealed for JetStreamBackendAdapter {}
impl BackendSink for JetStreamBackendAdapter {
    fn append(&mut self, bytes: &[u8]) -> Result<AckPosition, BackendError> {
        self.handle
            .append(bytes)
            .map(map_position)
            .map_err(|e| map_runtime_error(e, BackendOp::Append))
    }
    fn sync(&mut self) -> Result<AckPosition, BackendError> {
        self.handle
            .sync()
            .map(map_position)
            .map_err(|e| map_runtime_error(e, BackendOp::Sync))
    }
}
impl JetStreamBackendAdapter {
    pub(crate) fn fetch_durable_frames(&mut self) -> Result<Vec<Vec<u8>>, BackendError> {
        let records = self
            .handle
            .replay_all()
            .map_err(|e| map_runtime_error(e, BackendOp::Sync))?;
        Ok(records
            .into_iter()
            .map(|r| r.payload.as_ref().to_vec())
            .collect())
    }
}
fn latest_payload<I>(payloads: I) -> Vec<u8>
where
    I: IntoIterator,
    I::Item: AsRef<[u8]>,
{
    let mut last: Option<I::Item> = None;
    for p in payloads {
        last = Some(p);
    }
    last.map(|p| p.as_ref().to_vec()).unwrap_or_default()
}
impl super::journal::RehydrateableBackend for JetStreamBackendAdapter {
    fn fetch_durable_bytes(&mut self) -> Result<Vec<u8>, BackendError> {
        let frames = self.fetch_durable_frames()?;
        Ok(latest_payload(frames))
    }
}
#[cfg(test)]
mod tests {
    use super::BackendSink;
    use super::JetStreamBackendAdapter;
    use super::map_runtime_error;
    use crate::error::{BackendError, BackendOp, RuntimeFailureKind};
    use pardosa_nats::{
        DEFAULT_OPERATION_TIMEOUT, JetStreamBackend, JetStreamConfig, JetStreamRuntimeError,
        RuntimeHandle,
    };
    use std::time::Duration;
    fn detached_config(tag: &str) -> JetStreamConfig {
        JetStreamConfig::builder()
            .stream_name(format!("backend-{tag}"))
            .subject(format!("backend.{tag}"))
            .durable_consumer(format!("backend-c-{tag}"))
            .runtime_handle(RuntimeHandle::detached_for_tests())
            .build()
            .expect("offline config is valid")
    }
    fn boxed_source(msg: &str) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        Box::new(std::io::Error::other(msg))
    }
    #[test]
    fn detached_maps_to_runtime_failure_runtime_shutdown() {
        let mapped = map_runtime_error(JetStreamRuntimeError::Detached, BackendOp::Append);
        assert!(
            matches!(
                mapped,
                BackendError::RuntimeFailure {
                    kind: RuntimeFailureKind::RuntimeShutdown
                }
            ),
            "Detached must map to RuntimeFailure{{RuntimeShutdown}}, got {mapped:?}",
        );
    }
    #[test]
    fn timeout_maps_to_backend_timeout_with_v0_configured() {
        let elapsed = Duration::from_secs(31);
        let configured = Duration::from_secs(45);
        let mapped = map_runtime_error(
            JetStreamRuntimeError::Timeout {
                elapsed,
                configured,
            },
            BackendOp::Sync,
        );
        match mapped {
            BackendError::Timeout {
                op,
                elapsed: e,
                configured,
            } => {
                assert!(matches!(op, BackendOp::Sync), "op preserved");
                assert_eq!(e, elapsed, "elapsed preserved");
                assert_eq!(configured, Duration::from_secs(45), "configured preserved");
            }
            other => panic!("expected BackendError::Timeout, got {other:?}"),
        }
    }
    #[test]
    fn publish_maps_to_backend_publish_preserving_source() {
        let mapped = map_runtime_error(
            JetStreamRuntimeError::Publish {
                source: boxed_source("publish-failed"),
            },
            BackendOp::Append,
        );
        match mapped {
            BackendError::Publish { source } => {
                assert!(
                    source.to_string().contains("publish-failed"),
                    "source preserved: {source}",
                );
            }
            other => panic!("expected BackendError::Publish, got {other:?}"),
        }
    }
    #[test]
    fn connect_maps_to_backend_connect_preserving_source() {
        let mapped = map_runtime_error(
            JetStreamRuntimeError::Connect {
                source: boxed_source("connect-failed"),
            },
            BackendOp::Append,
        );
        match mapped {
            BackendError::Connect { op, source } => {
                assert!(matches!(op, BackendOp::Append), "op preserved");
                assert!(
                    source.to_string().contains("connect-failed"),
                    "source preserved: {source}",
                );
            }
            other => panic!("expected BackendError::Connect, got {other:?}"),
        }
    }
    #[test]
    fn replay_maps_to_backend_replay_preserving_inner_source_directly() {
        let mapped = map_runtime_error(
            JetStreamRuntimeError::Replay {
                source: boxed_source("replay-failed"),
            },
            BackendOp::Sync,
        );
        match mapped {
            BackendError::Replay { op, source } => {
                assert!(matches!(op, BackendOp::Sync), "op preserved");
                assert_eq!(
                    source.to_string(),
                    "replay-failed",
                    "source preserved without wrapping",
                );
            }
            other => panic!("expected BackendError::Replay, got {other:?}"),
        }
    }
    #[test]
    fn v0_operation_timeout_matches_substrate_default() {
        assert_eq!(
            DEFAULT_OPERATION_TIMEOUT,
            Duration::from_secs(30),
            "v0 substrate timeout default remains 30s",
        );
    }
    #[test]
    fn adapter_append_on_detached_handle_returns_runtime_failure() {
        let handle = JetStreamBackend::open(detached_config("append-detached"));
        let mut adapter = JetStreamBackendAdapter::new(handle);
        let err = adapter
            .append(b"any-bytes")
            .expect_err("detached handle must not append");
        assert!(
            matches!(
                err,
                BackendError::RuntimeFailure {
                    kind: RuntimeFailureKind::RuntimeShutdown
                }
            ),
            "detached append surface must be RuntimeFailure{{RuntimeShutdown}}, got {err:?}",
        );
    }
    #[test]
    fn adapter_sync_on_detached_handle_returns_runtime_failure() {
        let handle = JetStreamBackend::open(detached_config("sync-detached"));
        let mut adapter = JetStreamBackendAdapter::new(handle);
        let err = adapter.sync().expect_err("detached handle must not sync");
        assert!(
            matches!(
                err,
                BackendError::RuntimeFailure {
                    kind: RuntimeFailureKind::RuntimeShutdown
                }
            ),
            "detached sync surface must be RuntimeFailure{{RuntimeShutdown}}, got {err:?}",
        );
    }
    #[test]
    fn adapter_fetch_durable_bytes_on_detached_handle_returns_runtime_failure() {
        use crate::backend::journal::RehydrateableBackend;
        let handle = JetStreamBackend::open(detached_config("fetch-detached"));
        let mut adapter = JetStreamBackendAdapter::new(handle);
        let err = adapter
            .fetch_durable_bytes()
            .expect_err("detached handle must not fetch");
        assert!(
            matches!(
                err,
                BackendError::RuntimeFailure {
                    kind: RuntimeFailureKind::RuntimeShutdown
                }
            ),
            "detached fetch surface must be RuntimeFailure{{RuntimeShutdown}}, got {err:?}",
        );
    }
    #[test]
    fn latest_payload_on_empty_records_returns_empty_bytes() {
        let empty: Vec<Vec<u8>> = Vec::new();
        let out = super::latest_payload(empty);
        assert!(
            out.is_empty(),
            "an empty JetStream replay must rehydrate to an empty byte blob \
             (no records => fresh dragline; ADR-0022 §D2 reader-side seam; \
             mission event-storage-dual-backend-04 success_criteria #2 \
             empty-stream characterisation)",
        );
    }
    #[test]
    fn latest_payload_returns_last_record_payload_and_discards_prior_generations() {
        let stale_gen_1 = b"stale-pgno-blob-generation-1".to_vec();
        let stale_gen_2 = b"stale-pgno-blob-generation-2-with-trailing-bytes".to_vec();
        let latest_gen = b"latest-pgno-blob-this-is-the-authoritative-one".to_vec();
        let out = super::latest_payload(vec![
            stale_gen_1.clone(),
            stale_gen_2.clone(),
            latest_gen.clone(),
        ]);
        assert_eq!(
            out, latest_gen,
            "latest-message-wins: each JetStreamRecoveryJournal::sync publishes \
             one complete .pgno blob; the rehydrate seam must pick the last \
             record's payload verbatim and ignore earlier (now-stale) \
             generations of the same dragline (ADR-0022 §D2 sync-as-fence; \
             mission event-storage-dual-backend-04 success_criteria #2 \
             latest-message-wins characterisation)",
        );
        assert_ne!(
            out, stale_gen_1,
            "latest must not be the first stale generation",
        );
        assert_ne!(
            out, stale_gen_2,
            "latest must not be a middle stale generation",
        );
    }
    #[test]
    fn jetstream_seam_rehydrate_byte_parity_with_pgno_for_same_event_sequence() {
        use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
        use crate::dragline::Line;
        use crate::persist::persist_with_source;
        use std::io::Cursor;
        let mut line: Line<u64> = Line::with_anchor_config(
            "sub-04-jetstream-pgno-byte-parity".to_owned(),
            1,
            DEFAULT_ANCHOR_BUFFER_CAP,
        );
        for i in 0..5u64 {
            let _ = line.create(i).expect("commit reference event");
        }
        let original_line: Vec<u64> = line.read_line().iter().map(|e| *e.domain_event()).collect();
        let mut pgno_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        persist_with_source(&line, &mut pgno_sink, None)
            .expect("persist canonical .pgno blob (reference path)");
        let canonical_pgno_bytes: Vec<u8> = pgno_sink.into_inner();
        assert!(
            !canonical_pgno_bytes.is_empty(),
            "preflight: persist_with_source must produce a non-empty .pgno blob",
        );
        let earlier_stale_generation: Vec<u8> = {
            let mut prefix_line: Line<u64> = Line::with_anchor_config(
                "sub-04-jetstream-pgno-byte-parity".to_owned(),
                1,
                DEFAULT_ANCHOR_BUFFER_CAP,
            );
            for i in 0..2u64 {
                let _ = prefix_line
                    .create(i)
                    .expect("commit earlier-generation event");
            }
            let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
            persist_with_source(&prefix_line, &mut sink, None)
                .expect("persist earlier-generation .pgno blob");
            sink.into_inner()
        };
        let rehydrate_bytes = super::latest_payload(vec![
            earlier_stale_generation.clone(),
            canonical_pgno_bytes.clone(),
        ]);
        assert_eq!(
            rehydrate_bytes, canonical_pgno_bytes,
            "the bytes the JetStream rehydrate seam selects must be byte-identical \
             to the canonical .pgno blob the writer last sync-fenced (ADR-0022 \
             §D5 canonical bytes verbatim)",
        );
        let rehydrated: Line<u64> =
            crate::backend::rehydrate::from_pgno_bytes_unchecked::<u64>(&rehydrate_bytes)
                .expect("rehydrate via approved byte seam");
        let recovered_line: Vec<u64> = rehydrated
            .read_line()
            .iter()
            .map(|e| *e.domain_event())
            .collect();
        assert_eq!(
            recovered_line, original_line,
            "byte/state parity: feeding the JetStream rehydrate seam's selected \
             bytes into the approved from_pgno_bytes_unchecked rehydrate path \
             must reproduce the same event line as the .pgno writer would on \
             reopen (mission event-storage-dual-backend-04 success_criteria #2 \
             byte/state parity vs .pgno for the same canonical event sequence)",
        );
        let pgno_reopened: Line<u64> =
            crate::backend::rehydrate::from_pgno_bytes_unchecked::<u64>(&canonical_pgno_bytes)
                .expect("rehydrate the canonical .pgno blob directly");
        let pgno_reopened_line: Vec<u64> = pgno_reopened
            .read_line()
            .iter()
            .map(|e| *e.domain_event())
            .collect();
        assert_eq!(
            recovered_line, pgno_reopened_line,
            "JetStream-seam-rehydrated event line must equal the .pgno-direct \
             rehydrate of the identical canonical blob (cross-path parity)",
        );
    }
    #[test]
    fn jetstream_seam_rehydrate_event_id_derives_from_canonical_bytes_not_substrate_position() {
        use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
        use crate::dragline::Line;
        use crate::event::EventId;
        use crate::persist::persist_with_source;
        use std::io::Cursor;
        let mut line: Line<u64> = Line::with_anchor_config(
            "sub-04-jetstream-eventid-from-bytes".to_owned(),
            1,
            DEFAULT_ANCHOR_BUFFER_CAP,
        );
        for i in 0..4u64 {
            let _ = line.create(i).expect("commit event");
        }
        let original_event_ids: Vec<EventId> = line
            .read_line()
            .iter()
            .map(crate::event::Event::event_id)
            .collect();
        let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        persist_with_source(&line, &mut sink, None).expect("persist canonical .pgno blob");
        let canonical_bytes: Vec<u8> = sink.into_inner();
        let bytes_from_seq_3 = super::latest_payload(vec![
            b"stale-1".to_vec(),
            b"stale-2".to_vec(),
            canonical_bytes.clone(),
        ]);
        let bytes_from_seq_10 = super::latest_payload(vec![
            b"stale-1".to_vec(),
            b"stale-2".to_vec(),
            b"stale-3".to_vec(),
            b"stale-4".to_vec(),
            b"stale-5".to_vec(),
            b"stale-6".to_vec(),
            b"stale-7".to_vec(),
            b"stale-8".to_vec(),
            b"stale-9".to_vec(),
            canonical_bytes.clone(),
        ]);
        assert_eq!(
            bytes_from_seq_3, bytes_from_seq_10,
            "preflight: the rehydrate seam selects on canonical-bytes equality, \
             not on substrate sequence position",
        );
        let rehydrated_from_seq_3: Line<u64> =
            crate::backend::rehydrate::from_pgno_bytes_unchecked::<u64>(&bytes_from_seq_3)
                .expect("rehydrate from seq-3 selection");
        let rehydrated_from_seq_10: Line<u64> =
            crate::backend::rehydrate::from_pgno_bytes_unchecked::<u64>(&bytes_from_seq_10)
                .expect("rehydrate from seq-10 selection");
        let ids_from_seq_3: Vec<EventId> = rehydrated_from_seq_3
            .read_line()
            .iter()
            .map(crate::event::Event::event_id)
            .collect();
        let ids_from_seq_10: Vec<EventId> = rehydrated_from_seq_10
            .read_line()
            .iter()
            .map(crate::event::Event::event_id)
            .collect();
        assert_eq!(
            ids_from_seq_3, original_event_ids,
            "EventIds rehydrated through the JetStream seam must derive from the \
             canonical .pgno bytes (event-line position), not from the JetStream \
             substrate's ack-position / message sequence (mission \
             event-storage-dual-backend-04 success_criteria #3 EventId-from-bytes; \
             ADR-0022 §D5)",
        );
        assert_eq!(
            ids_from_seq_3, ids_from_seq_10,
            "EventIds must be byte-derived: identical canonical bytes selected \
             from a stream-position-3 record vs a stream-position-10 record \
             produce identical EventIds; if EventIds tracked JetStream sequence, \
             these vectors would differ — they must not",
        );
    }
}
