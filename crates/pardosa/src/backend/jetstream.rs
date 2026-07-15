//! JetStream backend telemetry: emits `pardosa::jetstream::metrics`
//! `info!` events consumed by a log-based-metric aggregation layer.
//! Backend naming + SLO/alerting bands: `docs/pardosa/observability-slo.md`.

use super::BackendSink;
use super::sealed;
use crate::authoritative::jetstream::JetStreamBackendAdapter;
use crate::durability::AckPosition;
use crate::error::{BackendError, BackendOp, RuntimeFailureKind};
use pardosa_nats::{JetStreamAckPosition, JetStreamRuntimeError};
use std::time::{Duration, Instant};
use tracing::{info, info_span};

#[derive(Clone, Debug)]
pub(crate) struct JetStreamDurableFrame {
    pub(crate) payload: Vec<u8>,
    pub(crate) schema_tag: Option<String>,
}

impl AsRef<[u8]> for JetStreamDurableFrame {
    fn as_ref(&self) -> &[u8] {
        &self.payload
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TelemetryOp {
    Append,
    Sync,
    Replay,
}

impl TelemetryOp {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Sync => "sync",
            Self::Replay => "replay",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalCategory {
    Ok,
    Timeout,
    Connect,
    Replay,
    Publish,
    FenceConflict,
    RuntimeFailure,
}

impl TerminalCategory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Replay => "replay",
            Self::Publish => "publish",
            Self::FenceConflict => "fence_conflict",
            Self::RuntimeFailure => "runtime_failure",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MetricKind {
    Counter,
    Histogram,
}

impl MetricKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Histogram => "histogram",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MetricLabelSpec {
    name: &'static str,
    values: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MetricSpec {
    name: &'static str,
    kind: MetricKind,
    labels: &'static [MetricLabelSpec],
}

const OP_LABEL_VALUES: &[&str] = &["append", "sync", "replay"];
const TERMINAL_CATEGORY_LABEL_VALUES: &[&str] = &[
    "ok",
    "timeout",
    "connect",
    "replay",
    "publish",
    "fence_conflict",
    "runtime_failure",
];
const OP_LABEL: MetricLabelSpec = MetricLabelSpec {
    name: "op",
    values: OP_LABEL_VALUES,
};
const METRIC_LABELS: &[MetricLabelSpec] = &[
    OP_LABEL,
    MetricLabelSpec {
        name: "terminal_category",
        values: TERMINAL_CATEGORY_LABEL_VALUES,
    },
];
const OP_ONLY_LABELS: &[MetricLabelSpec] = &[OP_LABEL];
const OPERATION_TERMINAL_COUNTER: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_operation_terminal_total",
    kind: MetricKind::Counter,
    labels: METRIC_LABELS,
};
const APPEND_LATENCY_HISTOGRAM: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_append_latency_seconds",
    kind: MetricKind::Histogram,
    labels: METRIC_LABELS,
};
/// I5: bridge (`block_on`) duration observed for every op, not only
/// append — extends `APPEND_LATENCY_HISTOGRAM` (kept, unchanged, for
/// backward compatibility) rather than replacing it.
const BRIDGE_DURATION_HISTOGRAM: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_bridge_duration_seconds",
    kind: MetricKind::Histogram,
    labels: METRIC_LABELS,
};
/// I5: `block_on` bridge timeout counter, all ops.
const ACK_TIMEOUT_COUNTER: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_ack_timeout_total",
    kind: MetricKind::Counter,
    labels: OP_ONLY_LABELS,
};
/// I2: `ConcurrencyConflict` surfaced to the caller. Adapter-boundary
/// limit (documented in `docs/pardosa/observability-slo.md`): whether
/// the caller performs a clean abort afterward is not observable from
/// here, so this counts conflict-surfaced-to-caller, not
/// conflict-then-non-abort.
///
/// This is the GND-0010:R7 surfacing point in the adapter ring: a
/// silent consistency violation must become an observable signal
/// (COM-0019), and this counter is that signal for OCC fence conflicts.
const OCC_CONFLICT_UNHANDLED_COUNTER: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_occ_conflict_unhandled_total",
    kind: MetricKind::Counter,
    labels: OP_ONLY_LABELS,
};
/// I2b: intra-handle self-fence. `append_gate` is a `Semaphore(1)`
/// (`pardosa-nats/src/handle.rs`), which makes this structurally
/// unreachable via the public API; the counter exists so its healthy
/// value (zero) is an observable invariant, not an absence of
/// instrumentation.
const OCC_SELF_FENCE_COUNTER: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_occ_self_fence_total",
    kind: MetricKind::Counter,
    labels: OP_ONLY_LABELS,
};
/// I8: dedup-hit signal. Fires whenever `PublishAck.duplicate` (async-nats
/// 0.49.1, threaded across the crate boundary via
/// [`pardosa_nats::JetStreamAppendAck::duplicate`]) reports the `JetStream`
/// server's own `Nats-Msg-Id` dedup window collapsed a re-publish. Recorded
/// here, in the pardosa adapter ring, not in `pardosa-nats`
/// (substrate ring purity, COM-0019:R6). Healthy band: bounded, only from
/// legitimate retries (`docs/pardosa/observability-slo.md`).
const DEDUP_HIT_COUNTER: MetricSpec = MetricSpec {
    name: "pardosa_jetstream_dedup_hit_total",
    kind: MetricKind::Counter,
    labels: OP_ONLY_LABELS,
};
const REGISTERED_METRICS: &[MetricSpec] = &[
    OPERATION_TERMINAL_COUNTER,
    APPEND_LATENCY_HISTOGRAM,
    BRIDGE_DURATION_HISTOGRAM,
    ACK_TIMEOUT_COUNTER,
    OCC_CONFLICT_UNHANDLED_COUNTER,
    OCC_SELF_FENCE_COUNTER,
    DEDUP_HIT_COUNTER,
];

