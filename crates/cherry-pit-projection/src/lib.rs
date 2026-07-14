//! Read-side adapters that drive [`cherry_pit_core::Projection`] from a
//! typed [`cherry_pit_core::EventStore`] per CHE-0048.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs::File;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use cherry_pit_core::{
    AggregateId, CorrelationContext, ErrorCategory, EventEnvelope, EventStore, Projection,
};
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
/// let four = NonZeroU64::new(4).unwrap();
///
/// store.persist(id, &CounterView { total: 4 }, four).await.unwrap();
///
/// let snapshot = store.load_snapshot(id).await.unwrap();
/// assert_eq!(snapshot, Some(CounterView { total: 4 }));
///
/// let checkpoint = store.load_checkpoint(id).await.unwrap().unwrap();
/// assert_eq!(checkpoint.last_sequence(), four);
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
    /// Serialises concurrent first-time acquirers of [`Self::lock`]
    /// (two clones calling a mutating method simultaneously before the
    /// `OnceLock` is set would otherwise race on `try_lock`, with the
    /// loser falsely surfacing [`ProjectionError::StoreLocked`]). After
    /// the `OnceLock` is set the gate is no longer taken — the fast
    /// path returns from [`Self::acquire_lock`] without entering the
    /// blocking section. Wrapped in `Arc` so clones share state.
    acquire_gate: Arc<StdMutex<()>>,
    /// Per-aggregate in-process write locks (CHE-0048:R7). Maps each
    /// active `aggregate_id` to a `tokio::sync::Mutex` held across the
    /// whole `persist` / `delete` / `rebuild_file` critical section,
    /// serialising concurrent in-process writers targeting the
    /// **same** aggregate (the deterministic temp-file name would
    /// otherwise race on rename). Distinct aggregates proceed in
    /// parallel; cross-aggregate non-interference is further enforced
    /// by [`Self::sweep_orphan_tmp`].
    ///
    /// The outer `std::sync::Mutex` guards only the lookup-or-insert
    /// (never held across an `await`); the returned per-aggregate
    /// `tokio::sync::Mutex` is the actual critical section. Never
    /// pruned — locks outlive any single call and are reused, mirroring
    /// `Arc<OnceLock<File>>`'s share-across-clones pattern (clones are
    /// the same backend identity per CHE-0048:R6).
    aggregate_locks: Arc<StdMutex<HashMap<AggregateId, Arc<tokio::sync::Mutex<()>>>>>,
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
            acquire_gate: Arc::new(StdMutex::new(())),
            aggregate_locks: Arc::new(StdMutex::new(HashMap::new())),
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
    ///
    /// Concurrent first-time acquirers (two clones of the same
    /// `FileProjectionStore` calling `persist` simultaneously before
    /// the `OnceLock` is set) are serialised by the
    /// `aggregate_locks` registry mutex used as a coarse gate: only
    /// one acquirer at a time enters the `try_lock` path, so the
    /// second's `try_lock` does not falsely race against the first's
    /// in-flight file-handle. After the `OnceLock` is set the fast
    /// path returns without taking any mutex.
    async fn acquire_lock(&self) -> ProjectionResult<()> {
        if self.lock.get().is_some() {
            return Ok(());
        }
        let dir = self.dir.clone();
        let path = self.lock_path();
        let lock = Arc::clone(&self.lock);
        let gate = Arc::clone(&self.acquire_gate);
        tokio::task::spawn_blocking(move || -> ProjectionResult<()> {
            std::fs::create_dir_all(&dir)
                .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
            let _gate = gate.lock().expect("acquire_gate poisoned");
            if lock.get().is_some() {
                return Ok(());
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
            match file.try_lock() {
                Ok(()) => {
                    let _ = lock.set(file);
                    Ok(())
                }
                Err(std::fs::TryLockError::WouldBlock) => Err(ProjectionError::StoreLocked),
                Err(std::fs::TryLockError::Error(e)) => {
                    Err(ProjectionError::Infrastructure(Box::new(e)))
                }
            }
        })
        .await
        .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?
    }

    /// Look up the per-aggregate write lock for `aggregate_id`,
    /// inserting a fresh `tokio::sync::Mutex` if this is the first
    /// write for the aggregate. Per CHE-0048:R7 the returned mutex is
    /// held across the entire `persist` / `delete` / `rebuild_file`
    /// critical section so two concurrent in-process callers targeting
    /// the **same** aggregate serialise their writes.
    ///
    /// The outer `std::sync::Mutex` on the registry is held only for
    /// the lookup-or-insert (microseconds, never across an `await`).
    /// The inner `tokio::sync::Mutex` is the actual per-aggregate lock.
    ///
    /// # Panics
    ///
    /// Panics if the registry mutex is poisoned — only possible if a
    /// previous registry access panicked, which would indicate an
    /// unrecoverable backend-wide failure.
    fn aggregate_lock(&self, aggregate_id: AggregateId) -> Arc<tokio::sync::Mutex<()>> {
        let mut registry = self
            .aggregate_locks
            .lock()
            .expect("aggregate-lock registry mutex poisoned");
        Arc::clone(
            registry
                .entry(aggregate_id)
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// Remove orphaned `*.tmp` files belonging to **this aggregate**
    /// before the next mutation (CHE-0047:R1, CHE-0048:R7). Conservative:
    /// a `.tmp` is removed only when its `.msgpack` companion is absent
    /// — i.e. a temp-file-then-rename cycle (CHE-0032) crashed between
    /// write and rename. A `.tmp` whose companion exists is left in
    /// place (may be an in-flight retry; the per-aggregate lock should
    /// already exclude that case).
    ///
    /// Only `.tmp` files named `{aggregate_id}-*` are eligible, matching
    /// the `{aggregate_id}-{projection_name}.{snapshot|checkpoint}.tmp`
    /// convention; other aggregates' tmp files are untouched, so a
    /// `persist` for aggregate A cannot reap an in-flight tmp for
    /// aggregate B (the directory-wide race CHE-0048:R7 scopes against).
    ///
    /// Called from `persist`, `delete`, `rebuild_file`, not the
    /// constructor, so it runs only when a mutation is imminent. A
    /// missing store directory is a no-op.
    async fn sweep_orphan_tmp(&self, aggregate_id: AggregateId) -> ProjectionResult<()> {
        let prefix = format!("{}-", aggregate_id.get());
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
            let belongs_to_target = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&prefix));
            if !belongs_to_target {
                continue;
            }
            let companion = path.with_extension("msgpack");
            match tokio::fs::metadata(&companion).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if let Err(e) = tokio::fs::remove_file(&path).await
                        && e.kind() != std::io::ErrorKind::NotFound
                    {
                        return Err(ProjectionError::Infrastructure(Box::new(e)));
                    }
                }
                Err(e) => return Err(ProjectionError::Infrastructure(Box::new(e))),
                Ok(_) => {}
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
    /// at each step (CHE-0048:R1, CHE-0048:R2, CHE-0032). R1 mandates the
    /// temp-file-then-rename write; R2 mandates the checkpoint is written
    /// strictly after the snapshot. A crash between the two leaves the
    /// snapshot present but the checkpoint absent; restart code must
    /// treat that as "rebuild" rather than "trust snapshot".
    ///
    /// # Concurrency
    ///
    /// Per-aggregate writes are serialised in-process via
    /// [`Self::aggregate_lock`] (CHE-0048:R7); concurrent `persist` calls
    /// for distinct `(aggregate_id, projection_name)` pairs are safe.
    /// Across processes, `persist` is store-fenced under CHE-0043 (one
    /// writer process per store directory), surfaced as
    /// [`ProjectionError::StoreLocked`] on contention.
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
        last_sequence: NonZeroU64,
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
            last_sequence = last_sequence.get(),
        ),
    )]
    async fn persist_inner(
        &self,
        aggregate_id: AggregateId,
        projection: &P,
        last_sequence: NonZeroU64,
    ) -> ProjectionResult<()> {
        self.acquire_lock().await?;
        let agg_lock = self.aggregate_lock(aggregate_id);
        let _guard = agg_lock.lock().await;
        self.sweep_orphan_tmp(aggregate_id).await?;
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
        self.acquire_lock().await?;
        let agg_lock = self.aggregate_lock(aggregate_id);
        let _guard = agg_lock.lock().await;
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
        self.acquire_lock().await?;
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
/// `S: EventStore<Event = P::Event>` — never `Box<dyn _>` (CHE-0048:R3,
/// CHE-0005:R1). [`replay`](Self::replay) loads the full stream, runs
/// [`cherry_pit_core::EventEnvelope::validate_stream`] (CHE-0042:R4), then
/// folds events into `P::default()`.
///
/// # Examples
///
/// Construct a driver and rebuild a file-backed projection
/// (`no_run`: signature-only, since this crate exports no concrete
/// `EventStore` impl to drive the doctest, keeping both traits generic
/// per CHE-0048:R3 + CHE-0005:R1 rather than adding a dev-dep solely
/// for doctest coverage).
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
    ) -> ProjectionResult<(P, Option<NonZeroU64>)> {
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
        let last_sequence = stream.last().map(EventEnvelope::sequence);
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
        if let Some(seq) = last_sequence {
            backend.persist(aggregate_id, &projection, seq).await?;
        }
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
        backend.acquire_lock().await?;
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

/// Extension trait adding per-event projection application on top of
/// [`ProjectionDriver`]'s replay-only surface.
///
/// `ProjectionDriver` ships `replay`, `project_to_file`, `rebuild_file`
/// — all stream-level operations. Live publish handlers need a
/// single-envelope entry point for incremental projection updates;
/// `apply_one` provides that without modifying CHE-0048's driver (C14).
///
/// The default impl simply delegates to [`Projection::apply`] on a
/// caller-owned mutable projection — the driver itself is stateless
/// w.r.t. the live projection (it owns only the store binding). This
/// preserves single-writer-per-aggregate (CHE-0006) by leaving the
/// projection state where the consumer chooses to keep it.
///
/// Per CHE-0057:R4 this trait must never appear as a trait object;
/// the workspace tripwire (ripgrep on `Box`+`dyn`+the trait name across
/// `crates/`) enforces the discipline.
pub trait ProjectionDriverExt<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    /// Apply a single event envelope to a caller-owned projection.
    ///
    /// Synchronous per CHE-0018:R1 — `Projection::apply` is sync.
    fn apply_one(&self, projection: &mut P, envelope: &EventEnvelope<P::Event>) {
        projection.apply(envelope);
    }

    /// Replay the entire stream into a fresh `P::default()`.
    ///
    /// Pass-through to [`ProjectionDriver::replay`] for ergonomic
    /// access through the extension trait surface.
    ///
    /// # Errors
    ///
    /// Surfaces [`ProjectionError`] from the underlying driver.
    fn replay_all(
        &self,
        aggregate_id: AggregateId,
        correlation: &CorrelationContext,
    ) -> impl std::future::Future<Output = ProjectionResult<P>> + Send;
}

