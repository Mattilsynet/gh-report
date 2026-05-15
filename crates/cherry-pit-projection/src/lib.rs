//! # cherry-pit-projection
//!
//! Read-side adapters that drive [`cherry_pit_core::Projection`] from a
//! typed [`cherry_pit_core::EventStore`].
//!
//! CHE-0048 fixes this crate's shape: single aggregate type per driver,
//! no `Box<dyn Projection>`, in-memory and file-based backends, and
//! checkpointed replay where the snapshot is written before the checkpoint.
//! The file backend intentionally re-implements the gateway crate's
//! atomic temp-file + rename + directory-sync pattern (CHE-0032/CHE-0043)
//! rather than extracting a shared utility crate mid-WU.
//!
//! TODO: if more crates need the same filesystem primitive, introduce a
//! dedicated ADR for a shared utility crate instead of copy-pasting again.
//!
//! Adoption of `cherry-pit-storage` (the eventual home of the
//! shared atomic-write + dir-sync helpers) is **explicitly deferred** for
//! v0.1 of this crate, mirroring the gateway-crate R12 carve-out per
//! CHE-0053 R8. The deferral is tracked under the WU-3 closure epic
//! (bd `adr-fmt-hh07`, sub-mission SM-3.5); revisit when a third call
//! site lands or when CHE-0053 R8 is rescinded.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt;
use std::fs::File;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use cherry_pit_core::{AggregateId, ErrorCategory, EventStore, Projection};
use serde::{Serialize, de::DeserializeOwned};

/// Errors returned by projection drivers and storage backends.
///
/// Variants split structural / unrecoverable failures
/// ([`CorruptData`](Self::CorruptData)) from transient infrastructure
/// failures ([`Infrastructure`](Self::Infrastructure)). Use
/// [`category`](Self::category) to drive retry policy per CHE-0021.
///
/// # Examples
///
/// ```
/// use cherry_pit_core::ErrorCategory;
/// use cherry_pit_projection::ProjectionError;
///
/// let corrupt = ProjectionError::CorruptData("bad bytes".into());
/// assert_eq!(corrupt.category(), ErrorCategory::Terminal);
///
/// let infra = ProjectionError::Infrastructure("disk full".into());
/// assert_eq!(infra.category(), ErrorCategory::Retryable);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum ProjectionError {
    /// Persisted or loaded data failed structural validation.
    CorruptData(Box<dyn Error + Send + Sync>),

    /// Infrastructure failure while loading events or storing projection state.
    Infrastructure(Box<dyn Error + Send + Sync>),

    /// Advisory store-directory lock is held by another process or
    /// projection store instance. Surfaces CHE-0043:R1–R3 fencing
    /// contention to callers as a retryable failure.
    StoreLocked,
}

impl ProjectionError {
    /// Classify the projection failure for retry guidance.
    ///
    /// `CorruptData` maps to [`ErrorCategory::Terminal`] (do not retry); other
    /// variants map to [`ErrorCategory::Retryable`] (retry per CHE-0046).
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::CorruptData(_) => ErrorCategory::Terminal,
            Self::Infrastructure(_) | Self::StoreLocked => ErrorCategory::Retryable,
        }
    }

    /// Emit a structured `warn`-level event tagged with this error's
    /// retry category. Called at every public API boundary so operators
    /// see categorisation (retryable vs terminal) on every surfaced
    /// failure without instrumenting each internal `?` site (COM-0019 L04).
    fn emit_event(&self) {
        let category = match self.category() {
            ErrorCategory::Retryable => "retryable",
            ErrorCategory::Terminal => "terminal",
            _ => "unknown",
        };
        tracing::warn!(
            target: "cherry_pit_projection",
            category,
            error = %self,
            "projection error surfaced",
        );
    }
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CorruptData(e) => write!(f, "projection corrupt data: {e}"),
            Self::Infrastructure(e) => write!(f, "projection infrastructure error: {e}"),
            Self::StoreLocked => write!(
                f,
                "projection store directory is locked by another writer (CHE-0043)"
            ),
        }
    }
}

impl Error for ProjectionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::CorruptData(e) | Self::Infrastructure(e) => Some(e.as_ref()),
            Self::StoreLocked => None,
        }
    }
}

/// Result alias for projection operations.
pub type ProjectionResult<T> = Result<T, ProjectionError>;

/// Durable checkpoint for one `(aggregate_id, projection_name)` pair.
///
/// Canonical home: [`cherry_pit_core::ProjectionCheckpoint`]. Re-exported
/// here for back-compat — existing `cherry_pit_projection::ProjectionCheckpoint`
/// paths continue to resolve. Per CHE-0048 R9 the type lives in core; this
/// crate owns the file-backend storage that consumes it.
pub use cherry_pit_core::ProjectionCheckpoint;

