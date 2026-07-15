use crate::config::JetStreamConfig;
use crate::error::JetStreamRuntimeError;
use bytes::Bytes;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
const PARDOSA_ENVELOPE_HASH_HEADER: &str = "Pardosa-Envelope-Hash";
/// Backend-opaque positional marker minted by the substrate on every
/// [`JetStreamHandle::append`] / [`JetStreamHandle::sync`]
/// (ADR-0022 §D2 — `AckPosition` is opaque per backend, monotonic
/// within a single instance).
///
/// The in-crate adapter in `pardosa` maps this to the runtime's
/// `pardosa::store::AckPosition`. The `u64` view ([`Self::as_u64`])
/// is for diagnostics and cross-crate mapping only — not arithmetic.
/// For `JetStream` the inner `u64` is the `PubAck.seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
#[repr(transparent)]
pub struct JetStreamAckPosition(u64);
impl JetStreamAckPosition {
    /// Wrap a backend-supplied stream `seq` as a typed positional
    /// marker. `pub(crate)` so the substrate is the only minting
    /// site; the in-crate adapter wrapping the handle never mints
    /// a value of its own (ADR-0022 §D2 — backend opaque).
    pub(crate) const fn from_u64(value: u64) -> Self {
        Self(value)
    }
    /// Extract the underlying `u64`. Diagnostics / cross-crate
    /// mapping only; not for arithmetic.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}
/// Outcome of a single [`JetStreamHandle::append`] /
/// [`JetStreamHandle::append_with_replay_tag`] call: the substrate-opaque
/// [`JetStreamAckPosition`] plus the `JetStream` server's own
/// `Nats-Msg-Id`-window dedup signal (`PublishAck.duplicate`, async-nats
/// 0.49.1). Additive return-type extension threading the dedup bit across
/// the crate boundary without changing [`JetStreamAckPosition`]'s
/// `#[repr(transparent)]` shape (I8, `docs/pardosa/observability-slo.md`).
/// `#[non_exhaustive]` per ADR-0007.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct JetStreamAppendAck {
    /// Substrate-opaque positional marker minted for this publish.
    pub ack: JetStreamAckPosition,
    /// `true` when the `JetStream` server's `Nats-Msg-Id` dedup window
    /// determined this publish was a duplicate of a prior one; the
    /// returned `ack` is then the position of the original, not a new
    /// append.
    pub duplicate: bool,
}
/// A single record observed by [`JetStreamHandle::replay_all`]: the
/// substrate-opaque [`JetStreamAckPosition`] the record was
/// published at, paired with the canonical payload bytes and opaque
/// metadata surfaced from the backend.
///
/// Recovery code consumes a `Vec` of these in stream-`seq` order.
/// `payload` is a [`bytes::Bytes`] handle — no copy on the read path.
/// `#[non_exhaustive]` per ADR-0007.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct JetStreamReplayRecord {
    /// Substrate-opaque positional marker the record was published
    /// at. Strictly monotonic across the records returned by a
    /// single [`JetStreamHandle::replay_all`] call.
    pub ack: JetStreamAckPosition,
    /// Canonical payload bytes for this record (ADR-0022 §D5 —
    /// returned verbatim).
    pub payload: Bytes,
    /// Opaque backend metadata value copied from the replayed
    /// `Pardosa-Envelope-Hash` header when present.
    pub schema_tag: Option<String>,
}
/// Lazy-init state for the `JetStream` client, the provisioned
/// stream, and the most-recent ack-position. Lives behind a
/// mutex ([`std::sync::Mutex`]) so the substrate's sync surface
/// is `Send + Sync` per the auto-trait policy adopters and the
/// in-crate adapter rely on.
struct LiveState {
    js: async_nats::jetstream::Context,
    last_ack_seq: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ConnectDecision {
    credentials_path: Option<PathBuf>,
    require_tls: bool,
}

impl ConnectDecision {
    fn uses_bare_anonymous_connect(&self) -> bool {
        self.credentials_path.is_none() && !self.require_tls
    }
}

fn connect_decision(url: &str, credentials_path: Option<&Path>) -> ConnectDecision {
    ConnectDecision {
        credentials_path: credentials_path.map(Path::to_path_buf),
        require_tls: url.starts_with("tls://"),
    }
}

/// Opaque handle to a JetStream-backed authoritative-storage
/// substrate (ADR-0022 §D10 sibling crate; §D11 sealed-trait +
/// in-crate adapter pattern — this crate exports only the handle,
/// `pardosa` owns the [`crate::JetStreamBackend`]
/// → `pardosa::store::AuthoritativeBackend` adapter).
///
/// Construction via [`JetStreamBackend::open`] is offline; the
/// network is reached lazily on the first
/// [`JetStreamHandle::append`] (ADR-0022 §D7 — backend owns its
/// internal blocking-bridge runtime; sync from the runtime's
/// perspective).
pub struct JetStreamHandle {
    config: JetStreamConfig,
    state: Mutex<Option<LiveState>>,
    append_gate: Semaphore,
}
impl std::fmt::Debug for JetStreamHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JetStreamHandle")
            .field("config", &self.config)
            .field(
                "state",
                &match self.state.lock() {
                    Ok(g) => {
                        if g.is_some() {
                            "live"
                        } else {
                            "lazy"
                        }
                    }
                    Err(_) => "poisoned",
                },
            )
            .field("append_gate", &self.append_gate.available_permits())
            .finish()
    }
}
impl JetStreamHandle {
    pub(crate) const fn new(config: JetStreamConfig) -> Self {
        Self {
            config,
            state: Mutex::new(None),
            append_gate: Semaphore::const_new(1),
        }
    }
    /// Borrowed view of the validated configuration this handle
    /// was constructed from.
    #[must_use]
    pub fn config(&self) -> &JetStreamConfig {
        &self.config
    }
    /// Publish `bytes`; return [`JetStreamAckPosition`] from
    /// `PubAck.seq` (ADR-0022 §D2). Bytes verbatim (ADR-0022 §D5).
    /// `Nats-Msg-Id` = BLAKE3-hex(payload) for dedup. Lazy connect.
    ///
    /// # Errors
    ///
    /// * [`JetStreamRuntimeError::Detached`] — detached test handle.
    /// * [`JetStreamRuntimeError::Connect`] — connection / stream
    ///   provisioning failed.
    /// * [`JetStreamRuntimeError::Publish`] — server rejected publish
    ///   or ack-stream errored.
    /// * [`JetStreamRuntimeError::Timeout`] — publish-ack not within
    ///   the configured operation timeout.
    ///
    /// # Panics
    ///
    /// Panics if the state mutex is poisoned.
    pub fn append(&self, bytes: &[u8]) -> Result<JetStreamAppendAck, JetStreamRuntimeError> {
        self.append_inner(bytes, None)
    }
    /// Publish `bytes` with an opaque per-message replay tag supplied
    /// by the caller.
    ///
    /// The tag is copied into a `JetStream` header and surfaced again
    /// by [`Self::replay_all`] as [`JetStreamReplayRecord::schema_tag`].
    /// The substrate treats the value as opaque text and does not parse
    /// or compare it.
    ///
    /// # Errors
    ///
    /// * [`JetStreamRuntimeError::Detached`] — detached test handle.
    /// * [`JetStreamRuntimeError::Connect`] — connection / stream
    ///   provisioning failed.
    /// * [`JetStreamRuntimeError::Publish`] — server rejected publish
    ///   or ack-stream errored.
    /// * [`JetStreamRuntimeError::Timeout`] — publish-ack not within
    ///   the configured operation timeout.
    ///
    /// # Panics
    ///
    /// Panics if the state mutex is poisoned.
    pub fn append_with_replay_tag(
        &self,
        bytes: &[u8],
        replay_tag: &str,
    ) -> Result<JetStreamAppendAck, JetStreamRuntimeError> {
        self.append_inner(bytes, Some(replay_tag))
    }
    fn append_inner(
        &self,
        bytes: &[u8],
        replay_tag: Option<&str>,
    ) -> Result<JetStreamAppendAck, JetStreamRuntimeError> {
        let runtime = self
            .config
            .runtime_handle()
            .as_tokio()
            .ok_or(JetStreamRuntimeError::Detached)?
            .clone();
        let cfg = &self.config;
        let nats_msg_id = blake3_hex(bytes);
        let timeout = cfg.operation_timeout();
        let (seq, duplicate) = run_op(&runtime, timeout, async {
            let _permit = self
                .append_gate
                .acquire()
                .await
                .expect("append_gate semaphore is never closed");
            let js = ensure_state(&self.state, cfg).await?;
            let fence_enabled = cfg.single_writer_fence_enabled();
            let last_ack_seq = current_last_ack_seq(&self.state);
            let expected_last_subject_sequence =
                expected_last_subject_sequence_for_publish(fence_enabled, last_ack_seq);
            publish_once(
                &js,
                cfg.subject(),
                bytes,
                &nats_msg_id,
                replay_tag,
                expected_last_subject_sequence,
            )
            .await
        })?;
        let mut guard = self.state_locked();
        if let Some(state) = guard.as_mut() {
            update_last_ack_seq(&mut state.last_ack_seq, seq);
        }
        Ok(JetStreamAppendAck {
            ack: JetStreamAckPosition::from_u64(seq),
            duplicate,
        })
    }
    /// Durability fence (ADR-0022 §D2). Under Phase 1.5 §2.1's
    /// `Storage: File` + `R ≥ 1` + `AckExplicit`, every successful
    /// [`Self::append`] already blocks until fsync; `sync` returns
    /// the most-recent ack-position, acting as an idempotent
    /// barrier. A vacuous call returns zero.
    ///
    /// # Errors
    ///
    /// * [`JetStreamRuntimeError::Detached`] — see [`Self::append`].
    /// * Other variants do not surface in v0; the no-op carries no
    ///   in-flight bytes. `#[non_exhaustive]` keeps room for a
    ///   real server-side flush.
    ///
    /// # Panics
    ///
    /// Panics only if the internal state mutex is poisoned.
    pub fn sync(&self) -> Result<JetStreamAckPosition, JetStreamRuntimeError> {
        if self.config.runtime_handle().as_tokio().is_none() {
            return Err(JetStreamRuntimeError::Detached);
        }
        let guard = self.state_locked();
        let seq = guard.as_ref().map_or(0, |s| s.last_ack_seq);
        Ok(JetStreamAckPosition::from_u64(seq))
    }
    /// Read every currently-durable record in `PubAck.seq` order
    /// (ADR-0022 §D11). Bytes verbatim (ADR-0022 §D5); single-subject
    /// filter (Phase 1.5 §7). Lazy connect; vacuous returns `Ok(vec![])`.
    ///
    /// # Errors
    ///
    /// * [`JetStreamRuntimeError::Detached`] — detached test handle.
    /// * [`JetStreamRuntimeError::Connect`] — connection / stream
    ///   info fetch failed.
    /// * [`JetStreamRuntimeError::Replay`] — per-message fetch errored
    ///   or stream terminated abnormally.
    /// * [`JetStreamRuntimeError::Timeout`] — replay not within the
    ///   configured operation timeout.
    pub fn replay_all(&self) -> Result<Vec<JetStreamReplayRecord>, JetStreamRuntimeError> {
        let runtime = self
            .config
            .runtime_handle()
            .as_tokio()
            .ok_or(JetStreamRuntimeError::Detached)?
            .clone();
        let cfg = &self.config;
        let records = run_op(&runtime, cfg.operation_timeout(), async {
            let js = ensure_state(&self.state, cfg).await?;
            replay_once(&js, cfg.stream_name(), cfg.subject()).await
        })?;
        let replay_seed = records.last().map_or(0, |r| r.ack.as_u64());
        let mut guard = self.state_locked();
        if let Some(state) = guard.as_mut() {
            state.last_ack_seq = replay_seed;
        }
        Ok(records)
    }
    /// Read the stream `description` currently stored in `JetStream`.
    ///
    /// The returned string is opaque substrate metadata. This crate
    /// does not parse, validate, or compare it.
    ///
    /// # Errors
    ///
    /// * [`JetStreamRuntimeError::Detached`] — detached test handle.
    /// * [`JetStreamRuntimeError::Connect`] — connection / stream
    ///   provisioning failed.
    /// * [`JetStreamRuntimeError::Replay`] — stream lookup or stream
    ///   info fetch failed.
    /// * [`JetStreamRuntimeError::Timeout`] — info fetch not within
    ///   the configured operation timeout.
    ///
    /// # Panics
    ///
    /// Panics if the state mutex is poisoned.
    pub fn read_stream_description(&self) -> Result<Option<String>, JetStreamRuntimeError> {
        let runtime = self
            .config
            .runtime_handle()
            .as_tokio()
            .ok_or(JetStreamRuntimeError::Detached)?
            .clone();
        let cfg = &self.config;
        run_op(&runtime, cfg.operation_timeout(), async {
            let js = ensure_state(&self.state, cfg).await?;
            read_stream_description_once(&js, cfg.stream_name()).await
        })
    }
    fn state_locked(&self) -> MutexGuard<'_, Option<LiveState>> {
        lock_state(&self.state)
    }
}
/// Per-backend factory for [`JetStreamHandle`] (ADR-0022 §D11 —
/// `pardosa-nats` exports `JetStreamBackend::open(...)`;
/// adopters never construct a [`JetStreamHandle`] directly).
///
/// Mirrors the `PgnoBackend::open` precedent in
/// `pardosa::store::PgnoBackend`: opaque newtype factory that
/// does not touch the underlying substrate at construction time.
/// The first network call happens on the first
/// [`JetStreamHandle::append`], not here.
pub struct JetStreamBackend {
    _private: (),
}
impl JetStreamBackend {
    /// Capture `config` as a [`JetStreamHandle`] for a future
    /// `pardosa::store::EventStore::<T>::open_with_backend` call.
    ///
    /// No filesystem access, no network access, no stream
    /// provisioning. The validated configuration is stored
    /// verbatim; rehydration / connection happens on the first
    /// [`JetStreamHandle::append`].
    #[must_use]
    pub fn open(config: JetStreamConfig) -> JetStreamHandle {
        JetStreamHandle::new(config)
    }
}
fn lock_state(state: &Mutex<Option<LiveState>>) -> MutexGuard<'_, Option<LiveState>> {
    state.lock().expect("JetStreamHandle::state mutex poisoned")
}
fn current_last_ack_seq(state: &Mutex<Option<LiveState>>) -> u64 {
    let guard = lock_state(state);
    guard.as_ref().map_or(0, |s| s.last_ack_seq)
}
fn blake3_hex(bytes: &[u8]) -> String {
    let hash = blake3::hash(bytes);
    hash.to_hex().to_string()
}
fn run_op<F, T>(rt: &Handle, timeout: Duration, fut: F) -> Result<T, JetStreamRuntimeError>
where
    F: std::future::Future<Output = Result<T, JetStreamRuntimeError>> + Send,
    T: Send,
{
    let start = std::time::Instant::now();
    match rt.block_on(async move { tokio::time::timeout(timeout, fut).await }) {
        Ok(inner) => inner,
        Err(_elapsed) => Err(JetStreamRuntimeError::Timeout {
            elapsed: start.elapsed(),
            configured: timeout,
        }),
    }
}
async fn ensure_state(
    state: &Mutex<Option<LiveState>>,
    cfg: &JetStreamConfig,
) -> Result<async_nats::jetstream::Context, JetStreamRuntimeError> {
    {
        let guard = lock_state(state);
        if let Some(s) = guard.as_ref() {
            return Ok(s.js.clone());
        }
    }
    let url = cfg.nats_url().to_owned();
    let decision = connect_decision(&url, cfg.credentials_path());
    let client = connect_client(&url, &decision).await?;
    let js = async_nats::jetstream::new(client);
    provision_stream(&js, cfg).await?;
    let mut guard = lock_state(state);
    let resolved = if let Some(s) = guard.as_ref() {
        s.js.clone()
    } else {
        *guard = Some(LiveState {
            js: js.clone(),
            last_ack_seq: 0,
        });
        js
    };
    Ok(resolved)
}