impl<P, S> ProjectionDriverExt<P, S> for ProjectionDriver<P, S>
where
    P: Projection,
    S: EventStore<Event = P::Event>,
{
    fn replay_all(
        &self,
        aggregate_id: AggregateId,
        correlation: &CorrelationContext,
    ) -> impl std::future::Future<Output = ProjectionResult<P>> + Send {
        self.replay(aggregate_id, correlation)
    }
}

/// Heterogeneous fixed-arity tuple of [`ProjectionDriver`] instances.
///
/// Each tuple element is a distinct `ProjectionDriver<Pn, Sn>` where
/// every `(Pn, Sn)` pair is independent — the tuple shape preserves
/// per-projection type discipline (no `Box<dyn Projection>`, CHE-0005:R1).
///
/// v0.1 ships arities **0, 1 and 2** which suffice for the
/// ergonomic-benchmark gate (2-aggregate composition). Higher
/// arities up to ~8 are tracked as a `// FOLLOW-UP S7` extension gated
/// by the ergonomic benchmark — if the benchmark passes at arity 2 with
/// comfortable headroom, macro-expansion to arity 8 is purely mechanical
/// and lands in S7.
///
/// The trait is currently a marker — driver-level operations
/// (`apply_one`, `replay_all`) are exercised on the individual elements
/// via destructuring or pattern matching at the consumer site.
pub trait ProjectionDriverTuple {
    /// Number of projections in the tuple. Const-folded at the call
    /// site so consumers can `assert!(<T as ProjectionDriverTuple>::ARITY == 2)`.
    const ARITY: usize;
}

