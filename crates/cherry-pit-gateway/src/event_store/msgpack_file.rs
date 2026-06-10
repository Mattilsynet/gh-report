use std::io;
use std::io::Write;
use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, ListableEventStore,
    StoreError,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// File-based event store using `MessagePack` serialization.
///
/// Stores each aggregate's event stream as a single `.msgpack` file
/// in the configured directory ([CHE-0036:R1]). Designed for development
/// and small deployments where a full database is unnecessary.
///
/// Parameterized by `E` — the single domain event type this store
/// persists. Each aggregate type gets its own `MsgpackFileStore<E>`
/// instance pointing at its own directory. The type parameter
/// guarantees at compile time that you cannot accidentally load or
/// persist the wrong event type.
///
/// # File layout
///
/// ```text
/// store/
/// ├── 1.msgpack
/// ├── 2.msgpack
/// └── ...
/// ```
///
/// Each file contains the complete event history for one aggregate,
/// serialized as `Vec<EventEnvelope<E>>` in `MessagePack` format.
/// On every append the full history is rewritten ([CHE-0036:R2]).
///
/// # Atomic writes ([CHE-0032])
///
/// All mutations write to a temporary file, call `fsync`, then rename
/// atomically to the target path ([CHE-0032:R1], [CHE-0032:R3]).
/// On rename failure the temp file is cleaned up best-effort
/// ([CHE-0032:R2]). Orphaned `.msgpack.tmp` files are removed on the
/// next store initialisation ([CHE-0032:R4], [CHE-0047:R1]).
///
/// # ID assignment ([CHE-0035:R1])
///
/// New aggregates get sequential `u64` IDs starting from 1 via
/// [`create`](EventStore::create). Allocation is serialized by an
/// atomic counter; the counter is lazily seeded by scanning the
/// directory for the highest existing numeric filename on the first
/// `create` call ([CHE-0035:R1]). Seeding runs exactly once per
/// store instance via an async `OnceCell`.
///
/// # Concurrency ([CHE-0035])
///
/// Per-aggregate write serialization via `scc::HashMap` keyed write
/// locks ([CHE-0035:R2]). Multiple aggregates can be written
/// concurrently. Reads are lock-free because writes are atomic via
/// temp-file + rename ([CHE-0035:R3]).
///
/// Optimistic concurrency (expected sequence check) provides
/// defense-in-depth within the owning process ([CHE-0006:R2]).
///
/// Not suitable for multi-process access — use a database-backed store
/// for that. File atomicity relies on POSIX `rename(2)` semantics.
///
/// # Process fencing ([CHE-0006:R1], [CHE-0043])
///
/// On first write, the store acquires an advisory `flock` on a `.lock`
/// sentinel file in the store directory ([CHE-0043:R1]). Lock
/// acquisition is lazy via `OnceCell` ([CHE-0043:R2]). If another
/// process already holds the lock, the store returns
/// `StoreError::StoreLocked` ([CHE-0043:R3]). This ensures each
/// aggregate instance is owned by exactly one OS process at a time
/// ([CHE-0006:R1]).
///
/// # Replay ([CHE-0024:R3])
///
/// Consumers replay from the event store via [`load`](EventStore::load).
/// Events are persisted before any publication attempt ([CHE-0024:R1] —
/// gateway owns the persist side; bus/publish layer is out of scope for
/// v0.1). No subscribe method exists on the `EventBus` port trait
/// ([CHE-0024:R2]).
///
/// # Operational recovery ([CHE-0047])
///
/// See [`RUNBOOKS.md`](../RUNBOOKS.md) for operator procedures covering
/// orphan temp-file recovery (R1), corrupt data classification (R2),
/// quarantine (R3), dead-letter schema (R4), stale-lock recovery (R5),
/// and migration recovery (R6).
///
/// [CHE-0006:R1]: ../../docs/adr/cherry/CHE-0006-single-writer-assumption.md
/// [CHE-0006:R2]: ../../docs/adr/cherry/CHE-0006-single-writer-assumption.md
/// [CHE-0024:R1]: ../../docs/adr/cherry/CHE-0024-event-delivery-model.md
/// [CHE-0024:R2]: ../../docs/adr/cherry/CHE-0024-event-delivery-model.md
/// [CHE-0024:R3]: ../../docs/adr/cherry/CHE-0024-event-delivery-model.md
/// [CHE-0032]: ../../docs/adr/cherry/CHE-0032-atomic-file-writes.md
/// [CHE-0032:R1]: ../../docs/adr/cherry/CHE-0032-atomic-file-writes.md
/// [CHE-0032:R2]: ../../docs/adr/cherry/CHE-0032-atomic-file-writes.md
/// [CHE-0032:R3]: ../../docs/adr/cherry/CHE-0032-atomic-file-writes.md
/// [CHE-0032:R4]: ../../docs/adr/cherry/CHE-0032-atomic-file-writes.md
/// [CHE-0035]: ../../docs/adr/cherry/CHE-0035-two-level-concurrency.md
/// [CHE-0035:R1]: ../../docs/adr/cherry/CHE-0035-two-level-concurrency.md
/// [CHE-0035:R2]: ../../docs/adr/cherry/CHE-0035-two-level-concurrency.md
/// [CHE-0035:R3]: ../../docs/adr/cherry/CHE-0035-two-level-concurrency.md
/// [CHE-0036:R1]: ../../docs/adr/cherry/CHE-0036-file-per-stream-full-rewrite-storage.md
/// [CHE-0036:R2]: ../../docs/adr/cherry/CHE-0036-file-per-stream-full-rewrite-storage.md
/// [CHE-0038:R5]: ../../docs/adr/cherry/CHE-0038-testing-strategy.md
/// [CHE-0043]: ../../docs/adr/cherry/CHE-0043-process-level-file-fencing.md
/// [CHE-0043:R1]: ../../docs/adr/cherry/CHE-0043-process-level-file-fencing.md
/// [CHE-0043:R2]: ../../docs/adr/cherry/CHE-0043-process-level-file-fencing.md
/// [CHE-0043:R3]: ../../docs/adr/cherry/CHE-0043-process-level-file-fencing.md
/// [CHE-0047]: ../../docs/adr/cherry/CHE-0047-operational-recovery-runbooks.md
/// [CHE-0047:R1]: ../../docs/adr/cherry/CHE-0047-operational-recovery-runbooks.md
///
/// # Example
///
/// ```rust
/// use cherry_pit_gateway::MsgpackFileStore;
/// use cherry_pit_core::DomainEvent;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// enum OrderEvent {
///     Created { name: String },
/// }
///
/// impl DomainEvent for OrderEvent {
///     fn event_type(&self) -> &'static str {
///         match self {
///             Self::Created { .. } => "order.created",
///         }
///     }
/// }
///
/// // Create a store pointing at a temporary directory (CHE-0038:R5).
/// let dir = tempfile::tempdir().unwrap();
/// let store = MsgpackFileStore::<OrderEvent>::new(dir.path());
///
/// // The default store uses `store/` as the directory.
/// let default_store = MsgpackFileStore::<OrderEvent>::default();
/// ```
pub struct MsgpackFileStore<E: DomainEvent> {
    dir: PathBuf,
    /// Lazy counter seeded from a one-shot directory scan. The
    /// `OnceCell` serializes the seeding step (so `scan_max_id` runs
    /// at most once per store instance), and the inner `AtomicU64`
    /// serializes per-call allocation without holding any guard
    /// across `.await`.
    next_id: tokio::sync::OnceCell<AtomicU64>,
    /// Per-aggregate write locks. `scc::HashMap` is lock-free for
    /// concurrent reads and uses fine-grained locking for writes —
    /// no poison risk, no contention on the map itself.
    locks: scc::HashMap<u64, Arc<tokio::sync::Mutex<()>>>,
    /// Advisory file lock on `{dir}/.lock`. Acquired lazily on first
    /// write operation, held for the store's lifetime. Detects
    /// accidental multi-process access to the same store directory.
    /// The `std::fs::File` handle keeps the flock alive — releasing
    /// happens automatically on drop.
    dir_lock: tokio::sync::OnceCell<std::fs::File>,
    /// Best-effort recovery of orphaned temp files. Runs once after
    /// process-level fencing succeeds, before the first mutating write.
    temp_recovery: tokio::sync::OnceCell<()>,
    _phantom: PhantomData<E>,
}