/// `MessagePack` file backend for projection snapshots and checkpoints.
///
/// Writes one snapshot and one sibling checkpoint file per
/// `(aggregate_id, projection_name)` pair. Persistence is crash-conscious:
/// each file is written to a temporary file, fsynced, renamed into place,
/// and followed by a directory fsync. [`persist`](Self::persist) writes the
/// snapshot strictly before writing the checkpoint, so a crash between the
/// two leaves the snapshot present but the checkpoint absent — restart code
/// must treat that as "rebuild" rather than "trust snapshot".
///
/// # Examples
///
/// Construct a backend and verify its identity (no I/O):
///
/// ```
/// use cherry_pit_projection::FileProjectionStore;
///
/// let store = FileProjectionStore::<()>::new("target/projection-doctest", "counter_view");
/// assert_eq!(store.projection_name(), "counter_view");
/// assert_eq!(store.dir(), std::path::Path::new("target/projection-doctest"));
/// ```
///
/// Persist a snapshot + checkpoint and read them back:
///
/// ```
/// use std::num::NonZeroU64;
/// use cherry_pit_core::AggregateId;
/// use cherry_pit_projection::FileProjectionStore;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// struct CounterView { total: u64 }
///
/// # tokio::runtime::Builder::new_current_thread()
/// #     .enable_all()
/// #     .build()
/// #     .unwrap()
/// #     .block_on(async {
/// let dir = tempfile::tempdir().unwrap();
/// let store = FileProjectionStore::<CounterView>::new(dir.path(), "counter_view");
/// let id = AggregateId::new(NonZeroU64::new(1).unwrap());
///
/// store.persist(id, &CounterView { total: 4 }, 4).await.unwrap();
///
/// let snapshot = store.load_snapshot(id).await.unwrap();
/// assert_eq!(snapshot, Some(CounterView { total: 4 }));
///
/// let checkpoint = store.load_checkpoint(id).await.unwrap().unwrap();
/// assert_eq!(checkpoint.last_sequence(), 4);
/// # });
/// ```
#[derive(Debug, Clone)]
pub struct FileProjectionStore<P> {
    dir: PathBuf,
    projection_name: String,
    /// Advisory `.lock` fencing per CHE-0043:R1–R3. Lazy-initialised on the
    /// first mutating call (`persist`, `delete`) per CHE-0043:R2. Wrapped in
    /// `Arc` so cloned instances share lock state — clones represent the
    /// same backend identity and must not self-contend.
    lock: Arc<OnceLock<File>>,
    _projection: PhantomData<fn() -> P>,
}

impl<P> FileProjectionStore<P> {
    /// Create a file backend rooted at `dir` for one projection identity.
    ///
    /// `projection_name` becomes part of every snapshot/checkpoint filename
    /// after a lossy sanitisation step that maps every character outside
    /// `[A-Za-z0-9_-]` to `_`. To guarantee distinct on-disk paths for
    /// distinct projection identities, callers must supply names that
    /// survive the filter without collapsing — i.e. ASCII alphanumeric
    /// plus `-` and `_` only. Names containing other characters are
    /// accepted but each disallowed character is replaced with `_`, so
    /// e.g. `"foo bar"` and `"foo_bar"` share the same on-disk component
    /// and would collide.
    ///
    /// The constructor performs no I/O — the directory is created and the
    /// CHE-0043:R1 advisory `.lock` is acquired lazily on the first
    /// mutating call (CHE-0043:R2).
    pub fn new(dir: impl Into<PathBuf>, projection_name: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            projection_name: projection_name.into(),
            lock: Arc::new(OnceLock::new()),
            _projection: PhantomData,
        }
    }

    /// Backend root directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Stable projection identity used in file names and checkpoints.
    #[must_use]
    pub fn projection_name(&self) -> &str {
        &self.projection_name
    }

    /// Snapshot file path for `aggregate_id`.
    #[must_use]
    pub fn snapshot_path(&self, aggregate_id: AggregateId) -> PathBuf {
        self.dir.join(format!(
            "{}-{}.snapshot.msgpack",
            aggregate_id.get(),
            safe_file_component(&self.projection_name)
        ))
    }

    /// Checkpoint file path for `aggregate_id`.
    #[must_use]
    pub fn checkpoint_path(&self, aggregate_id: AggregateId) -> PathBuf {
        self.dir.join(format!(
            "{}-{}.checkpoint.msgpack",
            aggregate_id.get(),
            safe_file_component(&self.projection_name)
        ))
    }

    /// Path to the advisory `.lock` file (CHE-0043:R1) under this backend's
    /// store directory.
    fn lock_path(&self) -> PathBuf {
        self.dir.join(".lock")
    }

    /// Acquire the CHE-0043:R1 advisory exclusive lock on the store
    /// directory, lazily on first call (CHE-0043:R2). Subsequent calls
    /// on the same instance (or any clone, since the `OnceLock` is shared
    /// via `Arc`) are no-ops. Returns [`ProjectionError::StoreLocked`]
    /// (CHE-0043:R3) when contended.
    fn acquire_lock(&self) -> ProjectionResult<()> {
        if self.lock.get().is_some() {
            return Ok(());
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        let path = self.lock_path();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        match file.try_lock() {
            Ok(()) => {
                // Race-tolerant: if another thread on this process won
                // `set`, drop our handle (its lock subsumes ours).
                let _ = self.lock.set(file);
                Ok(())
            }
            Err(std::fs::TryLockError::WouldBlock) => Err(ProjectionError::StoreLocked),
            Err(std::fs::TryLockError::Error(e)) => {
                Err(ProjectionError::Infrastructure(Box::new(e)))
            }
        }
    }

    /// CHE-0047:R1 — remove orphaned `*.tmp` files from the store
    /// directory before the next mutation. Conservative semantics: a
    /// `.tmp` file is removed only when its corresponding non-tmp
    /// companion (`.tmp` → `.msgpack`) is absent — i.e. the
    /// previous `temp-file-then-rename` cycle crashed between write
    /// and rename. A `.tmp` whose companion exists is left in place
    /// because it may belong to a concurrent writer (defensive even
    /// though CHE-0043 fencing should already exclude that case).
    ///
    /// Called from `persist` and `rebuild_file` (via `project_to_file`)
    /// at scoped call sites — not from the constructor — so the sweep
    /// runs only when a mutation is imminent. The store directory is
    /// created lazily on lock acquisition; absence of the directory at
    /// sweep time is a no-op.
    async fn sweep_orphan_tmp(&self) -> ProjectionResult<()> {
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(ProjectionError::Infrastructure(Box::new(e))),
        };
        loop {
            let next = entries
                .next_entry()
                .await
                .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
            let Some(entry) = next else { break };
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("tmp") {
                continue;
            }
            let companion = path.with_extension("msgpack");
            match tokio::fs::metadata(&companion).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Companion absent → orphan, safe to remove.
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        // Tolerate races (e.g. another sweep already
                        // removed it) but surface other errors.
                        if e.kind() != std::io::ErrorKind::NotFound {
                            return Err(ProjectionError::Infrastructure(Box::new(e)));
                        }
                    }
                }
                Err(e) => return Err(ProjectionError::Infrastructure(Box::new(e))),
                Ok(_) => {
                    // Companion present → leave the .tmp alone.
                }
            }
        }
        Ok(())
    }
}