impl<P1, S1> ProjectionDriverTuple for (ProjectionDriver<P1, S1>,)
where
    P1: Projection,
    S1: EventStore<Event = P1::Event>,
{
    const ARITY: usize = 1;
}

impl<P1, S1, P2, S2> ProjectionDriverTuple for (ProjectionDriver<P1, S1>, ProjectionDriver<P2, S2>)
where
    P1: Projection,
    S1: EventStore<Event = P1::Event>,
    P2: Projection,
    S2: EventStore<Event = P2::Event>,
{
    const ARITY: usize = 2;
}

/// Marker for "no projections wired" — used when `App::new` is called
/// without projection parameters. The unit type implements
/// [`ProjectionDriverTuple`] with arity 0 so an empty composition is
/// expressible without special-casing in `App`.
impl ProjectionDriverTuple for () {
    const ARITY: usize = 0;
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

    fn seq(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("non-zero sequence")
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

        backend
            .persist(id, &projection, seq(3))
            .await
            .expect("persist");

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
        assert_eq!(checkpoint.last_sequence(), seq(3));
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

        first
            .persist(id, &CounterView { total: 1 }, seq(1))
            .await
            .expect("first persist");

        let err = second
            .persist(id, &CounterView { total: 2 }, seq(2))
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
            .persist(aggregate_id(1), &CounterView { total: 1 }, seq(1))
            .await
            .expect("first persist");

        let _second = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
    }