fn infrastructure_error(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::Infrastructure(error.into())
}

fn corrupt_data(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::CorruptData(error.into())
}

impl<E: DomainEvent + Serialize + DeserializeOwned> MsgpackFileStore<E> {
    /// Create a new store writing to the given directory.
    ///
    /// The directory is created lazily on first write.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            next_id: tokio::sync::OnceCell::new(),
            locks: scc::HashMap::new(),
            dir_lock: tokio::sync::OnceCell::new(),
            temp_recovery: tokio::sync::OnceCell::new(),
            _phantom: PhantomData,
        }
    }

    /// Return the file path for an aggregate.
    ///
    /// Infallible — `u64` IDs cannot cause path traversal.
    fn aggregate_path(&self, id: AggregateId) -> PathBuf {
        self.dir.join(format!("{}.msgpack", id.get()))
    }

    fn get_lock(&self, id: u64) -> Arc<tokio::sync::Mutex<()>> {
        if let Some(lock) = self.locks.read_sync(&id, |_, v| Arc::clone(v)) {
            return lock;
        }
        self.locks
            .entry_sync(id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .get()
            .clone()
    }

    /// Acquire an advisory file lock on the store directory.
    ///
    /// Called lazily on the first write operation (`create` or
    /// `append`). Uses `flock(2)` via `std::fs::File::try_lock` —
    /// the lock is held for the `MsgpackFileStore` lifetime (the
    /// `std::fs::File` handle lives in the `OnceCell`). Released
    /// automatically on drop.
    ///
    /// # Errors
    ///
    /// Returns `StoreError::StoreLocked` if another process already
    /// holds an exclusive lock on the same directory's `.lock` file.
    async fn ensure_fenced(&self) -> Result<(), StoreError> {
        self.dir_lock
            .get_or_try_init(|| async {
                let dir = self.dir.clone();
                tokio::task::spawn_blocking(move || {
                    std::fs::create_dir_all(&dir).map_err(infrastructure_error)?;

                    let lock_path = dir.join(".lock");
                    let file = std::fs::File::create(&lock_path).map_err(infrastructure_error)?;

                    file.try_lock().map_err(|e| match e {
                        std::fs::TryLockError::WouldBlock => StoreError::StoreLocked { path: dir },
                        std::fs::TryLockError::Error(io_err) => infrastructure_error(io_err),
                    })?;

                    Ok(file)
                })
                .await
                .map_err(infrastructure_error)?
            })
            .await?;
        Ok(())
    }

    /// Scan the directory for the highest numeric filename to seed the
    /// auto-increment counter. Non-numeric filenames are silently
    /// skipped.
    async fn scan_max_id(&self) -> Result<u64, StoreError> {
        let mut max: u64 = 0;
        let mut entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(infrastructure_error(e)),
        };
        while let Some(entry) = entries.next_entry().await.map_err(infrastructure_error)? {
            if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str())
                && let Ok(id) = stem.parse::<u64>()
            {
                max = max.max(id);
            }
        }
        Ok(max)
    }

    /// Remove leftover temp files from interrupted atomic writes.
    ///
    /// A `.msgpack.tmp` file is never authoritative: it is written before
    /// the atomic rename and only exists after a crash or failed rename.
    /// Cleanup is best-effort, scoped to this store's temp-file suffix, and
    /// runs only once per store instance so it cannot race with active writes.
    async fn recover_temp_files(&self) -> Result<(), StoreError> {
        self.temp_recovery
            .get_or_try_init(|| async {
                let mut entries = match tokio::fs::read_dir(&self.dir).await {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(infrastructure_error(e)),
                };

                while let Some(entry) = entries.next_entry().await.map_err(infrastructure_error)? {
                    let file_type = entry.file_type().await.map_err(infrastructure_error)?;
                    if !file_type.is_file() {
                        continue;
                    }

                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.ends_with(".msgpack.tmp") {
                        tokio::fs::remove_file(entry.path())
                            .await
                            .map_err(infrastructure_error)?;
                    }
                }

                Ok(())
            })
            .await?;

        Ok(())
    }

    fn deserialize_and_validate_stream(
        id: AggregateId,
        bytes: &[u8],
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let envelopes: Vec<EventEnvelope<E>> =
            rmp_serde::from_slice(bytes).map_err(corrupt_data)?;
        EventEnvelope::validate_stream(id, &envelopes).map_err(corrupt_data)?;
        Ok(envelopes)
    }

    /// Build envelopes from raw domain events.
    ///
    /// Assigns `event_id` (UUID v7), `aggregate_id`, `sequence`
    /// (starting from `start_sequence`), a shared `timestamp`
    /// (single timestamp per batch — the batch is atomic), and
    /// `correlation_id`/`causation_id` from the provided context.
    fn build_envelopes(
        id: AggregateId,
        start_sequence: u64,
        events: Vec<E>,
        context: &CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let timestamp = jiff::Timestamp::now();
        let mut envelopes = Vec::with_capacity(events.len());
        for (i, payload) in events.into_iter().enumerate() {
            let i_u64 = u64::try_from(i).unwrap_or(u64::MAX);
            let sequence_raw = start_sequence
                .checked_add(i_u64)
                .and_then(|s| s.checked_add(1))
                .ok_or_else(|| {
                    infrastructure_error(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "sequence overflow",
                    ))
                })?;
            let sequence = NonZeroU64::new(sequence_raw).ok_or_else(|| {
                infrastructure_error(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "sequence must be non-zero",
                ))
            })?;
            let envelope = EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                sequence,
                timestamp,
                context.correlation_id(),
                context.causation_id(),
                payload,
            )
            .map_err(infrastructure_error)?;
            envelopes.push(envelope);
        }
        Ok(envelopes)
    }

    /// Serialize and atomically write envelopes to disk.
    async fn write_atomic(
        &self,
        path: &std::path::Path,
        envelopes: &[EventEnvelope<E>],
    ) -> Result<(), StoreError> {
        let bytes = rmp_serde::encode::to_vec_named(envelopes).map_err(infrastructure_error)?;

        tokio::fs::create_dir_all(&self.dir)
            .await
            .map_err(infrastructure_error)?;

        let tmp_name = format!(
            "{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp")
        );
        let tmp_path = self.dir.join(tmp_name);

        let tmp_path_for_write = tmp_path.clone();
        let bytes_for_write = bytes;
        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            let mut tmp_file =
                std::fs::File::create(&tmp_path_for_write).map_err(infrastructure_error)?;
            tmp_file
                .write_all(&bytes_for_write)
                .map_err(infrastructure_error)?;
            tmp_file.sync_all().map_err(infrastructure_error)?;
            Ok(())
        })
        .await
        .map_err(infrastructure_error)??;

        if let Err(e) = tokio::fs::rename(&tmp_path, path).await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(infrastructure_error(e));
        }

        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            let dir_file = std::fs::File::open(dir).map_err(infrastructure_error)?;
            dir_file.sync_all().map_err(infrastructure_error)?;
            Ok(())
        })
        .await
        .map_err(infrastructure_error)??;

        Ok(())
    }
}