impl<P> FileProjectionStore<P>
where
    P: Serialize + DeserializeOwned,
{
    /// Persist `projection` and then its checkpoint.
    ///
    /// Order is snapshot-then-checkpoint with `fsync` + directory `fsync`
    /// at each step (CHE-0048 R1+R2, CHE-0032). R1 mandates the
    /// temp-file-then-rename snapshot write; R2 mandates the checkpoint
    /// is written strictly after the snapshot. A crash between the two
    /// leaves the snapshot present but the checkpoint absent; restart
    /// code must treat that case as "rebuild" rather than "trust snapshot".
    ///
    /// # Concurrency
    ///
    /// CHE-0048:R7 **mandates** per-aggregate write coordination via
    /// in-process per-aggregate locks consistent with CHE-0035:R1–R3.
    /// In v0.1 this crate does **not** provide that lock: `persist` is
    /// store-fenced under CHE-0043 (one writer process per store
    /// directory, surfaced as [`ProjectionError::StoreLocked`]) but is
    /// **not** safe under concurrent in-process callers for the same
    /// `(aggregate_id, projection_name)` pair — the temp file used by
    /// the internal atomic-write helper is derived deterministically
    /// from the destination path, so two concurrent in-process writers
    /// would race on the same temp path and on the final atomic rename.
    ///
    /// Until per-aggregate locking lands, **callers must coordinate
    /// writes for the same `aggregate_id` externally**, typically via
    /// the per-aggregate write lock already required by the
    /// single-writer-per-aggregate invariant (CHE-0006). Concurrent
    /// `persist` for *distinct* `(aggregate_id, projection_name)`
    /// pairs within the same store directory is safe.
    ///
    /// Per-aggregate lock implementation is tracked as a follow-up
    /// (bd `adr-fmt-8y4r`, discovered-from epic `adr-fmt-hh07`).
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Infrastructure`] for filesystem or
    /// serialization failures. Returns [`ProjectionError::StoreLocked`]
    /// (CHE-0043:R3) when the advisory `.lock` is held by another writer.
    pub async fn persist(
        &self,
        aggregate_id: AggregateId,
        projection: &P,
        last_sequence: u64,
    ) -> ProjectionResult<()> {
        self.persist_inner(aggregate_id, projection, last_sequence)
            .await
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self, projection),
        fields(
            aggregate_id = %aggregate_id.get(),
            projection_name = %self.projection_name,
            last_sequence,
        ),
    )]
    async fn persist_inner(
        &self,
        aggregate_id: AggregateId,
        projection: &P,
        last_sequence: u64,
    ) -> ProjectionResult<()> {
        self.acquire_lock()?;
        self.sweep_orphan_tmp().await?;
        let snapshot = rmp_serde::encode::to_vec_named(projection)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        write_atomic(&self.snapshot_path(aggregate_id), snapshot).await?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "snapshot_written",
            "snapshot persisted",
        );

        let checkpoint =
            ProjectionCheckpoint::new(aggregate_id, self.projection_name.clone(), last_sequence);
        let bytes = rmp_serde::encode::to_vec_named(&checkpoint)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        write_atomic(&self.checkpoint_path(aggregate_id), bytes).await?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "checkpoint_written",
            "checkpoint persisted",
        );
        Ok(())
    }

    /// Load a persisted projection snapshot, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::CorruptData`] when snapshot bytes cannot
    /// deserialize as `P`.
    pub async fn load_snapshot(&self, aggregate_id: AggregateId) -> ProjectionResult<Option<P>> {
        self.load_snapshot_inner(aggregate_id)
            .await
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self),
        fields(
            aggregate_id = %aggregate_id.get(),
            projection_name = %self.projection_name,
        ),
    )]
    async fn load_snapshot_inner(&self, aggregate_id: AggregateId) -> ProjectionResult<Option<P>> {
        let path = self.snapshot_path(aggregate_id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    target: "cherry_pit_projection",
                    "snapshot absent; caller will rebuild",
                );
                return Ok(None);
            }
            Err(e) => return Err(ProjectionError::Infrastructure(Box::new(e))),
        };
        rmp_serde::from_slice(&bytes)
            .map(Some)
            .map_err(|e| ProjectionError::CorruptData(Box::new(e)))
    }

    /// Load a persisted checkpoint, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::CorruptData`] when checkpoint bytes cannot
    /// deserialize or do not match this backend's identity.
    pub async fn load_checkpoint(
        &self,
        aggregate_id: AggregateId,
    ) -> ProjectionResult<Option<ProjectionCheckpoint>> {
        self.load_checkpoint_inner(aggregate_id)
            .await
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self),
        fields(
            aggregate_id = %aggregate_id.get(),
            projection_name = %self.projection_name,
        ),
    )]
    async fn load_checkpoint_inner(
        &self,
        aggregate_id: AggregateId,
    ) -> ProjectionResult<Option<ProjectionCheckpoint>> {
        let path = self.checkpoint_path(aggregate_id);
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    target: "cherry_pit_projection",
                    "checkpoint absent; caller will rebuild from beginning",
                );
                return Ok(None);
            }
            Err(e) => return Err(ProjectionError::Infrastructure(Box::new(e))),
        };
        let checkpoint: ProjectionCheckpoint =
            rmp_serde::from_slice(&bytes).map_err(|e| ProjectionError::CorruptData(Box::new(e)))?;
        if checkpoint.aggregate_id() != aggregate_id
            || checkpoint.projection_name() != self.projection_name
        {
            return Err(ProjectionError::CorruptData(
                "checkpoint identity mismatch".into(),
            ));
        }
        Ok(Some(checkpoint))
    }

    /// Delete snapshot and checkpoint files for `aggregate_id`.
    ///
    /// Order is the inverse of [`Self::persist`] (which writes
    /// snapshot then checkpoint): the checkpoint is removed first
    /// and durably synced before the snapshot is removed. This
    /// preserves the persist invariant `checkpoint ⇒ snapshot
    /// exists` across a crash mid-`delete` — a fast-resume path
    /// that consumes a checkpoint without re-rebuilding (planned
    /// for WU-4+) must never observe a checkpoint whose snapshot
    /// has already been removed.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Infrastructure`] for filesystem failures
    /// other than missing files.
    pub async fn delete(&self, aggregate_id: AggregateId) -> ProjectionResult<()> {
        self.delete_inner(aggregate_id)
            .await
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self),
        fields(
            aggregate_id = %aggregate_id.get(),
            projection_name = %self.projection_name,
        ),
    )]
    async fn delete_inner(&self, aggregate_id: AggregateId) -> ProjectionResult<()> {
        self.acquire_lock()?;
        remove_if_exists(self.checkpoint_path(aggregate_id)).await?;
        sync_dir(&self.dir).await?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "checkpoint_removed",
            "checkpoint deleted",
        );
        remove_if_exists(self.snapshot_path(aggregate_id)).await?;
        sync_dir(&self.dir).await?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "snapshot_removed",
            "snapshot deleted",
        );
        Ok(())
    }

    #[cfg(test)]
    async fn persist_crash_after_snapshot(
        &self,
        aggregate_id: AggregateId,
        projection: &P,
    ) -> ProjectionResult<()> {
        self.acquire_lock()?;
        let snapshot = rmp_serde::encode::to_vec_named(projection)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        write_atomic(&self.snapshot_path(aggregate_id), snapshot).await?;
        Err(ProjectionError::Infrastructure(
            "simulated crash after snapshot before checkpoint".into(),
        ))
    }
}