#[derive(Clone, Copy, Debug)]
struct OperationTelemetry {
    op: TelemetryOp,
    payload_size_bytes: Option<usize>,
}

impl OperationTelemetry {
    const fn append(payload_size_bytes: usize) -> Self {
        Self {
            op: TelemetryOp::Append,
            payload_size_bytes: Some(payload_size_bytes),
        }
    }

    const fn sync() -> Self {
        Self {
            op: TelemetryOp::Sync,
            payload_size_bytes: None,
        }
    }
}

fn registered_metrics() -> &'static [MetricSpec] {
    REGISTERED_METRICS
}

fn terminal_category_from_backend(err: &BackendError) -> TerminalCategory {
    match err {
        BackendError::Timeout { .. } => TerminalCategory::Timeout,
        BackendError::RuntimeFailure { .. } => TerminalCategory::RuntimeFailure,
        BackendError::ConcurrencyConflict { .. } => TerminalCategory::FenceConflict,
        BackendError::Publish { .. } | BackendError::PublisherBacklog { .. } => {
            TerminalCategory::Publish
        }
        BackendError::Connect { .. } => TerminalCategory::Connect,
        BackendError::Replay { .. } => TerminalCategory::Replay,
    }
}

fn duration_seconds(d: Duration) -> f64 {
    d.as_secs_f64()
}

fn record_metric(
    spec: MetricSpec,
    op: TelemetryOp,
    terminal_category: TerminalCategory,
    value: f64,
) {
    assert!(
        registered_metrics()
            .iter()
            .any(|registered| registered == &spec),
        "metric must be registered before emission"
    );
    info!(
        target: "pardosa::jetstream::metrics",
        metric_name = spec.name,
        metric_kind = spec.kind.as_str(),
        metric_value = value,
        op = op.as_str(),
        terminal_category = terminal_category.as_str(),
        "pardosa jetstream metric"
    );
}

fn record_op_only_metric(spec: MetricSpec, op: TelemetryOp, value: f64) {
    assert!(
        registered_metrics()
            .iter()
            .any(|registered| registered == &spec),
        "metric must be registered before emission"
    );
    info!(
        target: "pardosa::jetstream::metrics",
        metric_name = spec.name,
        metric_kind = spec.kind.as_str(),
        metric_value = value,
        op = op.as_str(),
        "pardosa jetstream metric"
    );
}

/// I2b: emits the self-fence counter. `append_gate`'s `Semaphore(1)`
/// (`pardosa-nats/src/handle.rs`) makes intra-handle self-fence
/// structurally unreachable via any public entry point, so this
/// function has no call site on the normal path — it exists so the
/// registered metric's healthy value (zero, never emitted) is an
/// observable invariant rather than an absent signal.
#[expect(
    dead_code,
    reason = "I2b defensive counter path; Semaphore(1) makes self-fence \
              structurally unreachable today, kept registered for \
              defense-in-depth per adr-fmt-2ysyq D.3(b)"
)]
fn record_self_fence(op: TelemetryOp) {
    record_op_only_metric(OCC_SELF_FENCE_COUNTER, op, 1.0);
}