    /// CHE-0047:R1 — orphan `.tmp` files from a previous crashed
    /// rename cycle belonging to **this aggregate** are swept before
    /// the next mutation. Stale `*.tmp` whose `*.msgpack` companion is
    /// absent must be removed at the top of `persist` *if and only if*
    /// the tmp belongs to the target aggregate (CHE-0048:R7 aggregate-
    /// scoped sweep). A different aggregate's orphan tmp is left
    /// untouched — that aggregate's own next `persist` will reap it.
    #[tokio::test]
    async fn che_0047_r1_orphan_tmp_swept_on_next_persist() {
        let id = aggregate_id(1);
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

        let stale_tmp = backend.snapshot_path(id).with_extension("tmp");
        tokio::fs::create_dir_all(dir.path())
            .await
            .expect("ensure dir");
        tokio::fs::write(&stale_tmp, b"stale bytes")
            .await
            .expect("plant stale tmp");
        assert!(stale_tmp.exists(), "stale tmp planted");

        let other = backend.snapshot_path(aggregate_id(2)).with_extension("tmp");
        tokio::fs::write(&other, b"other stale")
            .await
            .expect("plant other stale");

        backend
            .persist(id, &CounterView { total: 9 }, seq(9))
            .await
            .expect("persist after sweep");

        assert!(
            !stale_tmp.exists(),
            "stale tmp removed by aggregate-scoped sweep"
        );
        assert!(
            other.exists(),
            "aggregate-scoped sweep (CHE-0048:R7) must leave other aggregates' tmps untouched"
        );
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

        backend
            .persist(id, &CounterView { total: 1 }, seq(1))
            .await
            .expect("initial persist");

        let live_companion = dir.path().join("foo.msgpack");
        let live_tmp = dir.path().join("foo.tmp");
        tokio::fs::write(&live_companion, b"companion lives")
            .await
            .expect("plant companion");
        tokio::fs::write(&live_tmp, b"tmp with live companion")
            .await
            .expect("plant live tmp");

        backend
            .persist(id, &CounterView { total: 2 }, seq(2))
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
            .persist(id, &CounterView { total: 999 }, seq(999))
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
            seq(2)
        );
    }

    /// findings F7 / CHE-0048 sub-problem 2 — `project_to_file` on a
    /// never-created aggregate (empty event stream) must produce neither
    /// a snapshot file nor a checkpoint file. The pre-fix code path
    /// called `backend.persist(_, &P::default(), 0)` unconditionally,
    /// writing a phantom `P::default()` snapshot + a `last_sequence=0`
    /// checkpoint for an aggregate the write side had never created.
    /// Post-fix (sub-task 3.1's type cascade) `replay_inner` returns
    /// `None` for an empty stream and `project_to_file_inner` skips the
    /// persist call. The returned projection is still `P::default()`
    /// (correct value semantics) but no disk evidence is fabricated.
    #[tokio::test]
    async fn empty_stream_aggregate_writes_no_snapshot_or_checkpoint_files() {
        let id = aggregate_id(7);
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");
        let store = StaticStore::new(vec![]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);

        let projection = driver
            .project_to_file(id, &CorrelationContext::none(), &backend)
            .await
            .expect("empty-stream projection succeeds (caller sees P::default())");

        assert_eq!(
            projection,
            CounterView::default(),
            "caller still receives the default projection value"
        );
        assert!(
            !backend.snapshot_path(id).exists(),
            "phantom snapshot must not be written for never-created aggregate"
        );
        assert!(
            !backend.checkpoint_path(id).exists(),
            "phantom checkpoint must not be written for never-created aggregate"
        );

        let entries = std::fs::read_dir(dir.path()).expect("read store dir");
        let payload_files: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("msgpack"))
            .collect();
        assert!(
            payload_files.is_empty(),
            "store directory must contain no .msgpack files; found {payload_files:?}"
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
                    seq(count)
                );
            });
        }

        #[test]
        fn safe_file_component_output_is_in_allowed_alphabet(input in "\\PC{0,32}") {
            let out = safe_file_component(&input);
            for ch in out.chars() {
                proptest::prop_assert!(
                    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-',
                    "unexpected char {ch:?} in {out:?} from {input:?}"
                );
            }
        }

        #[test]
        fn safe_file_component_is_idempotent(input in "\\PC{0,32}") {
            let once = safe_file_component(&input);
            let twice = safe_file_component(&once);
            proptest::prop_assert_eq!(once, twice);
        }

        #[test]
        fn persist_is_last_writer_wins_on_checkpoint(
            first in 1_u64..1_000_000,
            second in 1_u64..1_000_000,
        ) {
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(async move {
                let id = aggregate_id(1);
                let dir = tempfile::tempdir().expect("tempdir");
                let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

                backend
                    .persist(id, &CounterView { total: first }, seq(first))
                    .await
                    .expect("first persist");
                backend
                    .persist(id, &CounterView { total: second }, seq(second))
                    .await
                    .expect("second persist");

                let checkpoint = backend
                    .load_checkpoint(id)
                    .await
                    .expect("load checkpoint")
                    .expect("checkpoint exists");
                assert_eq!(checkpoint.last_sequence(), seq(second));
                assert_eq!(
                    backend.load_snapshot(id).await.expect("load snapshot"),
                    Some(CounterView { total: second })
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

        /// CHE-0048:R7 — two concurrent in-process `persist` calls
        /// targeting **distinct** aggregates on the same store
        /// directory must not race on each other's tmps. The pre-fix
        /// directory-wide `sweep_orphan_tmp` would reap any orphan
        /// tmp regardless of which aggregate it belonged to;
        /// aggregate A's persist could therefore wipe aggregate B's
        /// in-flight tmp out from under aggregate B's atomic rename.
        /// Post-fix the sweep is scoped to the target aggregate's
        /// filename prefix and the per-aggregate lock serialises
        /// same-aggregate writes — distinct-aggregate writes
        /// proceed in parallel without interference.
        ///
        /// Deterministic shape: rather than racing real tasks (flaky),
        /// each iteration plants an orphan tmp for aggregate B,
        /// runs `persist(A)`, then asserts B's orphan survives and
        /// A's snapshot+checkpoint exist. Then runs `persist(B)` and
        /// asserts B reaps its **own** orphan.
        #[test]
        fn che_0048_r7_aggregate_scoped_sweep_does_not_reap_other_aggregates_tmps(
            a in 1_u64..1_000,
            b in 1_u64..1_000,
        ) {
            proptest::prop_assume!(a != b);
            let rt = tokio::runtime::Runtime::new().expect("runtime");
            rt.block_on(async move {
                let id_a = aggregate_id(a);
                let id_b = aggregate_id(b);
                let dir = tempfile::tempdir().expect("tempdir");
                let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

                tokio::fs::create_dir_all(dir.path())
                    .await
                    .expect("ensure dir");
                let b_orphan = backend.snapshot_path(id_b).with_extension("tmp");
                tokio::fs::write(&b_orphan, b"b in-flight tmp")
                    .await
                    .expect("plant b orphan");

                backend
                    .persist(id_a, &CounterView { total: 1 }, seq(1))
                    .await
                    .expect("persist a");

                assert!(
                    b_orphan.exists(),
                    "aggregate B's tmp must survive aggregate A's sweep \
                     (aggregate-scoped sweep per CHE-0048:R7)"
                );
                assert!(backend.snapshot_path(id_a).exists(), "a snapshot written");
                assert!(backend.checkpoint_path(id_a).exists(), "a checkpoint written");

                backend
                    .persist(id_b, &CounterView { total: 2 }, seq(2))
                    .await
                    .expect("persist b");

                assert!(
                    !b_orphan.exists(),
                    "aggregate B's own next persist reaps its own orphan"
                );
                assert!(backend.snapshot_path(id_b).exists(), "b snapshot written");
                assert!(backend.checkpoint_path(id_b).exists(), "b checkpoint written");
            });
        }

        /// CHE-0048:R7 — concurrent `persist` for distinct aggregates
        /// on the same store directory completes without interleaving
        /// errors. Spawns two tasks, joins, asserts both observations
        /// (snapshot + checkpoint files) hold.
        #[test]
        fn che_0048_r7_concurrent_persists_distinct_aggregates_both_succeed(
            a in 1_u64..1_000,
            b in 1_u64..1_000,
        ) {
            proptest::prop_assume!(a != b);
            let rt = tokio::runtime::Runtime::new()
                .expect("runtime");
            rt.block_on(async move {
                let id_a = aggregate_id(a);
                let id_b = aggregate_id(b);
                let dir = tempfile::tempdir().expect("tempdir");
                let backend = FileProjectionStore::<CounterView>::new(dir.path(), "counter");

                let backend_a = backend.clone();
                let backend_b = backend.clone();
                let task_a = tokio::spawn(async move {
                    backend_a
                        .persist(id_a, &CounterView { total: 11 }, seq(11))
                        .await
                });
                let task_b = tokio::spawn(async move {
                    backend_b
                        .persist(id_b, &CounterView { total: 22 }, seq(22))
                        .await
                });
                let (ra, rb) = tokio::join!(task_a, task_b);
                ra.expect("task a join").expect("persist a");
                rb.expect("task b join").expect("persist b");

                assert_eq!(
                    backend.load_snapshot(id_a).await.expect("load a"),
                    Some(CounterView { total: 11 })
                );
                assert_eq!(
                    backend.load_snapshot(id_b).await.expect("load b"),
                    Some(CounterView { total: 22 })
                );
                assert_eq!(
                    backend
                        .load_checkpoint(id_a)
                        .await
                        .expect("load checkpoint a")
                        .expect("checkpoint a exists")
                        .last_sequence(),
                    seq(11)
                );
                assert_eq!(
                    backend
                        .load_checkpoint(id_b)
                        .await
                        .expect("load checkpoint b")
                        .expect("checkpoint b exists")
                        .last_sequence(),
                    seq(22)
                );
            });
        }
    }

    #[test]
    fn apply_one_delegates_to_projection_apply() {
        let id = aggregate_id(1);
        let store = StaticStore::new(vec![]);
        let driver = ProjectionDriver::<CounterView, _>::new(store);
        let mut view = CounterView::default();
        driver.apply_one(&mut view, &envelope(id, 1));
        driver.apply_one(&mut view, &envelope(id, 2));
        assert_eq!(view.total, 2);
    }

    #[test]
    fn tuple_arity_0() {
        assert_eq!(<() as ProjectionDriverTuple>::ARITY, 0);
    }

    #[test]
    fn tuple_arity_1() {
        type T = (ProjectionDriver<CounterView, StaticStore>,);
        assert_eq!(<T as ProjectionDriverTuple>::ARITY, 1);
    }

    #[test]
    fn tuple_arity_2() {
        type T = (
            ProjectionDriver<CounterView, StaticStore>,
            ProjectionDriver<CounterView, StaticStore>,
        );
        assert_eq!(<T as ProjectionDriverTuple>::ARITY, 2);
    }
}