/// Ephemeral projection backend for tests and short-lived views.
///
/// The backend is parameterised by `P: Projection` and owns a single `P`
/// value in memory. It performs no durable writes and uses no dynamic
/// projection dispatch.
///
/// # Relationship to CHE-0048:R5
///
/// CHE-0048:R5 prescribes a concurrent hash map keyed by
/// `(aggregate_id, projection_name)`. In v0.1 the single-aggregate,
/// single-projection-per-driver-instance scope of CHE-0048:R6 collapses
/// that key to exactly one tuple per driver instance, so this backend
/// stores one `P` directly rather than a degenerate one-entry map. The
/// other two R5 obligations — no durable state and rebuild-from-`EventStore`
/// — are satisfied unchanged. Multi-projection composition and the
/// keyed-map shape are deferred until CHE-0048:R6 is relaxed (tracked as
/// a follow-up under epic `adr-fmt-hh07`; targeted at WU-5).
///
/// # Examples
///
/// ```
/// use cherry_pit_projection::InMemoryProjection;
/// use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
///
/// #[derive(Default)]
/// struct CounterView { total: u64 }
/// impl Projection for CounterView {
///     type Event = CounterEvent;
///     fn apply(&mut self, _event: &EventEnvelope<Self::Event>) { self.total += 1; }
/// }
///
/// let projection = InMemoryProjection::<CounterView>::new();
/// assert_eq!(projection.get().total, 0);
/// ```
#[derive(Debug, Clone)]
pub struct InMemoryProjection<P: Projection> {
    projection: P,
}

