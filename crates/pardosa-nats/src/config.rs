use crate::error::JetStreamConfigError;
use crate::runtime::RuntimeHandle;
use std::num::NonZeroU16;
use std::time::Duration;

pub(crate) const DEFAULT_NATS_URL: &str = "nats://localhost:4222";
pub const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const OPERATION_TIMEOUT_ENV: &str = "PARDOSA_NATS_OPERATION_TIMEOUT_SECS";
/// `JetStream` stream durability backend — `Storage: File` is the
/// only v0-admissible setting per Phase 1.5 §7.3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Storage {
    /// File-backed stream storage (Phase 1.5 §7.3 binding default).
    File,
}
/// `JetStream` stream-overflow policy. Phase 1.5 §7.3 forbids
/// `Discard::Old` in v0 (discarding old messages from the
/// authoritative log would lose events).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Discard {
    /// Reject new writes on overflow (Phase 1.5 §7.3 binding
    /// admissible setting).
    New,
    /// Drop old messages on overflow.
    ///
    /// Surfaced to the public surface only so the constructor
    /// can typedly reject it via
    /// [`JetStreamConfigError::DiscardOldForbidden`]. Adopters
    /// who pass this value receive a typed error at build time
    /// rather than at runtime.
    Old,
}
/// Validated, immutable configuration for a [`crate::
/// JetStreamHandle`] — the offline shape of the substrate.
///
/// Constructed via [`JetStreamConfig::builder`] +
/// [`JetStreamConfigBuilder::build`]. Each accessor returns the
/// stored value verbatim; the builder is the only validation site.
#[derive(Debug)]
pub struct JetStreamConfig {
    stream_name: String,
    subject: String,
    durable_consumer: String,
    storage: Storage,
    discard: Discard,
    replicas: NonZeroU16,
    runtime_handle: RuntimeHandle,
    nats_url: String,
    operation_timeout: Duration,
    single_writer_fence_enabled: bool,
}
impl JetStreamConfig {
    /// Begin assembling a [`JetStreamConfig`] via the builder.
    #[must_use]
    pub fn builder() -> JetStreamConfigBuilder {
        JetStreamConfigBuilder::default()
    }
    /// The `JetStream` stream name this handle targets
    /// (single-stream-per-handle per ADR-0022 §D4 +
    /// Phase 1.5 §7).
    #[must_use]
    pub fn stream_name(&self) -> &str {
        &self.stream_name
    }
    /// The single subject this handle publishes on
    /// (Phase 1.5 §7 single-subject layout).
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }
    /// The durable `JetStream` consumer name used by the cursor
    /// (Phase 1.5 §5 — C2 sidecar-mirror reads from a durable
    /// consumer).
    #[must_use]
    pub fn durable_consumer(&self) -> &str {
        &self.durable_consumer
    }
    /// Stream storage backend (Phase 1.5 §7.3 — `Storage::File`
    /// is the only v0-admissible value).
    #[must_use]
    pub fn storage(&self) -> Storage {
        self.storage
    }
    /// Stream overflow policy (Phase 1.5 §7.3 — `Discard::New`
    /// is the only v0-admissible value).
    #[must_use]
    pub fn discard(&self) -> Discard {
        self.discard
    }
    /// `JetStream` replica count (Phase 1.5 §7.3 — `R ≥ 1`,
    /// enforced via [`NonZeroU16`]).
    #[must_use]
    pub fn replicas(&self) -> NonZeroU16 {
        self.replicas
    }
    /// Runtime handle the `JetStream` client will execute against
    /// (ADR-0022 §D7 — caller-supplied, no defaults).
    #[must_use]
    pub fn runtime_handle(&self) -> &RuntimeHandle {
        &self.runtime_handle
    }
    /// NATS URL the substrate connects to on the first
    /// [`crate::JetStreamHandle::append`] / `replay`. Defaults to
    /// `nats://localhost:4222` when [`JetStreamConfigBuilder::nats_url`]
    /// is not set. Live test harnesses pass the spawned server URL
    /// explicitly through the builder.
    #[must_use]
    pub fn nats_url(&self) -> &str {
        &self.nats_url
    }
    /// Per-operation timeout applied to `JetStream` connect, publish,
    /// replay, and publish-ack operations. Defaults to
    /// [`DEFAULT_OPERATION_TIMEOUT`] unless overridden by the builder
    /// or [`OPERATION_TIMEOUT_ENV`].
    #[must_use]
    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }
    /// Whether publishes include the `Nats-Expected-Last-Subject-Sequence`
    /// append fence. Defaults to `false`; runtime adopters opt in when the
    /// subject is the authoritative single-writer surface.
    #[must_use]
    pub const fn single_writer_fence_enabled(&self) -> bool {
        self.single_writer_fence_enabled
    }
}
/// Incremental builder for [`JetStreamConfig`]. Validation runs
/// exactly once, in [`Self::build`].
#[derive(Debug, Default)]
pub struct JetStreamConfigBuilder {
    stream_name: Option<String>,
    subject: Option<String>,
    durable_consumer: Option<String>,
    storage: Option<Storage>,
    discard: Option<Discard>,
    replicas: Option<u16>,
    runtime_handle: Option<RuntimeHandle>,
    nats_url: Option<String>,
    operation_timeout: Option<Duration>,
    single_writer_fence_enabled: Option<bool>,
}
impl JetStreamConfigBuilder {
    /// Set the `JetStream` stream name (rejected if empty at
    /// [`Self::build`]).
    #[must_use]
    pub fn stream_name(mut self, name: impl Into<String>) -> Self {
        self.stream_name = Some(name.into());
        self
    }
    /// Set the single subject this handle publishes on (rejected
    /// if empty or if it contains a NATS wildcard).
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }
    /// Set the durable `JetStream` consumer name (rejected if
    /// empty at [`Self::build`]).
    #[must_use]
    pub fn durable_consumer(mut self, name: impl Into<String>) -> Self {
        self.durable_consumer = Some(name.into());
        self
    }
    /// Override the stream storage backend. Default
    /// [`Storage::File`].
    #[must_use]
    pub fn storage(mut self, storage: Storage) -> Self {
        self.storage = Some(storage);
        self
    }
    /// Override the stream-overflow policy. Default
    /// [`Discard::New`]; [`Discard::Old`] is rejected at
    /// [`Self::build`] per Phase 1.5 §7.3.
    #[must_use]
    pub fn discard(mut self, discard: Discard) -> Self {
        self.discard = Some(discard);
        self
    }
    /// Override the `JetStream` replica count. Default `1`; `0`
    /// is rejected at [`Self::build`].
    #[must_use]
    pub fn replicas(mut self, replicas: u16) -> Self {
        self.replicas = Some(replicas);
        self
    }
    /// Set the caller-supplied runtime handle (ADR-0022 §D7 —
    /// required, no defaults).
    #[must_use]
    pub fn runtime_handle(mut self, handle: RuntimeHandle) -> Self {
        self.runtime_handle = Some(handle);
        self
    }
    /// Override the NATS URL the substrate connects to. Defaults to
    /// `nats://localhost:4222` when omitted. Live test harnesses use
    /// this setter to connect handles to their spawned server.
    #[must_use]
    pub fn nats_url(mut self, url: impl Into<String>) -> Self {
        self.nats_url = Some(url.into());
        self
    }
    /// Override the `JetStream` per-operation timeout. Defaults to
    /// [`DEFAULT_OPERATION_TIMEOUT`] unless [`OPERATION_TIMEOUT_ENV`]
    /// is present.
    #[must_use]
    pub fn operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = Some(timeout);
        self
    }
    /// Enable or disable the append-path single-writer fence.
    #[must_use]
    pub const fn single_writer_fence_enabled(mut self, enabled: bool) -> Self {
        self.single_writer_fence_enabled = Some(enabled);
        self
    }
    /// Run validation and assemble the immutable [`JetStreamConfig`].
    ///
    /// # Errors
    ///
    /// * [`JetStreamConfigError::EmptyStreamName`] — missing/empty.
    /// * [`JetStreamConfigError::EmptySubject`] — missing/empty.
    /// * [`JetStreamConfigError::SubjectContainsWildcard`] — `*` / `>`
    ///   (Phase 1.5 §7).
    /// * [`JetStreamConfigError::EmptyDurableConsumer`] — missing/empty.
    /// * [`JetStreamConfigError::DiscardOldForbidden`] —
    ///   [`Discard::Old`] (Phase 1.5 §7.3).
    /// * [`JetStreamConfigError::ReplicasMustBePositive`] — `R ≥ 1`
    ///   (Phase 1.5 §7.3).
    /// * [`JetStreamConfigError::MissingRuntimeHandle`] — no handle
    ///   (ADR-0022 §D7).
    pub fn build(self) -> Result<JetStreamConfig, JetStreamConfigError> {
        let stream_name = match self.stream_name {
            Some(s) if !s.is_empty() => s,
            _ => return Err(JetStreamConfigError::EmptyStreamName),
        };
        let subject = match self.subject {
            Some(s) if !s.is_empty() => s,
            _ => return Err(JetStreamConfigError::EmptySubject),
        };
        if let Some(c) = subject.chars().find(|c| *c == '*' || *c == '>') {
            return Err(JetStreamConfigError::SubjectContainsWildcard { wildcard: c });
        }
        let durable_consumer = match self.durable_consumer {
            Some(s) if !s.is_empty() => s,
            _ => return Err(JetStreamConfigError::EmptyDurableConsumer),
        };
        let storage = self.storage.unwrap_or(Storage::File);
        let discard = self.discard.unwrap_or(Discard::New);
        if matches!(discard, Discard::Old) {
            return Err(JetStreamConfigError::DiscardOldForbidden);
        }
        let replicas_u16 = self.replicas.unwrap_or(1);
        let replicas =
            NonZeroU16::new(replicas_u16).ok_or(JetStreamConfigError::ReplicasMustBePositive)?;
        let runtime_handle = self
            .runtime_handle
            .ok_or(JetStreamConfigError::MissingRuntimeHandle)?;
        let nats_url = self
            .nats_url
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DEFAULT_NATS_URL.to_owned());
        let operation_timeout = match self.operation_timeout {
            Some(timeout) => validate_operation_timeout(timeout)?,
            None => operation_timeout_from_env()?,
        };
        let single_writer_fence_enabled = self.single_writer_fence_enabled.unwrap_or(false);
        Ok(JetStreamConfig {
            stream_name,
            subject,
            durable_consumer,
            storage,
            discard,
            replicas,
            runtime_handle,
            nats_url,
            operation_timeout,
            single_writer_fence_enabled,
        })
    }
}
fn validate_operation_timeout(timeout: Duration) -> Result<Duration, JetStreamConfigError> {
    if timeout.is_zero() {
        Err(JetStreamConfigError::OperationTimeoutMustBePositive)
    } else {
        Ok(timeout)
    }
}
fn operation_timeout_from_env() -> Result<Duration, JetStreamConfigError> {
    match std::env::var(OPERATION_TIMEOUT_ENV) {
        Ok(raw) => {
            let secs =
                raw.parse::<u64>()
                    .map_err(|_| JetStreamConfigError::InvalidOperationTimeout {
                        value: raw.clone(),
                    })?;
            validate_operation_timeout(Duration::from_secs(secs))
        }
        Err(std::env::VarError::NotPresent) => Ok(DEFAULT_OPERATION_TIMEOUT),
        Err(std::env::VarError::NotUnicode(value)) => {
            Err(JetStreamConfigError::InvalidOperationTimeout {
                value: value.to_string_lossy().into_owned(),
            })
        }
    }
}