async fn connect_client(
    url: &str,
    decision: &ConnectDecision,
) -> Result<async_nats::Client, JetStreamRuntimeError> {
    if decision.uses_bare_anonymous_connect() {
        async_nats::connect(url)
            .await
            .map_err(|e| JetStreamRuntimeError::Connect {
                source: Box::new(e),
            })
    } else {
        let options = build_connect_options(decision)?;
        options
            .connect(url)
            .await
            .map_err(|e| JetStreamRuntimeError::Connect {
                source: Box::new(e),
            })
    }
}

fn build_connect_options(
    decision: &ConnectDecision,
) -> Result<async_nats::ConnectOptions, JetStreamRuntimeError> {
    let mut options = async_nats::ConnectOptions::new();
    if let Some(path) = decision.credentials_path.as_deref() {
        let creds = std::fs::read_to_string(path).map_err(|e| JetStreamRuntimeError::Connect {
            source: Box::new(e),
        })?;
        options = async_nats::ConnectOptions::with_credentials(&creds).map_err(|e| {
            JetStreamRuntimeError::Connect {
                source: Box::new(e),
            }
        })?;
    }
    if decision.require_tls {
        options = options.require_tls(true);
    }
    Ok(options)
}
async fn provision_stream(
    js: &async_nats::jetstream::Context,
    cfg: &JetStreamConfig,
) -> Result<(), JetStreamRuntimeError> {
    let stream_cfg = build_stream_config(cfg);
    js.get_or_create_stream(stream_cfg.clone())
        .await
        .map_err(|e| JetStreamRuntimeError::Connect {
            source: Box::new(e),
        })?;
    if stream_cfg.description.is_some() {
        js.update_stream(stream_cfg)
            .await
            .map_err(|e| JetStreamRuntimeError::Connect {
                source: Box::new(e),
            })?;
    }
    Ok(())
}