impl<P: Projection> InMemoryProjection<P> {
    /// Create an empty in-memory projection from `P::default()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            projection: P::default(),
        }
    }

    /// Borrow the current in-memory projection state.
    #[must_use]
    pub const fn get(&self) -> &P {
        &self.projection
    }

    /// Replace the in-memory projection state.
    pub fn replace(&mut self, projection: P) {
        self.projection = projection;
    }
}

impl<P: Projection> Default for InMemoryProjection<P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Driver that rebuilds a projection from a typed event store.
///
/// `ProjectionDriver` is generic over a single `P: Projection` and a typed
/// `S: EventStore<Event = P::Event>` — there is no `Box<dyn Projection>` and
/// no `Box<dyn EventStore>` (CHE-0048 R3, CHE-0005 R1). [`replay`](Self::replay)
/// loads the full stream, runs [`cherry_pit_core::EventEnvelope::validate_stream`]
/// (CHE-0042 R4), then folds events into `P::default()`.
///
/// # Examples
///
/// Construct a driver and rebuild a file-backed projection (compile-checked
/// signature only — needs a concrete `EventStore` impl to run).
///
/// `no_run` is structurally justified here, not a convenience: the example
/// is a **constructor-without-IO** pattern. `ProjectionDriver::rebuild_file`
/// requires an `S: EventStore<Event = P::Event>`, but
/// `cherry-pit-projection` exports no concrete `EventStore` impl and pulls
/// in no in-memory `EventStore` as a dev-dep at doctest scope (CHE-0048 R3 +
/// CHE-0005 R1 keep both traits generic, never `Box<dyn _>`). Promoting
/// this doctest to runnable would require either exporting a test-only
/// store from this crate (public-API surface change, out of scope per
/// FOCUS §8) or taking a new dev-dep solely for doctest coverage. The
/// signature-only check is the maximum coverage available without those.
///
/// ```no_run
/// use cherry_pit_core::{
///     AggregateId, DomainEvent, EventEnvelope, EventStore, Projection,
/// };
/// use cherry_pit_projection::{
///     FileProjectionStore, ProjectionDriver, ProjectionResult,
/// };
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum CounterEvent { Incremented }
/// impl DomainEvent for CounterEvent {
///     fn event_type(&self) -> &'static str { "counter.incremented" }
/// }
///
/// #[derive(Default, Clone, Serialize, Deserialize)]
/// struct CounterView { total: u64 }
/// impl Projection for CounterView {
///     type Event = CounterEvent;
///     fn apply(&mut self, _: &EventEnvelope<Self::Event>) { self.total += 1; }
/// }
///
/// async fn rebuild<S>(store: S, id: AggregateId) -> ProjectionResult<CounterView>
/// where
///     S: EventStore<Event = CounterEvent>,
/// {
///     let driver = ProjectionDriver::<CounterView, _>::new(store);
///     let backend = FileProjectionStore::<CounterView>::new(
///         "projection-store",
///         "counter_view",
///     );
///     driver.rebuild_file(id, &cherry_pit_core::CorrelationContext::none(), &backend).await
/// }
/// ```
pub struct ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    store: S,
    _projection: PhantomData<fn() -> P>,
}

impl<P, S> ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    /// Create a driver over a typed event store.
    #[must_use]
    pub const fn new(store: S) -> Self {
        Self {
            store,
            _projection: PhantomData,
        }
    }

    /// Replay all events for `aggregate_id` into a fresh `P::default()`.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::CorruptData`] when the loaded stream is
    /// not valid for `aggregate_id`, and [`ProjectionError::Infrastructure`]
    /// when the underlying store load fails.
    pub async fn replay(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
    ) -> ProjectionResult<P> {
        self.replay_inner(aggregate_id, correlation)
            .await
            .map(|(projection, _)| projection)
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self, correlation),
        fields(
            aggregate_id = %aggregate_id.get(),
            correlation_id = ?correlation.correlation_id(),
            causation_id = ?correlation.causation_id(),
        ),
    )]
    async fn replay_inner(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
    ) -> ProjectionResult<(P, u64)> {
        let stream = self
            .store
            .load(aggregate_id)
            .await
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        cherry_pit_core::EventEnvelope::validate_stream(aggregate_id, &stream)
            .map_err(|e| ProjectionError::CorruptData(Box::new(e)))?;
        let mut projection = P::default();
        for event in &stream {
            tracing::trace!(
                target: "cherry_pit_projection",
                event_id = %event.event_id(),
                sequence = event.sequence().get(),
                "applying event",
            );
            projection.apply(event);
        }
        let last_sequence = stream.last().map_or(0, |e| e.sequence().get());
        Ok((projection, last_sequence))
    }

    /// Replay from the event store and persist the resulting snapshot and checkpoint.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] when loading, validating, applying, or
    /// persisting fails.
    pub async fn project_to_file(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
        backend: &FileProjectionStore<P>,
    ) -> ProjectionResult<P>
    where
        P: Serialize + DeserializeOwned + Clone,
    {
        self.project_to_file_inner(aggregate_id, correlation, backend)
            .await
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self, correlation, backend),
        fields(
            aggregate_id = %aggregate_id.get(),
            correlation_id = ?correlation.correlation_id(),
            causation_id = ?correlation.causation_id(),
        ),
    )]
    async fn project_to_file_inner(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
        backend: &FileProjectionStore<P>,
    ) -> ProjectionResult<P>
    where
        P: Serialize + DeserializeOwned + Clone,
    {
        let (projection, last_sequence) = self.replay_inner(aggregate_id, correlation).await?;
        backend
            .persist(aggregate_id, &projection, last_sequence)
            .await?;
        Ok(projection)
    }

    /// Delete existing file state, replay from the event store, and persist fresh state.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError`] when deletion, loading, validation, or
    /// persistence fails.
    pub async fn rebuild_file(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
        backend: &FileProjectionStore<P>,
    ) -> ProjectionResult<P>
    where
        P: Serialize + DeserializeOwned + Clone,
    {
        self.rebuild_file_inner(aggregate_id, correlation, backend)
            .await
            .inspect_err(ProjectionError::emit_event)
    }

    #[tracing::instrument(
        skip(self, correlation, backend),
        fields(
            aggregate_id = %aggregate_id.get(),
            correlation_id = ?correlation.correlation_id(),
            causation_id = ?correlation.causation_id(),
        ),
    )]
    async fn rebuild_file_inner(
        &self,
        aggregate_id: AggregateId,
        correlation: &cherry_pit_core::CorrelationContext,
        backend: &FileProjectionStore<P>,
    ) -> ProjectionResult<P>
    where
        P: Serialize + DeserializeOwned + Clone,
    {
        backend.acquire_lock()?;
        backend.sweep_orphan_tmp().await?;
        backend.delete(aggregate_id).await?;
        self.project_to_file(aggregate_id, correlation, backend)
            .await
    }
}

