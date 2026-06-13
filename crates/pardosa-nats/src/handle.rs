use crate::config::JetStreamConfig;
use crate::error::JetStreamRuntimeError;
use bytes::Bytes;
use futures_util::StreamExt;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use tokio::runtime::Handle;
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
/// A single record observed by [`JetStreamHandle::replay_all`]: the
/// substrate-opaque [`JetStreamAckPosition`] the record was
/// published at, paired with the canonical payload bytes (ADR-0022
/// §D5 — verbatim, no re-framing).
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
            .finish()
    }
}
impl JetStreamHandle {
    pub(crate) const fn new(config: JetStreamConfig) -> Self {
        Self {
            config,
            state: Mutex::new(None),
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
    pub fn append(&self, bytes: &[u8]) -> Result<JetStreamAckPosition, JetStreamRuntimeError> {
        let runtime = self
            .config
            .runtime_handle()
            .as_tokio()
            .ok_or(JetStreamRuntimeError::Detached)?
            .clone();
        let cfg = &self.config;
        let nats_msg_id = blake3_hex(bytes);
        let timeout = cfg.operation_timeout();
        let seq = run_op(&runtime, timeout, async {
            let js = ensure_state(&self.state, cfg).await?;
            publish_once(&js, cfg.subject(), bytes, &nats_msg_id).await
        })?;
        let mut guard = self.state_locked();
        if let Some(state) = guard.as_mut()
            && seq > state.last_ack_seq
        {
            state.last_ack_seq = seq;
        }
        Ok(JetStreamAckPosition::from_u64(seq))
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
        run_op(&runtime, cfg.operation_timeout(), async {
            let js = ensure_state(&self.state, cfg).await?;
            replay_once(&js, cfg.stream_name(), cfg.subject()).await
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
    let client = async_nats::connect(url)
        .await
        .map_err(|e| JetStreamRuntimeError::Connect {
            source: Box::new(e),
        })?;
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
async fn provision_stream(
    js: &async_nats::jetstream::Context,
    cfg: &JetStreamConfig,
) -> Result<(), JetStreamRuntimeError> {
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
    let stream_cfg = StreamConfig {
        name: cfg.stream_name().to_string(),
        subjects: vec![cfg.subject().to_string()],
        storage,
        num_replicas: usize::from(cfg.replicas().get()),
        retention: RetentionPolicy::Limits,
        discard,
        duplicate_window: Duration::from_mins(2),
        ..Default::default()
    };
    js.get_or_create_stream(stream_cfg)
        .await
        .map_err(|e| JetStreamRuntimeError::Connect {
            source: Box::new(e),
        })?;
    Ok(())
}
async fn publish_once(
    js: &async_nats::jetstream::Context,
    subject: &str,
    bytes: &[u8],
    nats_msg_id: &str,
) -> Result<u64, JetStreamRuntimeError> {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", nats_msg_id);
    let payload = bytes::Bytes::copy_from_slice(bytes);
    let publish_ack_future = js
        .publish_with_headers(subject.to_string(), headers, payload)
        .await
        .map_err(|e| JetStreamRuntimeError::Publish {
            source: Box::new(e),
        })?;
    let pub_ack = publish_ack_future
        .await
        .map_err(|e| JetStreamRuntimeError::Publish {
            source: Box::new(e),
        })?;
    Ok(pub_ack.sequence)
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
        out.push(JetStreamReplayRecord {
            ack: JetStreamAckPosition::from_u64(seq),
            payload,
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