impl<E: DomainEvent + Serialize + DeserializeOwned> Default for MsgpackFileStore<E> {
    /// Default store directory: `store/`
    fn default() -> Self {
        Self::new("store")
    }
}

impl<E: DomainEvent + Serialize + DeserializeOwned> EventStore for MsgpackFileStore<E> {
    type Event = E;

    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let path = self.aggregate_path(id);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Self::deserialize_and_validate_stream(id, &bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(infrastructure_error(e)),
        }
    }

    async fn create(
        &self,
        events: Vec<E>,
        context: CorrelationContext,
    ) -> Result<(AggregateId, Vec<EventEnvelope<E>>), StoreError> {
        self.ensure_fenced().await?;
        self.recover_temp_files().await?;

        if events.is_empty() {
            return Err(infrastructure_error(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot create aggregate with zero events",
            )));
        }

        let counter = self
            .next_id
            .get_or_try_init(|| async {
                let max = self.scan_max_id().await?;
                let start = max.checked_add(1).ok_or_else(|| {
                    infrastructure_error(io::Error::other("aggregate ID overflow"))
                })?;
                Ok::<_, StoreError>(AtomicU64::new(start))
            })
            .await?;
        let n = counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| v.checked_add(1))
            .map_err(|_| infrastructure_error(io::Error::other("aggregate ID overflow")))?;
        let nz = NonZeroU64::new(n).ok_or_else(|| {
            infrastructure_error(io::Error::other("aggregate ID must be non-zero"))
        })?;
        let id = AggregateId::new(nz);

        let envelopes = Self::build_envelopes(id, 0, events, &context)?;
        let path = self.aggregate_path(id);
        self.write_atomic(&path, &envelopes).await?;

        Ok((id, envelopes))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<E>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        self.ensure_fenced().await?;
        self.recover_temp_files().await?;

        let lock = self.get_lock(id.get());
        let _guard = lock.lock().await;

        let path = self.aggregate_path(id);

        let mut existing: Vec<EventEnvelope<E>> = match tokio::fs::read(&path).await {
            Ok(bytes) => Self::deserialize_and_validate_stream(id, &bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(infrastructure_error(format!(
                    "cannot append to aggregate {id}: not created (use create() first)"
                )));
            }
            Err(e) => return Err(infrastructure_error(e)),
        };

        let actual_sequence = existing.last().map_or(0, |e| e.sequence().get());
        if actual_sequence != expected_sequence.get() {
            return Err(StoreError::ConcurrencyConflict {
                aggregate_id: id,
                expected_sequence,
                actual_sequence,
            });
        }

        let new_envelopes = Self::build_envelopes(id, expected_sequence.get(), events, &context)?;

        existing.extend(new_envelopes.iter().cloned());
        self.write_atomic(&path, &existing).await?;

        Ok(new_envelopes)
    }
}