/// Sanitise an arbitrary string for inclusion in a filename component.
///
/// Maps every character outside `[A-Za-z0-9_-]` to `_`. The mapping is
/// **lossy and non-injective**: distinct inputs collapse to identical
/// outputs (e.g. `"foo bar"` and `"foo_bar"` both yield `"foo_bar"`).
/// Callers controlling the input domain (currently only `projection_name`
/// supplied to [`FileProjectionStore::new`]) must restrict inputs to the
/// allowed alphabet to guarantee distinct on-disk paths.
fn safe_file_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

async fn write_atomic(path: &Path, bytes: Vec<u8>) -> ProjectionResult<()> {
    let parent = path.parent().ok_or_else(|| {
        ProjectionError::Infrastructure("projection path has no parent directory".into())
    })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
    let tmp_path = path.with_extension("tmp");
    let tmp_for_write = tmp_path.clone();
    tokio::task::spawn_blocking(move || -> ProjectionResult<()> {
        use std::io::Write as _;

        let mut tmp_file = std::fs::File::create(&tmp_for_write)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        tmp_file
            .write_all(&bytes)
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        tmp_file
            .sync_all()
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        Ok(())
    })
    .await
    .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))??;

    if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
        let _ = tokio::fs::remove_file(&tmp_path).await;
        return Err(ProjectionError::Infrastructure(Box::new(e)));
    }
    sync_dir(parent).await
}

async fn remove_if_exists(path: PathBuf) -> ProjectionResult<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ProjectionError::Infrastructure(Box::new(e))),
    }
}