fn build_stream_config(cfg: &JetStreamConfig) -> async_nats::jetstream::stream::Config {
    use async_nats::jetstream::stream::{
        Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType,
    };
    let storage = match cfg.storage() {
        crate::Storage::File => StorageType::File,
    };
    let discard = match cfg.discard() {
        crate::Discard::New => DiscardPolicy::New,
        crate::Discard::Old => DiscardPolicy::Old,
    };
    StreamConfig {
        name: cfg.stream_name().to_string(),
        subjects: vec![cfg.subject().to_string()],
        storage,
        num_replicas: usize::from(cfg.replicas().get()),
        retention: RetentionPolicy::Limits,
        discard,
        description: cfg.stream_description_marker().map(str::to_owned),
        duplicate_window: Duration::from_mins(2),
        ..Default::default()
    }
}

async fn read_stream_description_once(
    js: &async_nats::jetstream::Context,
    stream_name: &str,
) -> Result<Option<String>, JetStreamRuntimeError> {
    let stream = js
        .get_stream(stream_name)
        .await
        .map_err(|e| JetStreamRuntimeError::Replay {
            source: Box::new(e),
        })?;
    let info = stream
        .get_info()
        .await
        .map_err(|e| JetStreamRuntimeError::Replay {
            source: Box::new(e),
        })?;
    Ok(info.config.description)
}
async fn publish_once(
    js: &async_nats::jetstream::Context,
    subject: &str,
    bytes: &[u8],
    nats_msg_id: &str,
    replay_tag: Option<&str>,
    expected_last_subject_sequence: Option<u64>,
) -> Result<(u64, bool), JetStreamRuntimeError> {
    let headers = build_publish_headers(nats_msg_id, replay_tag, expected_last_subject_sequence);
    let payload = bytes::Bytes::copy_from_slice(bytes);
    let publish_ack_future = js
        .publish_with_headers(subject.to_string(), headers, payload)
        .await
        .map_err(|e| JetStreamRuntimeError::Publish {
            source: Box::new(e),
        })?;
    let pub_ack = publish_ack_future
        .await
        .map_err(runtime_error_from_publish_ack)?;
    Ok((pub_ack.sequence, pub_ack.duplicate))
}
fn runtime_error_from_publish_ack(
    err: async_nats::jetstream::context::PublishError,
) -> JetStreamRuntimeError {
    use async_nats::jetstream::context::PublishErrorKind;
    if err.kind() == PublishErrorKind::WrongLastSequence {
        JetStreamRuntimeError::WrongLastSequence {
            source: Box::new(err),
        }
    } else {
        JetStreamRuntimeError::Publish {
            source: Box::new(err),
        }
    }
}