impl<E: DomainEvent + Serialize + DeserializeOwned> ListableEventStore for MsgpackFileStore<E> {
    /// Enumerate aggregate IDs by scanning the store directory for
    /// `*.msgpack` files (skipping `.msgpack.tmp` orphans and the
    /// `.lock` sentinel) and parsing each stem as a `u64`.
    ///
    /// Non-existent directory is treated as an empty store
    /// (`Ok(vec![])`), matching the `scan_max_id` `NotFound` branch
    /// and [CHE-0070]: a never-written store enumerates as empty,
    /// not as an error.
    ///
    /// Per CHE-0070:R6 the blocking `std::fs::read_dir` scan runs on
    /// `tokio::task::spawn_blocking` so the reactor stays free for
    /// other tasks; a `tokio::task::JoinError` (runtime shutdown or
    /// blocking-body panic) maps to [`StoreError::JoinFailure`].
    async fn list_aggregates(&self) -> Result<Vec<AggregateId>, StoreError> {
        let dir = self.dir.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<AggregateId>, StoreError> {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => return Err(infrastructure_error(e)),
            };

            let mut ids = Vec::new();
            for entry in entries {
                let entry = entry.map_err(infrastructure_error)?;
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("msgpack") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Ok(raw) = stem.parse::<u64>() else {
                    continue;
                };
                let Some(nz) = NonZeroU64::new(raw) else {
                    continue;
                };
                ids.push(AggregateId::new(nz));
            }
            Ok(ids)
        })
        .await
        .map_err(|e| StoreError::JoinFailure(Box::new(e)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::num::NonZeroU64;

    /// Shorthand — most tests don't need correlation.
    fn no_ctx() -> CorrelationContext {
        CorrelationContext::none()
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    enum TestEvent {
        Created { name: String },
        Updated { name: String },
    }

    impl DomainEvent for TestEvent {
        fn event_type(&self) -> &'static str {
            match self {
                Self::Created { .. } => "test.created",
                Self::Updated { .. } => "test.updated",
            }
        }
    }

    /// Helper to construct an `AggregateId` from a raw `u64` in tests.
    fn agg_id(n: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(n).unwrap())
    }

    /// Helper to construct a `NonZeroU64` from a raw `u64` in tests.
    fn nz(n: u64) -> NonZeroU64 {
        NonZeroU64::new(n).unwrap()
    }

    fn fixed_timestamp() -> jiff::Timestamp {
        jiff::Timestamp::from_second(1_700_000_000).unwrap()
    }

    async fn has_tmp_file(dir: &std::path::Path) -> bool {
        let mut entries = tokio::fs::read_dir(dir).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry
                .file_name()
                .to_string_lossy()
                .ends_with(".msgpack.tmp")
            {
                return true;
            }
        }
        false
    }

    #[tokio::test]
    async fn create_assigns_sequential_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id1, _) = store
            .create(vec![TestEvent::Created { name: "a".into() }], no_ctx())
            .await
            .unwrap();
        let (id2, _) = store
            .create(vec![TestEvent::Created { name: "b".into() }], no_ctx())
            .await
            .unwrap();
        let (id3, _) = store
            .create(vec![TestEvent::Created { name: "c".into() }], no_ctx())
            .await
            .unwrap();

        assert_eq!(id1, agg_id(1));
        assert_eq!(id2, agg_id(2));
        assert_eq!(id3, agg_id(3));
    }

    #[tokio::test]
    async fn create_returns_correct_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let events = vec![
            TestEvent::Created { name: "a".into() },
            TestEvent::Updated { name: "b".into() },
        ];
        let (id, envelopes) = store.create(events, no_ctx()).await.unwrap();

        assert_eq!(envelopes.len(), 2);
        assert_eq!(envelopes[0].aggregate_id(), id);
        assert_eq!(envelopes[1].aggregate_id(), id);
        assert_eq!(envelopes[0].sequence().get(), 1);
        assert_eq!(envelopes[1].sequence().get(), 2);
        assert_eq!(
            *envelopes[0].payload(),
            TestEvent::Created { name: "a".into() }
        );
        assert_eq!(
            *envelopes[1].payload(),
            TestEvent::Updated { name: "b".into() }
        );
        assert!(!envelopes[0].event_id().is_nil());
        assert_ne!(envelopes[0].event_id(), envelopes[1].event_id());
        assert_eq!(envelopes[0].timestamp(), envelopes[1].timestamp());
    }

    #[tokio::test]
    async fn create_rejects_empty_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        let result = store.create(vec![], no_ctx()).await;
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), StoreError::Infrastructure(_)),
            "expected Infrastructure error for empty events"
        );
    }

    #[tokio::test]
    async fn create_survives_restart() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = MsgpackFileStore::new(dir.path());
            store
                .create(vec![TestEvent::Created { name: "a".into() }], no_ctx())
                .await
                .unwrap();
            store
                .create(vec![TestEvent::Created { name: "b".into() }], no_ctx())
                .await
                .unwrap();
        }

        let store = MsgpackFileStore::new(dir.path());
        let (id, _) = store
            .create(vec![TestEvent::Created { name: "c".into() }], no_ctx())
            .await
            .unwrap();
        assert_eq!(id, agg_id(3));
    }

    #[tokio::test]
    async fn directory_scan_ignores_non_numeric() {
        let dir = tempfile::tempdir().unwrap();

        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        tokio::fs::write(dir.path().join("old-format.msgpack"), b"junk")
            .await
            .unwrap();
        {
            let store = MsgpackFileStore::new(dir.path());
            store
                .create(vec![TestEvent::Created { name: "a".into() }], no_ctx())
                .await
                .unwrap();
        }

        let store = MsgpackFileStore::new(dir.path());
        let (id, _) = store
            .create(vec![TestEvent::Created { name: "b".into() }], no_ctx())
            .await
            .unwrap();
        assert_eq!(id, agg_id(2));
    }

    #[tokio::test]
    async fn load_nonexistent_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let events: Vec<EventEnvelope<TestEvent>> = store.load(agg_id(999)).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn corrupt_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();

        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        tokio::fs::write(dir.path().join("1.msgpack"), b"not valid msgpack")
            .await
            .unwrap();

        let store = MsgpackFileStore::new(dir.path());
        let result: Result<Vec<EventEnvelope<TestEvent>>, _> = store.load(agg_id(1)).await;
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), StoreError::CorruptData(_)),
            "expected CorruptData error for corrupt file"
        );
    }

    #[tokio::test]
    async fn load_rejects_sequence_gap() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();

        let id = agg_id(1);
        let envelopes = vec![
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                NonZeroU64::new(1).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Created { name: "a".into() },
            )
            .unwrap(),
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                id,
                NonZeroU64::new(3).unwrap(),
                jiff::Timestamp::now(),
                None,
                None,
                TestEvent::Updated { name: "b".into() },
            )
            .unwrap(),
        ];
        let bytes = rmp_serde::encode::to_vec_named(&envelopes).unwrap();
        tokio::fs::write(dir.path().join("1.msgpack"), &bytes)
            .await
            .unwrap();

        let store = MsgpackFileStore::<TestEvent>::new(dir.path());
        let result = store.load(id).await;

        assert!(
            matches!(result, Err(StoreError::CorruptData(_))),
            "expected CorruptData for sequence gap, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn load_rejects_aggregate_id_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();

        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            agg_id(2),
            NonZeroU64::new(1).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            TestEvent::Created {
                name: "wrong".into(),
            },
        )
        .unwrap();
        let bytes = rmp_serde::encode::to_vec_named(&vec![envelope]).unwrap();
        tokio::fs::write(dir.path().join("1.msgpack"), &bytes)
            .await
            .unwrap();

        let store = MsgpackFileStore::<TestEvent>::new(dir.path());
        let result = store.load(agg_id(1)).await;

        assert!(
            matches!(result, Err(StoreError::CorruptData(_))),
            "expected CorruptData for aggregate mismatch, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn create_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, created) = store
            .create(
                vec![TestEvent::Created {
                    name: "alice".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(*loaded[0].payload(), *created[0].payload());
        assert_eq!(loaded[0].sequence().get(), 1);
        assert_eq!(loaded[0].aggregate_id(), id);
    }

    #[tokio::test]
    async fn msgpack_stream_golden_file_roundtrip() {
        let id = agg_id(42);
        let envelopes = vec![
            EventEnvelope::new(
                uuid::Uuid::from_bytes([
                    0x01, 0x93, 0xa3, 0xe8, 0x80, 0x00, 0x7c, 0xde, 0x8f, 0x01, 0x23, 0x45, 0x67,
                    0x89, 0xab, 0xcd,
                ]),
                id,
                NonZeroU64::new(1).unwrap(),
                fixed_timestamp(),
                Some(uuid::Uuid::from_bytes([
                    0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x71, 0x22, 0x83, 0x44, 0x55, 0x66, 0x77,
                    0x88, 0x99, 0x00,
                ])),
                None,
                TestEvent::Created {
                    name: "golden-create".into(),
                },
            )
            .unwrap(),
            EventEnvelope::new(
                uuid::Uuid::from_bytes([
                    0x01, 0x93, 0xa3, 0xe8, 0x80, 0x01, 0x7c, 0xde, 0x8f, 0x01, 0x23, 0x45, 0x67,
                    0x89, 0xab, 0xce,
                ]),
                id,
                NonZeroU64::new(2).unwrap(),
                fixed_timestamp(),
                Some(uuid::Uuid::from_bytes([
                    0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x71, 0x22, 0x83, 0x44, 0x55, 0x66, 0x77,
                    0x88, 0x99, 0x00,
                ])),
                Some(uuid::Uuid::from_bytes([
                    0x01, 0x93, 0xa3, 0xe8, 0x80, 0x00, 0x7c, 0xde, 0x8f, 0x01, 0x23, 0x45, 0x67,
                    0x89, 0xab, 0xcd,
                ])),
                TestEvent::Updated {
                    name: "golden-update".into(),
                },
            )
            .unwrap(),
        ];

        let serialized = rmp_serde::encode::to_vec_named(&envelopes).unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/msgpack_stream_golden.msgpack");
        if !path.exists() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &serialized).unwrap();
            eprintln!(
                "Golden file written to {}. Commit this file.",
                path.display()
            );
        }

        let expected = std::fs::read(&path).unwrap();
        assert_eq!(serialized, expected, "MessagePack stream fixture changed");

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("42.msgpack"), &expected)
            .await
            .unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());
        let loaded = store.load(id).await.unwrap();

        assert_eq!(loaded.len(), 2);
        assert_eq!(*loaded[0].payload(), *envelopes[0].payload());
        assert_eq!(*loaded[1].payload(), *envelopes[1].payload());
        assert_eq!(loaded[0].sequence().get(), 1);
        assert_eq!(loaded[1].sequence().get(), 2);
    }

    #[tokio::test]
    async fn append_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "alice".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let appended = store
            .append(
                id,
                nz(1),
                vec![TestEvent::Updated { name: "bob".into() }],
                no_ctx(),
            )
            .await
            .unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].sequence().get(), 2);
        assert_eq!(appended[0].aggregate_id(), id);

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].sequence().get(), 1);
        assert_eq!(loaded[1].sequence().get(), 2);
    }

    #[tokio::test]
    async fn append_multiple_batches() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "alice".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        store
            .append(
                id,
                nz(1),
                vec![TestEvent::Updated { name: "bob".into() }],
                no_ctx(),
            )
            .await
            .unwrap();
        store
            .append(
                id,
                nz(2),
                vec![TestEvent::Updated {
                    name: "carol".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].sequence().get(), 1);
        assert_eq!(loaded[1].sequence().get(), 2);
        assert_eq!(loaded[2].sequence().get(), 3);
    }

    #[tokio::test]
    async fn append_empty_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "alice".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let result = store.append(id, nz(1), vec![], no_ctx()).await.unwrap();
        assert!(result.is_empty());

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[tokio::test]
    async fn append_returns_correct_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(vec![TestEvent::Created { name: "a".into() }], no_ctx())
            .await
            .unwrap();

        let envelopes = store
            .append(
                id,
                nz(1),
                vec![
                    TestEvent::Updated { name: "b".into() },
                    TestEvent::Updated { name: "c".into() },
                ],
                no_ctx(),
            )
            .await
            .unwrap();

        assert_eq!(envelopes.len(), 2);
        assert_eq!(envelopes[0].aggregate_id(), id);
        assert_eq!(envelopes[1].aggregate_id(), id);
        assert_eq!(envelopes[0].sequence().get(), 2);
        assert_eq!(envelopes[1].sequence().get(), 3);
        assert!(!envelopes[0].event_id().is_nil());
        assert_ne!(envelopes[0].event_id(), envelopes[1].event_id());

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(*loaded[1].payload(), *envelopes[0].payload());
        assert_eq!(*loaded[2].payload(), *envelopes[1].payload());
    }

    #[tokio::test]
    async fn concurrency_conflict_detected() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "alice".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        store
            .append(
                id,
                nz(1),
                vec![TestEvent::Updated { name: "bob".into() }],
                no_ctx(),
            )
            .await
            .unwrap();

        let result = store
            .append(
                id,
                nz(1),
                vec![TestEvent::Updated {
                    name: "carol".into(),
                }],
                no_ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), StoreError::ConcurrencyConflict { .. }),
            "expected ConcurrencyConflict"
        );
    }

    #[tokio::test]
    async fn concurrent_appends_to_same_aggregate() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::new(dir.path()));

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "seed".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let mut handles = Vec::new();
        for i in 0..5 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.append(
                    id,
                    nz(1),
                    vec![TestEvent::Updated {
                        name: format!("writer-{i}"),
                    }],
                    no_ctx(),
                )
                .await
            }));
        }

        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let successes = results.iter().filter(|r| r.is_ok()).count();
        let conflicts = results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::ConcurrencyConflict { .. })))
            .count();

        assert_eq!(successes, 1, "exactly one writer should succeed");
        assert_eq!(
            conflicts, 4,
            "remaining writers should get ConcurrencyConflict"
        );
    }

    #[tokio::test]
    async fn separate_aggregates_isolated() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id1, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "alice".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();
        let (id2, _) = store
            .create(vec![TestEvent::Created { name: "bob".into() }], no_ctx())
            .await
            .unwrap();

        let loaded1 = store.load(id1).await.unwrap();
        let loaded2 = store.load(id2).await.unwrap();

        assert_eq!(loaded1.len(), 1);
        assert_eq!(loaded2.len(), 1);
        assert_eq!(
            *loaded1[0].payload(),
            TestEvent::Created {
                name: "alice".into()
            }
        );
        assert_eq!(
            *loaded2[0].payload(),
            TestEvent::Created { name: "bob".into() }
        );
        assert_ne!(id1, id2);
    }

    #[test]
    fn default_uses_store_dir() {
        let store = MsgpackFileStore::<TestEvent>::default();
        assert_eq!(store.dir, PathBuf::from("store"));
    }

    #[tokio::test]
    async fn create_then_append_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, created) = store
            .create(
                vec![TestEvent::Created {
                    name: "order".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].sequence().get(), 1);

        let appended = store
            .append(
                id,
                nz(1),
                vec![TestEvent::Updated {
                    name: "shipped".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].sequence().get(), 2);

        let all = store.load(id).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].aggregate_id(), id);
        assert_eq!(all[1].aggregate_id(), id);
    }

    #[tokio::test]
    async fn send_to_nonexistent_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let events: Vec<EventEnvelope<TestEvent>> = store.load(agg_id(42)).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn concurrent_creates_assign_unique_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::new(dir.path()));

        let mut handles = Vec::new();
        for i in 0..10 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.create(
                    vec![TestEvent::Created {
                        name: format!("agg-{i}"),
                    }],
                    no_ctx(),
                )
                .await
            }));
        }

        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap().unwrap())
            .collect();

        let mut ids: Vec<u64> = results.iter().map(|(id, _)| id.get()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (1..=10).collect::<Vec<_>>(),
            "concurrent first creates must yield the contiguous range 1..=10 (no gaps, no duplicates)"
        );
    }

    #[tokio::test]
    async fn concurrent_first_creates_after_existing_files_seed_once() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        for id in [1u64, 2, 5, 17] {
            tokio::fs::write(dir.path().join(format!("{id}.msgpack")), b"placeholder")
                .await
                .unwrap();
        }

        let store = Arc::new(MsgpackFileStore::<TestEvent>::new(dir.path()));

        let mut handles = Vec::new();
        for i in 0..16 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.create(
                    vec![TestEvent::Created {
                        name: format!("agg-{i}"),
                    }],
                    no_ctx(),
                )
                .await
            }));
        }
        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap().unwrap())
            .collect();

        let mut ids: Vec<u64> = results.iter().map(|(id, _)| id.get()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (18u64..=33).collect::<Vec<_>>(),
            "scan must seed from max existing id (17) exactly once; 16 concurrent first creates must occupy 18..=33"
        );
    }

    #[tokio::test]
    async fn build_envelopes_sequence_overflow() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(vec![TestEvent::Created { name: "a".into() }], no_ctx())
            .await
            .unwrap();

        let result = store
            .append(
                id,
                nz(u64::MAX),
                vec![TestEvent::Updated { name: "b".into() }],
                no_ctx(),
            )
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deserializes_old_format_without_correlation_fields() {
        #[derive(Serialize)]
        struct OldEnvelope {
            event_id: uuid::Uuid,
            aggregate_id: AggregateId,
            sequence: u64,
            timestamp: jiff::Timestamp,
            payload: TestEvent,
        }

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();

        let old = vec![OldEnvelope {
            event_id: uuid::Uuid::now_v7(),
            aggregate_id: agg_id(1),
            sequence: 1,
            timestamp: jiff::Timestamp::now(),
            payload: TestEvent::Created { name: "old".into() },
        }];

        let bytes = rmp_serde::encode::to_vec_named(&old).unwrap();
        tokio::fs::write(dir.path().join("1.msgpack"), &bytes)
            .await
            .unwrap();

        let store = MsgpackFileStore::<TestEvent>::new(dir.path());
        let loaded = store.load(agg_id(1)).await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            *loaded[0].payload(),
            TestEvent::Created { name: "old".into() }
        );
        assert!(loaded[0].correlation_id().is_none());
        assert!(loaded[0].causation_id().is_none());
    }

    #[tokio::test]
    async fn correlation_and_causation_ids_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, created) = store
            .create(
                vec![TestEvent::Created {
                    name: "traced".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        assert!(created[0].correlation_id().is_none());
        assert!(created[0].causation_id().is_none());

        let loaded = store.load(id).await.unwrap();
        assert!(loaded[0].correlation_id().is_none());
        assert!(loaded[0].causation_id().is_none());
    }

    #[tokio::test]
    async fn correlation_and_causation_some_values_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();

        let corr = uuid::Uuid::now_v7();
        let cause = uuid::Uuid::now_v7();
        let envelopes = vec![
            EventEnvelope::new(
                uuid::Uuid::now_v7(),
                agg_id(1),
                NonZeroU64::new(1).unwrap(),
                jiff::Timestamp::now(),
                Some(corr),
                Some(cause),
                TestEvent::Created {
                    name: "with-ids".into(),
                },
            )
            .unwrap(),
        ];

        let bytes = rmp_serde::encode::to_vec_named(&envelopes).unwrap();
        tokio::fs::write(dir.path().join("1.msgpack"), &bytes)
            .await
            .unwrap();

        let store = MsgpackFileStore::<TestEvent>::new(dir.path());
        let loaded = store.load(agg_id(1)).await.unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].correlation_id(), Some(corr));
        assert_eq!(loaded[0].causation_id(), Some(cause));
    }

    #[tokio::test]
    async fn create_with_correlation_context_stamps_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let corr = uuid::Uuid::now_v7();
        let cause = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::new(corr, cause);

        let (id, created) = store
            .create(vec![TestEvent::Created { name: "ctx".into() }], ctx)
            .await
            .unwrap();

        assert_eq!(created[0].correlation_id(), Some(corr));
        assert_eq!(created[0].causation_id(), Some(cause));

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded[0].correlation_id(), Some(corr));
        assert_eq!(loaded[0].causation_id(), Some(cause));
    }

    #[tokio::test]
    async fn append_with_correlation_context_stamps_envelopes() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "seed".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let corr = uuid::Uuid::now_v7();
        let cause = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::new(corr, cause);

        let appended = store
            .append(
                id,
                nz(1),
                vec![TestEvent::Updated { name: "ctx".into() }],
                ctx,
            )
            .await
            .unwrap();

        assert_eq!(appended[0].correlation_id(), Some(corr));
        assert_eq!(appended[0].causation_id(), Some(cause));

        let loaded = store.load(id).await.unwrap();
        assert!(loaded[0].correlation_id().is_none());
        assert_eq!(loaded[1].correlation_id(), Some(corr));
    }

    #[tokio::test]
    async fn create_with_correlated_context_stamps_correlation_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let corr = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::correlated(corr);

        let (id, created) = store
            .create(
                vec![TestEvent::Created {
                    name: "corr-only".into(),
                }],
                ctx,
            )
            .await
            .unwrap();

        assert_eq!(created[0].correlation_id(), Some(corr));
        assert!(created[0].causation_id().is_none());

        let loaded = store.load(id).await.unwrap();
        assert_eq!(loaded[0].correlation_id(), Some(corr));
        assert!(loaded[0].causation_id().is_none());
    }

    #[tokio::test]
    async fn old_format_with_zero_sequence_rejected_on_load() {
        #[derive(serde::Serialize)]
        struct BadEnvelope {
            event_id: uuid::Uuid,
            aggregate_id: AggregateId,
            sequence: u64,
            timestamp: jiff::Timestamp,
            payload: TestEvent,
        }

        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();

        let bad = vec![BadEnvelope {
            event_id: uuid::Uuid::now_v7(),
            aggregate_id: agg_id(1),
            sequence: 0,
            timestamp: jiff::Timestamp::now(),
            payload: TestEvent::Created {
                name: "zero-seq".into(),
            },
        }];

        let bytes = rmp_serde::encode::to_vec_named(&bad).unwrap();
        tokio::fs::write(dir.path().join("1.msgpack"), &bytes)
            .await
            .unwrap();

        let store = MsgpackFileStore::<TestEvent>::new(dir.path());
        let result = store.load(agg_id(1)).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), StoreError::CorruptData(_)),
            "expected CorruptData error for zero sequence"
        );
    }

    #[tokio::test]
    async fn append_to_uncreated_aggregate_fails() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        let result = store
            .append(
                agg_id(999),
                nz(1),
                vec![TestEvent::Created {
                    name: "sneaky".into(),
                }],
                no_ctx(),
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, StoreError::Infrastructure(_)),
            "expected Infrastructure error, got: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("not created"),
            "error message should mention 'not created', got: {msg}"
        );
    }

    #[tokio::test]
    async fn create_does_not_overwrite_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();

        let sentinel = b"sentinel data";
        tokio::fs::write(dir.path().join("1.msgpack"), sentinel)
            .await
            .unwrap();

        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "safe".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        assert_eq!(id.get(), 2, "should skip the occupied ID");

        let data = tokio::fs::read(dir.path().join("1.msgpack")).await.unwrap();
        assert_eq!(data, sentinel, "existing file must not be overwritten");
    }

    #[tokio::test]
    async fn scan_max_id_with_gaps() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        for id_val in [1u64, 5] {
            let id = agg_id(id_val);
            let envelopes = vec![
                EventEnvelope::new(
                    uuid::Uuid::now_v7(),
                    id,
                    NonZeroU64::new(1).unwrap(),
                    jiff::Timestamp::now(),
                    None,
                    None,
                    TestEvent::Created {
                        name: format!("agg-{id_val}"),
                    },
                )
                .unwrap(),
            ];
            let bytes = rmp_serde::encode::to_vec_named(&envelopes).unwrap();
            tokio::fs::write(dir.path().join(format!("{id_val}.msgpack")), &bytes)
                .await
                .unwrap();
        }

        let store2 = MsgpackFileStore::<TestEvent>::new(dir.path());

        let (id, _) = store2
            .create(
                vec![TestEvent::Created {
                    name: "after-gap".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        assert_eq!(id.get(), 6, "next ID should be max(1,5)+1 = 6");
        drop(store);
    }

    #[tokio::test]
    async fn concurrent_create_and_append() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::new(dir.path()));

        let (seed_id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "seed".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let mut handles = Vec::new();

        for i in 0..5 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.create(
                    vec![TestEvent::Created {
                        name: format!("new-{i}"),
                    }],
                    no_ctx(),
                )
                .await
                .map(|r| ("create", r.0))
            }));
        }

        for i in 0..5 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.append(
                    seed_id,
                    nz(1),
                    vec![TestEvent::Updated {
                        name: format!("upd-{i}"),
                    }],
                    no_ctx(),
                )
                .await
                .map(|_| ("append", seed_id))
            }));
        }

        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        let creates: Vec<_> = results
            .iter()
            .filter(|r| r.as_ref().ok().is_some_and(|v| v.0 == "create"))
            .collect();
        assert_eq!(creates.len(), 5, "all creates should succeed");

        let append_ok = results
            .iter()
            .filter(|r| r.as_ref().ok().is_some_and(|v| v.0 == "append"))
            .count();
        let append_err = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(append_ok, 1, "exactly one append should win");
        assert_eq!(append_err, 4, "four appends should get ConcurrencyConflict");

        let mut created_ids: Vec<u64> = creates
            .iter()
            .map(|r| r.as_ref().unwrap().1.get())
            .collect();
        created_ids.sort_unstable();
        created_ids.dedup();
        assert_eq!(created_ids.len(), 5, "all created IDs must be unique");
    }

    #[tokio::test]
    async fn temp_file_cleaned_up_after_successful_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "clean".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            assert!(
                !name_str.ends_with(".tmp"),
                "temp file should be cleaned up: {name_str}"
            );
        }

        let path = dir.path().join(format!("{}.msgpack", id.get()));
        assert!(path.exists(), "aggregate file should exist");
    }

    #[tokio::test]
    async fn orphaned_temp_file_removed_on_next_write() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::create_dir_all(dir.path()).await.unwrap();
        let orphan = dir.path().join("1.msgpack.tmp");
        tokio::fs::write(&orphan, b"interrupted write")
            .await
            .unwrap();

        let store = MsgpackFileStore::new(dir.path());
        store
            .create(
                vec![TestEvent::Created {
                    name: "recover".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        assert!(
            !has_tmp_file(dir.path()).await,
            "orphaned temp file should be removed"
        );
    }

    #[tokio::test]
    async fn single_store_acquires_lock_successfully() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let result = store
            .create(
                vec![TestEvent::Created {
                    name: "fenced".into(),
                }],
                no_ctx(),
            )
            .await;
        assert!(result.is_ok(), "first store should acquire lock");

        assert!(
            dir.path().join(".lock").exists(),
            ".lock sentinel file should be created"
        );
    }

    #[tokio::test]
    async fn second_store_same_dir_fails_with_store_locked() {
        let dir = tempfile::tempdir().unwrap();

        let store1 = MsgpackFileStore::new(dir.path());
        store1
            .create(
                vec![TestEvent::Created {
                    name: "first".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let store2 = MsgpackFileStore::<TestEvent>::new(dir.path());
        let result = store2
            .create(
                vec![TestEvent::Created {
                    name: "second".into(),
                }],
                no_ctx(),
            )
            .await;

        assert!(
            matches!(result, Err(StoreError::StoreLocked { .. })),
            "second store should get StoreLocked, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn lock_released_on_drop_allows_reacquisition() {
        let dir = tempfile::tempdir().unwrap();

        {
            let store = MsgpackFileStore::new(dir.path());
            store
                .create(
                    vec![TestEvent::Created {
                        name: "first".into(),
                    }],
                    no_ctx(),
                )
                .await
                .unwrap();
        }

        let store2 = MsgpackFileStore::<TestEvent>::new(dir.path());
        let result = store2
            .append(
                agg_id(1),
                nz(1),
                vec![TestEvent::Updated {
                    name: "after-drop".into(),
                }],
                no_ctx(),
            )
            .await;
        assert!(result.is_ok(), "should reacquire lock after drop");
    }

    #[tokio::test]
    async fn concurrent_ensure_fenced_does_not_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::new(dir.path()));

        let mut handles = Vec::new();
        for i in 0..5 {
            let s = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                s.create(
                    vec![TestEvent::Created {
                        name: format!("concurrent-{i}"),
                    }],
                    no_ctx(),
                )
                .await
            }));
        }

        let results: Vec<_> = futures_util::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert!(
            results.iter().all(std::result::Result::is_ok),
            "all concurrent creates should succeed within same store"
        );
    }

    #[tokio::test]
    async fn concurrent_read_during_write_is_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MsgpackFileStore::new(dir.path()));

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "seed".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let writer_store = Arc::clone(&store);
        let writer = tokio::spawn(async move {
            for seq in 1..=10u64 {
                let _ = writer_store
                    .append(
                        id,
                        nz(seq),
                        vec![TestEvent::Updated {
                            name: format!("w-{seq}"),
                        }],
                        no_ctx(),
                    )
                    .await;
            }
        });

        let mut readers = Vec::new();
        for _ in 0..20 {
            let s = Arc::clone(&store);
            readers.push(tokio::spawn(async move {
                let events = s.load(id).await.unwrap();
                assert!(!events.is_empty(), "load must return at least the seed");
                for (i, env) in events.iter().enumerate() {
                    assert_eq!(
                        env.sequence().get(),
                        (i + 1) as u64,
                        "sequence gap or reorder detected at position {i}"
                    );
                    assert_eq!(env.aggregate_id(), id, "wrong aggregate in stream");
                }
            }));
        }

        writer.await.unwrap();
        for r in readers {
            r.await.unwrap();
        }
    }

    #[tokio::test]
    async fn one_file_per_aggregate_filesystem_shape() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "shape".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        for seq in 1..=5u64 {
            store
                .append(
                    id,
                    nz(seq),
                    vec![TestEvent::Updated {
                        name: format!("ev-{seq}"),
                    }],
                    no_ctx(),
                )
                .await
                .unwrap();
        }

        let mut msgpack_count = 0u32;
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".msgpack") && !name_str.ends_with(".tmp") {
                msgpack_count += 1;
            }
        }
        assert_eq!(msgpack_count, 1, "exactly one .msgpack file per aggregate");

        let expected = dir.path().join(format!("{}.msgpack", id.get()));
        assert!(expected.exists(), "file must be named <id>.msgpack");
    }

    #[tokio::test]
    async fn append_rewrites_full_history_monotonic_growth() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::new(dir.path());

        let (id, _) = store
            .create(
                vec![TestEvent::Created {
                    name: "grow".into(),
                }],
                no_ctx(),
            )
            .await
            .unwrap();

        let path = dir.path().join(format!("{}.msgpack", id.get()));
        let mut prev_len = tokio::fs::metadata(&path).await.unwrap().len();

        for seq in 1..=5u64 {
            store
                .append(
                    id,
                    nz(seq),
                    vec![TestEvent::Updated {
                        name: format!("growth-{seq}"),
                    }],
                    no_ctx(),
                )
                .await
                .unwrap();

            let new_len = tokio::fs::metadata(&path).await.unwrap().len();
            assert!(
                new_len > prev_len,
                "file must grow after append (seq {seq}): {prev_len} -> {new_len}"
            );
            prev_len = new_len;
        }
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn build_envelopes_sequences_are_monotonic(
                count in 1..20usize,
                start in 0..u64::MAX - 20,
            ) {
                let id = agg_id(1);
                let events: Vec<TestEvent> = (0..count)
                    .map(|i| TestEvent::Created { name: format!("e{i}") })
                    .collect();
                let ctx = no_ctx();

                let envelopes = MsgpackFileStore::build_envelopes(id, start, events, &ctx)
                    .unwrap();

                prop_assert_eq!(envelopes.len(), count);

                for window in envelopes.windows(2) {
                    prop_assert!(
                        window[1].sequence() > window[0].sequence(),
                        "sequence not monotonically increasing: {} <= {}",
                        window[1].sequence(),
                        window[0].sequence()
                    );
                }

                prop_assert_eq!(envelopes[0].sequence().get(), start + 1);
                prop_assert_eq!(
                    envelopes.last().unwrap().sequence().get(),
                    start + count as u64
                );
            }
        }

        /// Replay determinism (CHE-0038:R3, CHE-0024:R3): create, append,
        /// then reload from a fresh store handle — the loaded stream must
        /// match what was written, with monotonic sequences.
        #[test]
        fn replay_determinism_create_append_reload() {
            use proptest::test_runner::{Config, TestRunner};

            let mut runner = TestRunner::new(Config {
                cases: 64,
                ..Config::default()
            });

            runner
                .run(
                    &(
                        proptest::collection::vec("[a-z]{1,8}", 1..6),
                        proptest::collection::vec("[a-z]{1,8}", 1..6),
                    ),
                    |(create_names, append_names)| {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .unwrap();
                        rt.block_on(async {
                            let dir = tempfile::tempdir().unwrap();

                            let (id, total_written) = {
                                let store = MsgpackFileStore::new(dir.path());
                                let create_events: Vec<TestEvent> = create_names
                                    .iter()
                                    .map(|n| TestEvent::Created { name: n.clone() })
                                    .collect();
                                let (id, mut written) =
                                    store.create(create_events, no_ctx()).await.unwrap();

                                let append_events: Vec<TestEvent> = append_names
                                    .iter()
                                    .map(|n| TestEvent::Updated { name: n.clone() })
                                    .collect();
                                let expected_seq = nz(written.len() as u64);
                                let appended = store
                                    .append(id, expected_seq, append_events, no_ctx())
                                    .await
                                    .unwrap();
                                written.extend(appended);
                                (id, written)
                            };

                            let store2 = MsgpackFileStore::<TestEvent>::new(dir.path());
                            let loaded = store2.load(id).await.unwrap();

                            assert_eq!(loaded.len(), total_written.len());
                            for (w, l) in total_written.iter().zip(loaded.iter()) {
                                assert_eq!(w.sequence(), l.sequence());
                                assert_eq!(w.payload(), l.payload());
                                assert_eq!(w.aggregate_id(), l.aggregate_id());
                            }

                            for window in loaded.windows(2) {
                                assert!(
                                    window[1].sequence() > window[0].sequence(),
                                    "replay not monotonic"
                                );
                            }
                        });
                        Ok(())
                    },
                )
                .unwrap();
        }
    }

    /// CHE-0032:R2 — when the atomic rename in `write_atomic` fails,
    /// the `.msgpack.tmp` artefact MUST be removed so subsequent
    /// writes do not see an orphan temp file.
    ///
    /// Failure is forced by pre-creating the target aggregate path as
    /// a *non-empty* directory. POSIX `rename(2)` refuses to replace a
    /// non-empty directory with a regular file (ENOTEMPTY / EEXIST /
    /// EISDIR depending on platform), so this is portable across
    /// Linux and macOS.
    ///
    /// Id assignment: the first `create` lands `1.msgpack` and caches
    /// `next_id = 2`. The blocker directory is placed at `2.msgpack`
    /// so the second `create` collides on the rename step specifically
    /// (not on temp-file creation).
    #[tokio::test]
    async fn rename_failure_cleans_up_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        store
            .create(
                vec![TestEvent::Created {
                    name: "first".into(),
                }],
                no_ctx(),
            )
            .await
            .expect("first create succeeds on fresh tempdir");

        let blocker = dir.path().join("2.msgpack");
        tokio::fs::create_dir(&blocker).await.unwrap();
        tokio::fs::write(blocker.join("child"), b"x").await.unwrap();

        let result = store
            .create(
                vec![TestEvent::Created {
                    name: "second".into(),
                }],
                no_ctx(),
            )
            .await;

        assert!(
            matches!(result.as_ref().unwrap_err(), StoreError::Infrastructure(_)),
            "rename failure must surface as StoreError::Infrastructure, got: {result:?}"
        );

        assert!(
            !has_tmp_file(dir.path()).await,
            "CHE-0032:R2: .msgpack.tmp must be cleaned up after rename failure"
        );
    }

    #[tokio::test]
    async fn list_aggregates_empty_store_returns_empty_vec() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path().join("never-created"));

        let ids = store
            .list_aggregates()
            .await
            .expect("list on missing dir is Ok");
        assert!(ids.is_empty(), "missing dir enumerates as empty");

        let store2 = MsgpackFileStore::<TestEvent>::new(dir.path());
        let ids2 = store2
            .list_aggregates()
            .await
            .expect("list on empty dir is Ok");
        assert!(ids2.is_empty(), "empty dir enumerates as empty");
    }

    #[tokio::test]
    async fn list_aggregates_returns_all_created_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        let (id1, _) = store
            .create(vec![TestEvent::Created { name: "a".into() }], no_ctx())
            .await
            .unwrap();
        let (id2, _) = store
            .create(vec![TestEvent::Created { name: "b".into() }], no_ctx())
            .await
            .unwrap();

        tokio::fs::write(dir.path().join("99.msgpack.tmp"), b"junk")
            .await
            .unwrap();

        let mut ids = store.list_aggregates().await.expect("list is Ok");
        ids.sort_by_key(|id| id.get());
        assert_eq!(ids, vec![id1, id2], "list returns exactly the created ids");
    }

    mod proptests_eda {
        use super::*;
        use proptest::prelude::*;

        fn rt() -> tokio::runtime::Runtime {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
        }

        fn evs(n: usize) -> Vec<TestEvent> {
            (0..n)
                .map(|i| TestEvent::Created {
                    name: format!("e{i}"),
                })
                .collect()
        }

        proptest! {
            #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]

            #[test]
            fn create_append_reload_is_monotone_and_durable(
                create_n in 1usize..5,
                batches in proptest::collection::vec(1usize..4, 1..4),
            ) {
                let dir = tempfile::tempdir().unwrap();
                let store = MsgpackFileStore::new(dir.path());
                let runtime = rt();

                let (id, created) = runtime.block_on(
                    store.create(evs(create_n), no_ctx()),
                ).unwrap();
                let mut last_seq = created.last().unwrap().sequence();
                let mut total = create_n;

                for batch_n in &batches {
                    let appended = runtime.block_on(store.append(
                        id,
                        last_seq,
                        evs(*batch_n),
                        no_ctx(),
                    )).unwrap();
                    prop_assert_eq!(appended.len(), *batch_n);
                    for (i, e) in appended.iter().enumerate() {
                        let expected = last_seq.get() + 1 + u64::try_from(i).unwrap();
                        prop_assert_eq!(e.sequence().get(), expected);
                        prop_assert_eq!(e.aggregate_id(), id);
                    }
                    total += batch_n;
                    last_seq = appended.last().unwrap().sequence();
                }

                let all = runtime.block_on(store.load(id)).unwrap();
                prop_assert_eq!(all.len(), total);
                for (i, e) in all.iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                    prop_assert_eq!(e.aggregate_id(), id);
                    prop_assert!(!e.event_id().is_nil());
                }
            }

            #[test]
            fn reload_after_restart_preserves_stream(
                n in 1usize..6,
            ) {
                let dir = tempfile::tempdir().unwrap();
                let runtime = rt();

                let (id, original) = {
                    let store = MsgpackFileStore::new(dir.path());
                    runtime.block_on(store.create(evs(n), no_ctx())).unwrap()
                };

                let store2 = MsgpackFileStore::<TestEvent>::new(dir.path());
                let loaded = runtime.block_on(store2.load(id)).unwrap();

                prop_assert_eq!(loaded.len(), n);
                for (i, e) in loaded.iter().enumerate() {
                    let expected = u64::try_from(i).unwrap() + 1;
                    prop_assert_eq!(e.sequence().get(), expected);
                    prop_assert_eq!(e.event_id(), original[i].event_id());
                    prop_assert_eq!(e.payload().clone(), original[i].payload().clone());
                }
            }

            #[test]
            fn append_rejects_any_stale_expected_sequence(
                create_n in 2usize..6,
                stale_raw in 1u64..1_000,
            ) {
                let dir = tempfile::tempdir().unwrap();
                let store = MsgpackFileStore::new(dir.path());
                let runtime = rt();

                let (id, created) = runtime.block_on(
                    store.create(evs(create_n), no_ctx()),
                ).unwrap();
                let real = created.last().unwrap().sequence().get();
                prop_assume!(stale_raw != real);
                let stale = NonZeroU64::new(stale_raw).unwrap();
                let err = runtime.block_on(store.append(
                    id,
                    stale,
                    evs(1),
                    no_ctx(),
                )).unwrap_err();
                match err {
                    StoreError::ConcurrencyConflict {
                        aggregate_id,
                        expected_sequence,
                        actual_sequence,
                    } => {
                        prop_assert_eq!(aggregate_id, id);
                        prop_assert_eq!(expected_sequence, stale);
                        prop_assert_eq!(actual_sequence, real);
                    }
                    other => prop_assert!(false, "expected ConcurrencyConflict, got {other:?}"),
                }
                let loaded = runtime.block_on(store.load(id)).unwrap();
                prop_assert_eq!(loaded.len(), create_n);
            }

            #[test]
            fn create_assigns_distinct_sequential_ids(k in 1usize..8) {
                let dir = tempfile::tempdir().unwrap();
                let store = MsgpackFileStore::new(dir.path());
                let runtime = rt();

                let mut ids = Vec::with_capacity(k);
                for i in 0..k {
                    let (id, _) = runtime.block_on(store.create(
                        vec![TestEvent::Created { name: format!("agg{i}") }],
                        no_ctx(),
                    )).unwrap();
                    ids.push(id.get());
                }
                let expected: Vec<u64> = (1..=u64::try_from(k).unwrap()).collect();
                prop_assert_eq!(ids, expected);
            }
        }
    }
}