/// I8: emits the dedup-hit counter when the `JetStream` server reported
/// `PublishAck.duplicate == true` for this operation. No-op otherwise, so
/// the counter's healthy band (bounded, legitimate-retry-only) stays
/// observable per `docs/pardosa/observability-slo.md`.
fn record_append_outcome_metrics(op: TelemetryOp, duplicate: bool) {
    if duplicate {
        record_op_only_metric(DEDUP_HIT_COUNTER, op, 1.0);
    }
}

fn record_operation_metrics(
    op: TelemetryOp,
    terminal_category: TerminalCategory,
    elapsed: Duration,
) {
    record_metric(OPERATION_TERMINAL_COUNTER, op, terminal_category, 1.0);
    record_metric(
        BRIDGE_DURATION_HISTOGRAM,
        op,
        terminal_category,
        duration_seconds(elapsed),
    );
    if matches!(op, TelemetryOp::Append) {
        record_metric(
            APPEND_LATENCY_HISTOGRAM,
            op,
            terminal_category,
            duration_seconds(elapsed),
        );
    }
    if matches!(terminal_category, TerminalCategory::Timeout) {
        record_op_only_metric(ACK_TIMEOUT_COUNTER, op, 1.0);
    }
    if matches!(terminal_category, TerminalCategory::FenceConflict) {
        record_op_only_metric(OCC_CONFLICT_UNHANDLED_COUNTER, op, 1.0);
    }
}

fn observe_operation(
    fields: OperationTelemetry,
    run: impl FnOnce() -> Result<AckPosition, BackendError>,
) -> Result<AckPosition, BackendError> {
    let start = Instant::now();
    let payload_size = fields.payload_size_bytes.unwrap_or(0);
    let span = info_span!(
        "pardosa.jetstream.operation",
        op = fields.op.as_str(),
        payload_size_bytes = payload_size,
        replay_record_count = tracing::field::Empty,
        latency_seconds = tracing::field::Empty,
        terminal_category = tracing::field::Empty,
    );
    span.in_scope(|| {
        info!(
            phase = "entry",
            op = fields.op.as_str(),
            "pardosa jetstream backend entry"
        );
    });
    let result = run();
    let terminal_category = result
        .as_ref()
        .map_or_else(terminal_category_from_backend, |_| TerminalCategory::Ok);
    let elapsed = start.elapsed();
    span.record("latency_seconds", duration_seconds(elapsed));
    span.record("terminal_category", terminal_category.as_str());
    if let Ok(ack) = result.as_ref() {
        span.record("ack_position", ack.as_u64());
    }
    span.in_scope(|| {
        if let Ok(ack) = result.as_ref() {
            info!(
                phase = "completion",
                op = fields.op.as_str(),
                terminal_category = terminal_category.as_str(),
                latency_seconds = duration_seconds(elapsed),
                ack_position = ack.as_u64(),
                "pardosa jetstream backend completion"
            );
        } else {
            info!(
                phase = "completion",
                op = fields.op.as_str(),
                terminal_category = terminal_category.as_str(),
                latency_seconds = duration_seconds(elapsed),
                "pardosa jetstream backend completion"
            );
        }
    });
    record_operation_metrics(fields.op, terminal_category, elapsed);
    result
}