fn expected_last_subject_sequence_for_publish(
    fence_enabled: bool,
    last_ack_seq: u64,
) -> Option<u64> {
    fence_enabled.then_some(last_ack_seq)
}

fn update_last_ack_seq(last_ack_seq: &mut u64, seq: u64) {
    if seq > *last_ack_seq {
        *last_ack_seq = seq;
    }
}

fn build_publish_headers(
    nats_msg_id: &str,
    replay_tag: Option<&str>,
    expected_last_subject_sequence: Option<u64>,
) -> async_nats::HeaderMap {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", nats_msg_id);
    if let Some(tag) = replay_tag {
        headers.insert(PARDOSA_ENVELOPE_HASH_HEADER, tag);
    }
    if let Some(seq) = expected_last_subject_sequence {
        headers.insert(
            async_nats::header::NATS_EXPECTED_LAST_SUBJECT_SEQUENCE,
            async_nats::HeaderValue::from(seq),
        );
    }
    headers
}
async fn replay_once(
    js: &async_nats::jetstream::Context,
    stream_name: &str,
    subject: &str,
) -> Result<Vec<JetStreamReplayRecord>, JetStreamRuntimeError> {
    use async_nats::jetstream::consumer::pull::Stream as PullStream;
    let stream = js
        .get_stream(stream_name)
        .await
        .map_err(|e| JetStreamRuntimeError::Replay {
            source: Box::new(e),
        })?;
    let info = stream
        .get_info()
        .await
        .map_err(|e| JetStreamRuntimeError::Replay {
            source: Box::new(e),
        })?;
    let last_seq = info.state.last_sequence;
    if info.state.messages == 0 {
        return Ok(Vec::new());
    }
    let consumer = stream
        .create_consumer(build_replay_pull_config(subject))
        .await
        .map_err(|e| JetStreamRuntimeError::Replay {
            source: Box::new(e),
        })?;
    let mut messages: PullStream =
        consumer
            .messages()
            .await
            .map_err(|e| JetStreamRuntimeError::Replay {
                source: Box::new(e),
            })?;
    let mut out: Vec<JetStreamReplayRecord> = Vec::new();
    while let Some(item) = messages.next().await {
        let msg = item.map_err(|e| JetStreamRuntimeError::Replay {
            source: Box::new(e),
        })?;
        let info = msg
            .info()
            .map_err(|e| JetStreamRuntimeError::Replay { source: e })?;
        let seq = info.stream_sequence;
        let payload = msg.message.payload.clone();
        let schema_tag = msg
            .message
            .headers
            .as_ref()
            .and_then(|headers| headers.get(PARDOSA_ENVELOPE_HASH_HEADER))
            .map(ToString::to_string);
        out.push(JetStreamReplayRecord {
            ack: JetStreamAckPosition::from_u64(seq),
            payload,
            schema_tag,
        });
        if seq >= last_seq {
            break;
        }
    }
    out.sort_by_key(|r| r.ack.as_u64());
    Ok(out)
}
fn build_replay_pull_config(subject: &str) -> async_nats::jetstream::consumer::pull::Config {
    use async_nats::jetstream::consumer::{
        AckPolicy, DeliverPolicy, ReplayPolicy, pull::Config as PullConfig,
    };
    PullConfig {
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::None,
        filter_subject: subject.to_string(),
        replay_policy: ReplayPolicy::Instant,
        inactive_threshold: Duration::from_secs(30),
        ..Default::default()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_pull_config_sets_inactive_threshold_30s() {
        let cfg = build_replay_pull_config("pardosa.test.subject");
        assert_eq!(
            cfg.inactive_threshold,
            Duration::from_secs(30),
            "ephemeral replay consumer must declare a 30s inactive_threshold so the \
             JetStream server reclaims the consumer if replay_all is interrupted \
             (linus review round 1 M1)"
        );
    }
    #[test]
    fn connect_decision_tls_url_with_credentials_requires_tls_and_applies_credentials() {
        let creds = std::path::Path::new("/run/secrets/nats.creds");
        let decision = connect_decision("tls://connect.nats.mattilsynet.io:4222", Some(creds));

        assert_eq!(decision.credentials_path.as_deref(), Some(creds));
        assert!(
            decision.require_tls,
            "tls:// URL with credentials must require TLS"
        );
        assert!(
            !decision.uses_bare_anonymous_connect(),
            "credentials path must leave the anonymous connect branch"
        );
    }

    #[test]
    fn connect_decision_nats_url_with_credentials_does_not_force_tls() {
        let creds = std::path::Path::new("/run/secrets/nats.creds");
        let decision = connect_decision("nats://127.0.0.1:4222", Some(creds));

        assert_eq!(decision.credentials_path.as_deref(), Some(creds));
        assert!(
            !decision.require_tls,
            "nats:// URL must not force TLS for the local live-NATS harness"
        );
    }

    #[test]
    fn connect_decision_none_credentials_on_nats_url_preserves_anonymous_branch() {
        let decision = connect_decision("nats://127.0.0.1:4222", None);

        assert_eq!(decision.credentials_path, None);
        assert!(
            decision.uses_bare_anonymous_connect(),
            "None credentials on nats:// must preserve the bare anonymous connect branch"
        );
    }

    #[test]
    fn build_connect_options_accepts_valid_creds_fixture_without_tokio_blocking_pool() {
        let dir = tempfile::tempdir().expect("tempdir is created");
        let creds_path = dir.path().join("user.creds");
        std::fs::write(&creds_path, valid_user_creds()).expect("creds fixture is written");
        let decision = ConnectDecision {
            credentials_path: Some(creds_path),
            require_tls: false,
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .max_blocking_threads(1)
            .build()
            .expect("current-thread runtime is created with one blocking thread");

        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let options = runtime.block_on(async {
            let blocker = tokio::task::spawn_blocking(move || {
                entered_tx.send(()).expect("blocking task starts");
                release_rx.recv().expect("blocking task is released");
            });
            entered_rx.recv().expect("blocking task has started");
            let result = tokio::time::timeout(Duration::from_millis(100), async {
                build_connect_options(&decision)
            })
            .await;
            release_tx.send(()).expect("blocking task release is sent");
            blocker.await.expect("blocking task joins");
            result
        });

        assert!(
            matches!(options, Ok(Ok(_))),
            "valid credentials fixture must build connect options without tokio file IO"
        );
        assert!(!decision.uses_bare_anonymous_connect());
    }

    #[test]
    fn build_connect_options_missing_creds_file_returns_connect_error() {
        let dir = tempfile::tempdir().expect("tempdir is created");
        let decision = ConnectDecision {
            credentials_path: Some(dir.path().join("missing.creds")),
            require_tls: false,
        };

        let result = blocking_build_connect_options(&decision);

        assert!(matches!(result, Err(JetStreamRuntimeError::Connect { .. })));
    }

    #[test]
    fn build_connect_options_garbage_creds_returns_connect_error() {
        let dir = tempfile::tempdir().expect("tempdir is created");
        let creds_path = dir.path().join("garbage.creds");
        std::fs::write(&creds_path, "not creds").expect("garbage fixture is written");
        let decision = ConnectDecision {
            credentials_path: Some(creds_path),
            require_tls: false,
        };

        let result = blocking_build_connect_options(&decision);

        assert!(matches!(result, Err(JetStreamRuntimeError::Connect { .. })));
    }

    fn blocking_build_connect_options(
        decision: &ConnectDecision,
    ) -> Result<async_nats::ConnectOptions, JetStreamRuntimeError> {
        tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .build()
            .expect("current-thread runtime is created")
            .block_on(async { build_connect_options(decision) })
    }

    fn valid_user_creds() -> String {
        let seed = "SUACH75SWCM5D2JMJM6EKLR2WDARVGZT4QC6LX3AGHSWOMVAKERABBBRWM";
        format!(
            "-----BEGIN NATS USER JWT-----\nnot.a.jwt\n------END NATS USER JWT------\n\n-----BEGIN USER NKEY SEED-----\n{seed}\n------END USER NKEY SEED------\n"
        )
    }

    fn minimal_config() -> JetStreamConfig {
        JetStreamConfig::builder()
            .stream_name("PARDOSA_TEST")
            .subject("pardosa.test")
            .durable_consumer("pardosa-test")
            .runtime_handle(crate::RuntimeHandle::detached_for_tests())
            .operation_timeout(Duration::from_secs(1))
            .build()
            .expect("test config builds")
    }

    #[test]
    fn stream_config_carries_opaque_description_marker_when_configured() {
        let cfg = JetStreamConfig::builder()
            .stream_name("PARDOSA_TEST")
            .subject("pardosa.test")
            .durable_consumer("pardosa-test")
            .runtime_handle(crate::RuntimeHandle::detached_for_tests())
            .operation_timeout(Duration::from_secs(1))
            .stream_description_marker("opaque-marker")
            .build()
            .expect("marker-bearing config builds");

        let stream_cfg = build_stream_config(&cfg);

        assert_eq!(stream_cfg.description, Some("opaque-marker".to_owned()));
    }

    #[test]
    fn stream_config_leaves_description_unset_without_marker() {
        let stream_cfg = build_stream_config(&minimal_config());

        assert_eq!(stream_cfg.description, None);
    }

    #[test]
    fn detached_handle_read_stream_description_returns_detached() {
        let handle = JetStreamBackend::open(minimal_config());

        let err = handle
            .read_stream_description()
            .expect_err("detached test handle cannot read live stream info");

        assert!(matches!(err, JetStreamRuntimeError::Detached));
    }
    #[test]
    fn wrong_last_sequence_ack_error_maps_to_neutral_variant() {
        let err =
            runtime_error_from_publish_ack(async_nats::jetstream::context::PublishError::new(
                async_nats::jetstream::context::PublishErrorKind::WrongLastSequence,
            ));
        match err {
            JetStreamRuntimeError::WrongLastSequence { source } => {
                assert!(
                    source.to_string().contains("wrong last sequence"),
                    "source preserved for operator diagnosis: {source}"
                );
            }
            other => panic!("expected WrongLastSequence, got {other:?}"),
        }
    }
    #[test]
    fn enabled_fence_uses_updated_last_ack_seq_on_next_publish() {
        let mut last_ack_seq = 0;
        let first = expected_last_subject_sequence_for_publish(true, last_ack_seq);
        assert_eq!(first, Some(0), "fresh subject publish must expect 0");
        update_last_ack_seq(&mut last_ack_seq, 7);

        let second = expected_last_subject_sequence_for_publish(true, last_ack_seq);
        let headers = build_publish_headers("msg-2", None, second);
        let header = headers
            .get(async_nats::header::NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
            .expect("enabled fence must set expect-last-subject-sequence header");

        assert_eq!(
            header.to_string(),
            "7",
            "next append must expect acked seq N"
        );
    }
    #[test]
    fn disabled_fence_keeps_publish_headers_without_expect_sequence() {
        let expected = expected_last_subject_sequence_for_publish(false, 7);
        let headers = build_publish_headers("msg-id", Some("schema"), expected);

        assert_eq!(
            headers.len(),
            2,
            "disabled fence preserves existing header set"
        );
        assert_eq!(
            headers.get("Nats-Msg-Id").expect("message id").to_string(),
            "msg-id"
        );
        assert_eq!(
            headers
                .get(PARDOSA_ENVELOPE_HASH_HEADER)
                .expect("replay tag")
                .to_string(),
            "schema"
        );
        assert!(
            headers
                .get(async_nats::header::NATS_EXPECTED_LAST_SUBJECT_SEQUENCE)
                .is_none(),
            "disabled fence must not emit expect-last-subject-sequence"
        );
    }
    #[test]
    fn non_conflict_ack_error_stays_publish() {
        let err =
            runtime_error_from_publish_ack(async_nats::jetstream::context::PublishError::new(
                async_nats::jetstream::context::PublishErrorKind::Other,
            ));
        match err {
            JetStreamRuntimeError::Publish { source } => {
                assert!(
                    source.to_string().contains("publish failed"),
                    "non-conflict source preserved: {source}"
                );
            }
            other => panic!("expected Publish, got {other:?}"),
        }
    }
    #[test]
    fn replay_pull_config_filters_to_caller_subject() {
        let cfg = build_replay_pull_config("pardosa.test.subject");
        assert_eq!(cfg.filter_subject, "pardosa.test.subject");
    }
    #[test]
    fn replay_pull_config_is_read_only_replay() {
        use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
        let cfg = build_replay_pull_config("any");
        assert!(
            matches!(cfg.deliver_policy, DeliverPolicy::All),
            "replay must enumerate every seq from 1"
        );
        assert!(
            matches!(cfg.ack_policy, AckPolicy::None),
            "replay must not consume from the stream"
        );
        assert!(
            matches!(cfg.replay_policy, ReplayPolicy::Instant),
            "replay is not rate-throttled"
        );
        assert!(
            cfg.durable_name.is_none(),
            "replay consumer must be ephemeral (no durable name)"
        );
    }
    #[test]
    fn ack_position_derived_ord_matches_underlying_seq_ordering() {
        let lo = JetStreamAckPosition::from_u64(1);
        let mid = JetStreamAckPosition::from_u64(2);
        let hi = JetStreamAckPosition::from_u64(u64::MAX);
        assert!(lo < mid, "monotonic on adjacent seq values");
        assert!(mid < hi, "monotonic across the full u64 range");
        assert_eq!(lo, JetStreamAckPosition::from_u64(1), "Eq by value");
        let mut positions = vec![hi, lo, mid];
        positions.sort();
        assert_eq!(
            positions,
            vec![lo, mid, hi],
            "derived Ord agrees with as_u64 ordering — the invariant `replay_once` \
             relies on when calling `out.sort_by_key(|r| r.ack.as_u64())` to surface \
             records in publish (PubAck.seq) order without parsing JetStream \
             metadata in the runtime crate (substrate-level I2 ordered opaque \
             positions; mission rescue-pardosa-k67j)"
        );
    }
    #[test]
    fn replay_record_payload_carries_bytes_verbatim_without_reframing() {
        let payload_in: &[u8] = b"alpha\x00\xff-beta-gamma";
        let record = JetStreamReplayRecord {
            ack: JetStreamAckPosition::from_u64(42),
            payload: Bytes::copy_from_slice(payload_in),
            schema_tag: None,
        };
        assert_eq!(
            record.payload.as_ref(),
            payload_in,
            "JetStreamReplayRecord.payload must round-trip canonical bytes \
             verbatim (ADR-0022 §D5 — no transformation, no re-framing, no \
             header injection at the substrate read boundary; substrate-level \
             I1 append→replay byte identity; mission rescue-pardosa-k67j)"
        );
        assert_eq!(
            record.ack.as_u64(),
            42,
            "JetStreamReplayRecord.ack surfaces the opaque positional primitive \
             unchanged from JetStreamHandle::append's PubAck.seq (substrate-level \
             I2 ordered opaque positions; mission rescue-pardosa-k67j)"
        );
    }
}