async fn sync_dir(path: &Path) -> ProjectionResult<()> {
    let dir = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> ProjectionResult<()> {
        let file =
            std::fs::File::open(dir).map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        file.sync_all()
            .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
        Ok(())
    })
    .await
    .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherry_pit_core::{
        CorrelationContext, DomainEvent, EventEnvelope, StoreCreateResult, StoreError,
    };
    use serde::Deserialize;
    use std::num::NonZeroU64;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum CounterEvent {
        Incremented,
    }

    impl DomainEvent for CounterEvent {
        fn event_type(&self) -> &'static str {
            "counter.incremented"
        }
    }

    #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct CounterView {
        total: u64,
    }

    impl Projection for CounterView {
        type Event = CounterEvent;

        fn apply(&mut self, _event: &EventEnvelope<Self::Event>) {
            self.total += 1;
        }
    }

    struct StaticStore {
        stream: Mutex<Vec<EventEnvelope<CounterEvent>>>,
    }

    impl StaticStore {
        fn new(stream: Vec<EventEnvelope<CounterEvent>>) -> Self {
            Self {
                stream: Mutex::new(stream),
            }
        }
    }

    impl EventStore for StaticStore {
        type Event = CounterEvent;

        async fn load(
            &self,
            _id: AggregateId,
        ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
            Ok(self.stream.lock().expect("stream mutex").clone())
        }

        async fn create(
            &self,
            _events: Vec<Self::Event>,
            _context: CorrelationContext,
        ) -> StoreCreateResult<Self::Event> {
            Err(StoreError::Infrastructure("unused".into()))
        }

        async fn append(
            &self,
            _id: AggregateId,
            _expected_sequence: NonZeroU64,
            _events: Vec<Self::Event>,
            _context: CorrelationContext,
        ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
            Err(StoreError::Infrastructure("unused".into()))
        }
    }

    fn aggregate_id(value: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(value).expect("non-zero id"))
    }

    fn envelope(id: AggregateId, sequence: u64) -> EventEnvelope<CounterEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            id,
            NonZeroU64::new(sequence).expect("non-zero sequence"),
            jiff::Timestamp::now(),
            None,
            None,
            CounterEvent::Incremented,
        )
        .expect("valid envelope")
    }

    #[test]
    fn inmem_defaults_to_empty_projection() {
        let backend = InMemoryProjection::<CounterView>::new();
        assert_eq!(backend.get().total, 0);
    }

    #[test]
    fn inmem_replace_updates_ephemeral_state() {
        let mut backend = InMemoryProjection::<CounterView>::new();
        backend.replace(CounterView { total: 3 });
        assert_eq!(backend.get().total, 3);
    }

    #[tokio::test]
    async fn inmem_driver_replays_valid_stream() {
        let id = aggregate_id(1);
        let store = StaticStore::new(vec![envelope(id, 1), envelope(id, 2)]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);

        let projection = driver
            .replay(id, &CorrelationContext::none())
            .await
            .expect("replay succeeds");

        assert_eq!(projection.total, 2);
    }

    #[tokio::test]
    async fn file_backend_writes_snapshot_then_checkpoint_files() {
        let id = aggregate_id(1);
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
        let projection = CounterView { total: 7 };

        backend.persist(id, &projection, 3).await.expect("persist");

        assert!(backend.snapshot_path(id).exists());
        assert!(backend.checkpoint_path(id).exists());
        assert_eq!(
            backend.load_snapshot(id).await.expect("snapshot"),
            Some(projection)
        );
        let checkpoint = backend
            .load_checkpoint(id)
            .await
            .expect("checkpoint")
            .expect("checkpoint exists");
        assert_eq!(checkpoint.last_sequence(), 3);
        assert_eq!(checkpoint.projection_name(), "counter");
    }

    /// CHE-0043:R1–R3 — a second backend instance pointed at the same store
    /// directory must fail-fast with [`ProjectionError::StoreLocked`] on
    /// any mutating call while the first instance holds the advisory lock.
    #[tokio::test]
    async fn che_0043_second_backend_on_same_dir_errors_with_store_locked() {
        let id = aggregate_id(1);
        let dir = tempfile::tempdir().expect("tempdir");
        let first = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
        let second = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

        // First persist acquires the .lock lazily and succeeds.
        first
            .persist(id, &CounterView { total: 1 }, 1)
            .await
            .expect("first persist");

        // Second instance attempts to acquire the same .lock — must fail.
        let err = second
            .persist(id, &CounterView { total: 2 }, 2)
            .await
            .expect_err("second persist must contend");
        assert!(matches!(err, ProjectionError::StoreLocked));
        assert_eq!(err.category(), ErrorCategory::Retryable);
    }

    /// CHE-0043:R2 — lock acquisition is lazy. Constructing a backend
    /// against a directory currently locked by another instance must not
    /// fail in itself; only the first mutating call surfaces contention.
    #[tokio::test]
    async fn che_0043_construction_is_lazy_no_lock_on_new() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
        first
            .persist(aggregate_id(1), &CounterView { total: 1 }, 1)
            .await
            .expect("first persist");

        // Constructing a second instance must not error — R2 forbids
        // eager acquisition on construction.
        let _second = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
    }

    /// CHE-0047:R1 — orphan `.tmp` files from a previous crashed
    /// rename cycle are swept before the next mutation. Stale
    /// `*.tmp` whose `*.msgpack` companion is absent must be removed
    /// at the top of `persist`.
    #[tokio::test]
    async fn che_0047_r1_orphan_tmp_swept_on_next_persist() {
        let id = aggregate_id(1);
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

        // Plant a stale .tmp matching the deterministic tmp scheme used
        // by write_atomic: snapshot_path.with_extension("tmp").
        let stale_tmp = backend.snapshot_path(id).with_extension("tmp");
        tokio::fs::create_dir_all(dir.path())
            .await
            .expect("ensure dir");
        tokio::fs::write(&stale_tmp, b"stale bytes")
            .await
            .expect("plant stale tmp");
        assert!(stale_tmp.exists(), "stale tmp planted");

        // Plant a second orphan tmp for an unrelated aggregate to
        // ensure the sweep is dir-wide, not single-path.
        let other = backend.snapshot_path(aggregate_id(2)).with_extension("tmp");
        tokio::fs::write(&other, b"other stale")
            .await
            .expect("plant other stale");

        // Persist should sweep both stale .tmp files before writing.
        backend
            .persist(id, &CounterView { total: 9 }, 9)
            .await
            .expect("persist after sweep");

        assert!(!stale_tmp.exists(), "stale tmp removed by sweep");
        assert!(!other.exists(), "sibling stale tmp also removed");
        assert!(backend.snapshot_path(id).exists(), "new snapshot written");
        assert!(
            backend.checkpoint_path(id).exists(),
            "new checkpoint written"
        );
        assert_eq!(
            backend.load_snapshot(id).await.expect("load snapshot"),
            Some(CounterView { total: 9 })
        );
    }

    /// CHE-0047:R1 — conservative semantics. A `.tmp` whose `.msgpack`
    /// companion exists must be left in place (it may belong to a
    /// concurrent or in-flight writer, even though CHE-0043 fencing
    /// should already exclude that). Uses an unrelated `foo.msgpack` +
    /// `foo.tmp` pair so the next persist (which renames *through* the
    /// snapshot's own deterministic tmp path) cannot itself remove the
    /// planted `.tmp`.
    #[tokio::test]
    async fn che_0047_r1_sweep_preserves_tmp_with_existing_companion() {
        let id = aggregate_id(1);
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

        // Land an initial snapshot so the dir exists and is locked.
        backend
            .persist(id, &CounterView { total: 1 }, 1)
            .await
            .expect("initial persist");

        // Plant an *unrelated* companion pair: foo.msgpack + foo.tmp.
        // The next persist will not touch these paths, so survival of
        // foo.tmp depends only on the sweep's conservative rule.
        let live_companion = dir.path().join("foo.msgpack");
        let live_tmp = dir.path().join("foo.tmp");
        tokio::fs::write(&live_companion, b"companion lives")
            .await
            .expect("plant companion");
        tokio::fs::write(&live_tmp, b"tmp with live companion")
            .await
            .expect("plant live tmp");

        // Trigger sweep via another persist; companion-bearing tmp must survive.
        backend
            .persist(id, &CounterView { total: 2 }, 2)
            .await
            .expect("second persist");

        assert!(
            live_tmp.exists(),
            "conservative sweep preserved .tmp with live companion"
        );
        assert!(live_companion.exists(), "live companion untouched");
    }

    #[tokio::test]
    async fn validation_bad_stream_returns_typed_error_without_partial_application() {
        let id = aggregate_id(1);
        let store = StaticStore::new(vec![envelope(id, 2)]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);

        let err = driver
            .replay(id, &CorrelationContext::none())
            .await
            .expect_err("invalid stream rejected");

        assert!(matches!(err, ProjectionError::CorruptData(_)));
        assert_eq!(err.category(), ErrorCategory::Terminal);
    }

    #[tokio::test]
    async fn rebuild_deletes_existing_state_and_recreates_equal_snapshot() {
        let id = aggregate_id(1);
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
        let store = StaticStore::new(vec![envelope(id, 1), envelope(id, 2)]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);

        let first = driver
            .project_to_file(id, &cherry_pit_core::CorrelationContext::none(), &backend)
            .await
            .expect("initial projection");
        backend
            .persist(id, &CounterView { total: 999 }, 999)
            .await
            .expect("overwrite stale state");
        let rebuilt = driver
            .rebuild_file(id, &cherry_pit_core::CorrelationContext::none(), &backend)
            .await
            .expect("rebuild projection");

        assert_eq!(rebuilt, first);
        assert_eq!(
            backend.load_snapshot(id).await.expect("snapshot"),
            Some(first)
        );
        assert_eq!(
            backend
                .load_checkpoint(id)
                .await
                .expect("checkpoint")
                .expect("checkpoint exists")
                .last_sequence(),
            2
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(256))]

        #[test]
        fn crash_between_snapshot_and_checkpoint_replays_events_instead_of_skipping(count in 1_u64..20) {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(async move {
                let id = aggregate_id(1);
                let dir = tempfile::tempdir().expect("tempdir");
                let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
                let crashed_projection = CounterView { total: count };

                let crash = backend
                    .persist_crash_after_snapshot(id, &crashed_projection)
                    .await;
                assert!(crash.is_err());
                assert!(backend.snapshot_path(id).exists());
                assert!(!backend.checkpoint_path(id).exists());

                let stream = (1..=count).map(|seq| envelope(id, seq)).collect();
                let store = StaticStore::new(stream);
                let driver = ProjectionDriver::<CounterView, _>::new(store);
                let rebuilt = driver
                    .rebuild_file(id, &cherry_pit_core::CorrelationContext::none(), &backend)
                    .await
                    .expect("restart rebuild does not trust missing checkpoint");

                assert_eq!(rebuilt.total, count);
                assert_eq!(
                    backend
                        .load_checkpoint(id)
                        .await
                        .expect("checkpoint")
                        .expect("checkpoint exists")
                        .last_sequence(),
                    count
                );
            });
        }

        /// CHE-0048:R3 — `apply` is deterministic and idempotent over a
        /// fixed event stream: replaying the same envelope sequence twice
        /// against fresh projections yields equal final states. Two
        /// independent replays exercise the property without depending on
        /// driver-internal retry semantics.
        #[test]
        fn r3_apply_is_idempotent_over_a_fixed_event_stream(count in 1_u64..32) {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(async move {
                let id = aggregate_id(1);
                let stream: Vec<EventEnvelope<CounterEvent>> =
                    (1..=count).map(|seq| envelope(id, seq)).collect();

                let store_a = StaticStore::new(stream.clone());
                let driver_a = ProjectionDriver::<CounterView, _>::new(store_a);
                let first = driver_a
                    .replay(id, &CorrelationContext::none())
                    .await
                    .expect("first replay");

                let store_b = StaticStore::new(stream);
                let driver_b = ProjectionDriver::<CounterView, _>::new(store_b);
                let second = driver_b
                    .replay(id, &CorrelationContext::none())
                    .await
                    .expect("second replay");

                assert_eq!(first, second);
                assert_eq!(first.total, count);
            });
        }
    }
}