fn observe_replay_operation(
    run: impl FnOnce() -> Result<Vec<pardosa_nats::JetStreamReplayRecord>, BackendError>,
) -> Result<Vec<pardosa_nats::JetStreamReplayRecord>, BackendError> {
    let start = Instant::now();
    let span = info_span!(
        "pardosa.jetstream.operation",
        op = TelemetryOp::Replay.as_str(),
        payload_size_bytes = 0,
        replay_record_count = tracing::field::Empty,
        latency_seconds = tracing::field::Empty,
        terminal_category = tracing::field::Empty,
    );
    span.in_scope(|| {
        info!(
            phase = "entry",
            op = TelemetryOp::Replay.as_str(),
            "pardosa jetstream backend entry"
        );
    });
    let result = run();
    let terminal_category = result
        .as_ref()
        .map_or_else(terminal_category_from_backend, |_| TerminalCategory::Ok);
    let elapsed = start.elapsed();
    span.record("latency_seconds", duration_seconds(elapsed));
    span.record("terminal_category", terminal_category.as_str());
    if let Ok(records) = result.as_ref() {
        span.record("replay_record_count", records.len());
    }
    span.in_scope(|| {
        if let Ok(records) = result.as_ref() {
            info!(
                phase = "completion",
                op = TelemetryOp::Replay.as_str(),
                terminal_category = terminal_category.as_str(),
                latency_seconds = duration_seconds(elapsed),
                replay_record_count = records.len(),
                "pardosa jetstream backend completion"
            );
        } else {
            info!(
                phase = "completion",
                op = TelemetryOp::Replay.as_str(),
                terminal_category = terminal_category.as_str(),
                latency_seconds = duration_seconds(elapsed),
                "pardosa jetstream backend completion"
            );
        }
    });
    record_operation_metrics(TelemetryOp::Replay, terminal_category, elapsed);
    result
}
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
        JetStreamRuntimeError::WrongLastSequence { source } => {
            BackendError::ConcurrencyConflict { source }
        }
        JetStreamRuntimeError::Publish { source } => BackendError::Publish { op, source },
        JetStreamRuntimeError::Connect { source } => BackendError::Connect { op, source },
        JetStreamRuntimeError::Replay { source } => BackendError::Replay { op, source },
        other => BackendError::Publish {
            op,
            source: Box::new(other),
        },
    }
}
impl sealed::Sealed for JetStreamBackendAdapter {}
impl BackendSink for JetStreamBackendAdapter {
    fn append(&mut self, bytes: &[u8]) -> Result<AckPosition, BackendError> {
        observe_operation(OperationTelemetry::append(bytes.len()), || {
            let outcome = match self.schema_tag.as_deref() {
                Some(schema_tag) => self.handle.append_with_replay_tag(bytes, schema_tag),
                None => self.handle.append(bytes),
            }
            .map_err(|e| map_runtime_error(e, BackendOp::Append))?;
            record_append_outcome_metrics(TelemetryOp::Append, outcome.duplicate);
            Ok(map_position(outcome.ack))
        })
    }
    fn sync(&mut self) -> Result<AckPosition, BackendError> {
        observe_operation(OperationTelemetry::sync(), || {
            self.handle
                .sync()
                .map(map_position)
                .map_err(|e| map_runtime_error(e, BackendOp::Sync))
        })
    }
}
impl JetStreamBackendAdapter {
    pub(crate) fn read_stream_description(&self) -> Result<Option<String>, BackendError> {
        self.handle
            .read_stream_description()
            .map_err(|e| map_runtime_error(e, BackendOp::Open))
    }

    pub(crate) fn fetch_durable_frames(
        &mut self,
    ) -> Result<Vec<JetStreamDurableFrame>, BackendError> {
        let records = observe_replay_operation(|| {
            self.handle
                .replay_all()
                .map_err(|e| map_runtime_error(e, BackendOp::Open))
        })?;
        Ok(records
            .into_iter()
            .map(|r| JetStreamDurableFrame {
                payload: r.payload.as_ref().to_vec(),
                schema_tag: r.schema_tag,
            })
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
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone, Default)]
    struct VecWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl VecWriter {
        fn snapshot(&self) -> String {
            String::from_utf8(self.buf.lock().unwrap().clone()).expect("utf-8")
        }
    }

    impl Write for VecWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn capture_tracing(f: impl FnOnce()) -> String {
        let writer = VecWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_target(false)
            .with_level(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        writer.snapshot()
    }
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
            BackendError::Publish { op, source } => {
                assert!(matches!(op, BackendOp::Append), "op preserved");
                assert!(
                    source.to_string().contains("publish-failed"),
                    "source preserved: {source}",
                );
            }
            other => panic!("expected BackendError::Publish, got {other:?}"),
        }
    }
    #[test]
    fn wrong_last_sequence_maps_to_backend_concurrency_conflict() {
        let mapped = map_runtime_error(
            JetStreamRuntimeError::WrongLastSequence {
                source: boxed_source("stale writer"),
            },
            BackendOp::Append,
        );
        match mapped {
            BackendError::ConcurrencyConflict { source } => {
                assert!(
                    source.to_string().contains("stale writer"),
                    "conflict source preserved: {source}"
                );
            }
            other => panic!("expected BackendError::ConcurrencyConflict, got {other:?}"),
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
    fn open_gate_read_replay_failure_maps_to_backend_replay_preserving_open_op() {
        let mapped = map_runtime_error(
            JetStreamRuntimeError::Replay {
                source: boxed_source("stream marker read failed"),
            },
            BackendOp::Open,
        );
        match mapped {
            BackendError::Replay { op, source } => {
                assert!(
                    matches!(op, BackendOp::Open),
                    "read_stream_description/fetch_durable_frames open-gate reads must tag \
                     BackendOp::Open, not the append-time Sync durability fence",
                );
                assert_eq!(
                    source.to_string(),
                    "stream marker read failed",
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

    fn assert_metric_schema_bounded(metrics: &[super::MetricSpec]) {
        assert!(
            metrics
                .iter()
                .any(|metric| matches!(metric.kind, super::MetricKind::Counter)),
            "at least one counter must be registered",
        );
        assert!(
            metrics
                .iter()
                .any(|metric| matches!(metric.kind, super::MetricKind::Histogram)),
            "at least one histogram must be registered",
        );
        for metric in metrics {
            let label_names: Vec<&str> = metric.labels.iter().map(|label| label.name).collect();
            assert!(
                label_names == vec!["op", "terminal_category"] || label_names == vec!["op"],
                "metric `{}` label names must stay bounded, got {label_names:?}",
                metric.name,
            );
            for label in metric.labels {
                assert!(
                    label.values.len() <= 7,
                    "metric `{}` label `{}` value set must stay bounded",
                    metric.name,
                    label.name,
                );
            }
        }
        let total_series: usize = metrics
            .iter()
            .map(|metric| {
                metric
                    .labels
                    .iter()
                    .map(|label| label.values.len())
                    .product::<usize>()
            })
            .sum();
        assert!(
            total_series <= 500,
            "COM-0019:R6 bound: total series across registered metrics must stay <= 500, got {total_series}",
        );
    }

    #[test]
    fn jetstream_adapter_observability_emits_spans_metrics_and_bounded_labels() {
        use crate::backend::journal::RehydrateableBackend;
        let metrics = super::registered_metrics();
        assert_metric_schema_bounded(metrics);
        let captured = capture_tracing(|| {
            let handle = JetStreamBackend::open(detached_config("observability"));
            let mut adapter = JetStreamBackendAdapter::new(handle);
            let _ = adapter.append(b"event-bytes");
            let _ = adapter.sync();
            let _ = adapter.fetch_durable_bytes();
        });
        for needle in [
            "op=\"append\"",
            "op=\"sync\"",
            "op=\"replay\"",
            "phase=\"entry\"",
            "phase=\"completion\"",
            "terminal_category=\"runtime_failure\"",
            "metric_kind=\"counter\"",
            "metric_kind=\"histogram\"",
            "payload_size_bytes=11",
        ] {
            assert!(
                captured.contains(needle),
                "JetStream observability output missing `{needle}`; captured={captured:?}",
            );
        }
        for forbidden in [
            "op=\"connect\"",
            "event_id=",
            "AckPosition=",
            "ack=",
            "stream=backend-observability",
            "subject=backend.observability",
            "correlation_id=",
            "fiber_id=",
        ] {
            assert!(
                !captured.contains(forbidden),
                "metric labels must stay bounded and span-only ids must not appear as labels: `{forbidden}` in {captured:?}",
            );
        }
        for metric_line in captured
            .lines()
            .filter(|line| line.contains("pardosa jetstream metric"))
        {
            for forbidden in [
                "payload_size_bytes=",
                "replay_record_count=",
                "ack_position=",
                "event_id=",
                "correlation_id=",
                "fiber_id=",
                "stream=",
                "subject=",
            ] {
                assert!(
                    !metric_line.contains(forbidden),
                    "metric event must carry only bounded labels; `{forbidden}` in {metric_line:?}",
                );
            }
        }
    }

    #[test]
    fn jetstream_adapter_observability_success_paths_emit_ok_completion() {
        let captured = capture_tracing(|| {
            let _ = super::observe_operation(super::OperationTelemetry::append(7), || {
                Ok(crate::durability::AckPosition::from_u64(13))
            });
            let _ = super::observe_operation(super::OperationTelemetry::sync(), || {
                Ok(crate::durability::AckPosition::from_u64(21))
            });
            let _ = super::observe_replay_operation(|| Ok(Vec::new()));
        });
        for needle in [
            "op=\"append\"",
            "op=\"sync\"",
            "op=\"replay\"",
            "phase=\"entry\"",
            "phase=\"completion\"",
            "terminal_category=\"ok\"",
            "ack_position=13",
            "ack_position=21",
            "replay_record_count=0",
        ] {
            assert!(
                captured.contains(needle),
                "JetStream success observability output missing `{needle}`; captured={captured:?}",
            );
        }
        assert!(
            !captured.contains("op=\"connect\""),
            "no connect span is emitted on the append/replay hot path; the genuine \
             lazy connect occurs once inside the substrate, not per operation; \
             captured={captured:?}",
        );
    }

    #[test]
    fn jetstream_observability_terminal_category_vocabulary_matches_backend_taxonomy() {
        use crate::error::PublisherBacklogKind;
        let timeout = BackendError::Timeout {
            op: BackendOp::Append,
            elapsed: Duration::from_secs(2),
            configured: Duration::from_secs(1),
        };
        let runtime_failure = BackendError::RuntimeFailure {
            kind: RuntimeFailureKind::RuntimeShutdown,
        };
        let publish = BackendError::Publish {
            op: BackendOp::Append,
            source: boxed_source("publish"),
        };
        let backlog = BackendError::PublisherBacklog {
            kind: PublisherBacklogKind::CapExceeded,
        };
        let connect = BackendError::Connect {
            op: BackendOp::Append,
            source: boxed_source("connect"),
        };
        let replay = BackendError::Replay {
            op: BackendOp::Sync,
            source: boxed_source("replay"),
        };
        let conflict = BackendError::ConcurrencyConflict {
            source: boxed_source("wrong last sequence"),
        };
        let observed = [
            super::terminal_category_from_backend(&timeout).as_str(),
            super::terminal_category_from_backend(&runtime_failure).as_str(),
            super::terminal_category_from_backend(&publish).as_str(),
            super::terminal_category_from_backend(&backlog).as_str(),
            super::terminal_category_from_backend(&connect).as_str(),
            super::terminal_category_from_backend(&replay).as_str(),
            super::terminal_category_from_backend(&conflict).as_str(),
        ];
        assert_eq!(
            observed,
            [
                "timeout",
                "runtime_failure",
                "publish",
                "publish",
                "connect",
                "replay",
                "fence_conflict",
            ],
            "terminal telemetry categories must match BackendError taxonomy; \
             ConcurrencyConflict must be `fence_conflict`, distinct from `publish` (I1)",
        );
        let terminal_values = super::REGISTERED_METRICS[0].labels[1].values;
        assert_eq!(
            terminal_values,
            [
                "ok",
                "timeout",
                "connect",
                "replay",
                "publish",
                "fence_conflict",
                "runtime_failure",
            ],
            "metric terminal_category label values must stay bounded to T2 categories plus ok",
        );
    }

    #[test]
    fn i1_fence_conflict_increments_fence_conflict_path_not_publish_path() {
        let captured = capture_tracing(|| {
            let _ = super::observe_operation(super::OperationTelemetry::append(4), || {
                Err(BackendError::ConcurrencyConflict {
                    source: boxed_source("wrong last sequence"),
                })
            });
        });
        let metric_lines: Vec<&str> = captured
            .lines()
            .filter(|line| line.contains("pardosa jetstream metric"))
            .collect();
        assert!(
            metric_lines.iter().any(|line| {
                line.contains("terminal_category=\"fence_conflict\"")
                    && line.contains("metric_name=\"pardosa_jetstream_operation_terminal_total\"")
            }),
            "I1: ConcurrencyConflict must increment the terminal counter with \
             terminal_category=fence_conflict; captured={metric_lines:?}",
        );
        assert!(
            !metric_lines.iter().any(|line| {
                line.contains("metric_name=\"pardosa_jetstream_operation_terminal_total\"")
                    && line.contains("terminal_category=\"publish\"")
            }),
            "I1: ConcurrencyConflict must NOT increment the generic publish path; \
             captured={metric_lines:?}",
        );
    }

    #[test]
    fn i2_conflict_unhandled_counter_fires_on_fence_conflict_surfaced_to_caller() {
        let captured = capture_tracing(|| {
            let _ = super::observe_operation(super::OperationTelemetry::append(4), || {
                Err(BackendError::ConcurrencyConflict {
                    source: boxed_source("wrong last sequence"),
                })
            });
        });
        assert!(
            captured.contains("metric_name=\"pardosa_jetstream_occ_conflict_unhandled_total\""),
            "I2: conflict_unhandled counter must fire when a fence conflict surfaces \
             to the caller (adapter-boundary scope: this repo cannot observe whether \
             the caller then performs a clean abort); captured={captured:?}",
        );
    }

    #[test]
    fn i8_dedup_hit_counter_fires_when_publish_ack_reports_duplicate() {
        let captured = capture_tracing(|| {
            super::record_append_outcome_metrics(super::TelemetryOp::Append, true);
        });
        assert!(
            captured.contains("metric_name=\"pardosa_jetstream_dedup_hit_total\""),
            "I8: dedup_hit counter must fire when PublishAck.duplicate is true; \
             captured={captured:?}",
        );
        assert!(
            captured.contains("op=\"append\""),
            "I8: dedup_hit counter must carry the op label; captured={captured:?}",
        );
    }

    #[test]
    fn i8_dedup_hit_counter_silent_when_publish_ack_reports_no_duplicate() {
        let captured = capture_tracing(|| {
            super::record_append_outcome_metrics(super::TelemetryOp::Append, false);
        });
        assert!(
            !captured.contains("pardosa_jetstream_dedup_hit_total"),
            "I8: dedup_hit counter must stay silent when PublishAck.duplicate is \
             false (healthy band: bounded, legitimate-retry-only); captured={captured:?}",
        );
    }

    #[test]
    fn i2b_self_fence_counter_stays_zero_under_normal_operation() {
        use crate::backend::journal::RehydrateableBackend;
        let captured = capture_tracing(|| {
            let handle = JetStreamBackend::open(detached_config("self-fence"));
            let mut adapter = JetStreamBackendAdapter::new(handle);
            let _ = adapter.append(b"normal-bytes");
            let _ = adapter.sync();
            let _ = adapter.fetch_durable_bytes();
        });
        assert!(
            !captured.contains("pardosa_jetstream_occ_self_fence_total"),
            "I2b: self_fence counter must stay at its healthy zero value under \
             normal operation (Semaphore(1) makes self-fence structurally \
             unreachable); captured={captured:?}",
        );
        assert!(
            super::registered_metrics()
                .iter()
                .any(|metric| metric.name == "pardosa_jetstream_occ_self_fence_total"),
            "I2b: self_fence counter must still be registered so its zero value \
             is an observed invariant, not an absent signal",
        );
    }

    #[test]
    fn i5_bridge_duration_histogram_fires_for_sync_and_replay_not_only_append() {
        use crate::backend::journal::RehydrateableBackend;
        let captured = capture_tracing(|| {
            let handle = JetStreamBackend::open(detached_config("bridge-duration"));
            let mut adapter = JetStreamBackendAdapter::new(handle);
            let _ = adapter.sync();
            let _ = adapter.fetch_durable_bytes();
        });
        for op in ["sync", "replay"] {
            let has_op_line = captured.lines().any(|line| {
                line.contains("pardosa_jetstream_bridge_duration_seconds")
                    && line.contains(&format!("op=\"{op}\""))
            });
            assert!(
                has_op_line,
                "I5: bridge_duration histogram must fire for `{op}`, not only append; \
                 captured={captured:?}",
            );
        }
    }

    #[test]
    fn i5_ack_timeout_counter_fires_on_timeout_terminal_category() {
        let captured = capture_tracing(|| {
            let _ = super::observe_operation(super::OperationTelemetry::sync(), || {
                Err(BackendError::Timeout {
                    op: BackendOp::Sync,
                    elapsed: Duration::from_secs(31),
                    configured: Duration::from_secs(30),
                })
            });
        });
        assert!(
            captured.contains("metric_name=\"pardosa_jetstream_ack_timeout_total\""),
            "I5: ack_timeout counter must fire on a Timeout terminal category; \
             captured={captured:?}",
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
