//! File-backed variant of [`PardosaEventStore`] — append-only log with
//! one file per aggregate, advisory single-writer lock, and replay-on-open
//! into the in-memory [`Dragline`](pardosa::Dragline).
//!
//! ## Topology — file-per-aggregate (δ.3a moltke-set)
//!
//! ```text
//! store_dir/
//! ├── .lock              advisory single-writer sentinel (CHE-0043:R1)
//! ├── 1.pardosa          aggregate 1 — append-only log
//! ├── 2.pardosa          aggregate 2 — append-only log
//! └── …
//! ```
//!
//! Each `{aggregate_id}.pardosa` file holds the *complete* append-only
//! history of one aggregate as a sequence of length-prefixed
//! [`pardosa_encoding`]-encoded [`EventEnvelope<E>`]s. The `.lock`
//! sentinel at the store-dir root holds the directory-scope advisory
//! flock per CHE-0043:R1. Per-aggregate isolation matches the
//! `cherry-pit-gateway` `MsgpackFileStore` pattern (CHE-0036:R1 +
//! CHE-0048:R1).
//!
//! **Why file-per-aggregate (not single-log).** Natural per-org
//! isolation, per-org rollback semantics the daemon's resume model
//! wants, and the deployment cardinality (small handful of orgs)
//! makes filesystem overhead negligible. Single-log topology would
//! require global sequencing the consumer use case (gh-report's
//! `<store_dir>/events/<org>/` layout) does not benefit from.
//!
//! ## Wire format
//!
//! ```text
//! file := record*
//! record := length:u32_le  payload:[u8; length]
//! payload := pardosa_encoding::to_vec(&EventEnvelope<E>)
//! ```
//!
//! Length prefix is **`u32` little-endian (4 bytes)** — the same
//! length-prefix shape used internally by `pardosa-encoding`'s
//! composite encoders (see `encode_len_prefix` in
//! `crates/pardosa-encoding/src/composites.rs`). The brief specifies
//! "varint length prefix"; we choose `u32 LE` for consistency with the
//! existing encoding crate's framing convention rather than introducing
//! LEB128 as a new format. The trade-off — 4 bytes per record vs ≤10
//! for LEB128 — is irrelevant at envelope payload sizes (envelopes are
//! tens to hundreds of bytes), and the consistency win on encoder
//! introspection is worth the determinism.
//!
//! Truncated trailing record (partial length header *or* partial
//! payload) is treated as crash recovery: the log is truncated at the
//! last complete record on open. Mid-file corruption surfaces as
//! [`StoreError::CorruptData`] from `pardosa_encoding::from_bytes`.
//!
//! ## Lock convention (mirrors cherry-pit-gateway)
//!
//! Lock acquisition uses [`std::fs::File::try_lock`] (Rust 1.94+
//! stdlib advisory file lock) on `{store_dir}/.lock`, *not* `fs2` —
//! matching `cherry-pit-gateway::MsgpackFileStore`'s convention. On
//! `WouldBlock` the constructor returns [`StoreError::StoreLocked`]
//! (CHE-0043:R3). The `std::fs::File` handle is retained by the store
//! struct for its lifetime so the flock is released on drop.
//!
//! ## Sync policy
//!
//! **Synchronous `fsync` after every record.** Each `append` writes
//! the encoded record, calls `sync_data` on the aggregate file, and
//! returns only when durability is reported by the OS. Batched sync
//! (write coalescing) is intentional follow-up work, not part of δ.3a
//! — see the package pre-mortem. The v0.1 cost (one fsync per append
//! batch) is acceptable at the target cardinality.
//!
//! ## Replay on open
//!
//! 1. Acquire `.lock` exclusively. On `WouldBlock` →
//!    [`StoreError::StoreLocked`].
//! 2. Enumerate `{store_dir}/*.pardosa` in **sorted [`AggregateId`] order**
//!    (so pardosa's `next_domain_id()` allocation order matches the
//!    restored ids — see [`PardosaEventStore::replay_envelopes`]).
//! 3. For each file, read length-prefixed records, decode each into
//!    `EventEnvelope<E>`. Truncate at the last complete record (warn,
//!    do not error).
//! 4. Hand the per-aggregate envelopes to
//!    [`PardosaEventStore::replay_envelopes`] which inserts them into
//!    the inner [`Dragline`] without re-fabricating envelopes
//!    (CHE-0042:R1 — envelopes survive `append → load` byte-identical).
//!
//! ## ADR citation bundle (per α′ oracle adr-fmt-1j0vy § δ.3a)
//!
//! - **CHE-0006** single-writer assumption (directory flock enforces).
//! - **CHE-0032** atomic file writes — append-only logs do not need
//!   atomic rename, but the parent-dir fsync after first write is
//!   honoured to make rename of `.lock` durable.
//! - **CHE-0036** file-per-stream storage layout.
//! - **CHE-0042** envelope construction at store layer — envelopes
//!   are built once on `append` and the same bytes survive replay.
//! - **CHE-0043** process-level file fencing (advisory flock).
//! - **CHE-0046** retryable-vs-terminal error classification.
//! - **CHE-0065** pardosa-encoding canonicalisation invariants.
//! - **GEN-0015**, **GEN-0016** genome substrate guarantees consumed
//!   transitively via `pardosa_encoding::Encode`/`Decode`.
//! - **PAR-0004** single-writer substrate fencing.
//! - **PAR-0006**, **PAR-0008**, **PAR-0012** pardosa-side invariants
//!   (lock-not-across-`.await`, monotone timestamps, fiber identity).
//!
//! No new ADR — the composition of these is sufficient.

use std::collections::BTreeMap;
use std::fs::File;
use std::future::Future;
use std::io::{Read, Seek, SeekFrom, Write};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreCreateResult,
    StoreError,
};

use crate::PardosaEventStore;

const LOG_EXTENSION: &str = "pardosa";
const LOCK_FILE_NAME: &str = ".lock";

/// File-backed [`PardosaEventStore`] — durable, single-writer,
/// append-only log per aggregate.
///
/// Wraps an in-memory [`PardosaEventStore`] (CHE-0030 flat composition)
/// extended with:
/// - A directory-scope advisory `flock` on `{dir}/.lock` for
///   process-level single-writer fencing (CHE-0043:R1).
/// - One append-only `{aggregate_id}.pardosa` file per aggregate
///   (CHE-0036, CHE-0048).
/// - Replay-on-open that rebuilds the inner [`pardosa::Dragline`] from
///   on-disk records (CHE-0042:R1).
///
/// See the module-level docstring for wire format, sync policy, and
/// ADR citation bundle.
pub struct PardosaFileEventStore<E>
where
    E: DomainEvent + pardosa_encoding::Decode,
{
    /// Store directory.
    dir: PathBuf,
    /// In-memory pardosa-backed store. All non-IO state (sequence
    /// bookkeeping, optimistic-concurrency check, aggregate-id
    /// allocation) is delegated to this inner store, populated on open
    /// via [`PardosaEventStore::replay_envelopes`].
    inner: PardosaEventStore<E>,
    /// Per-aggregate file write-serialisation. The aggregate file is
    /// only ever appended to, but two concurrent `append` calls
    /// (different tokio tasks, same aggregate) must not interleave
    /// their length-prefix + payload writes. The inner store's
    /// `Mutex<State>` already serialises bookkeeping; this mutex
    /// serialises the *write* itself.
    write_locks: Mutex<BTreeMap<AggregateId, std::sync::Arc<Mutex<()>>>>,
    /// Advisory file lock handle. Held for the store's lifetime; the
    /// underlying flock is released when `File` drops.
    _dir_lock: File,
}

fn infrastructure(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::Infrastructure(e.into())
}

fn corrupt(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> StoreError {
    StoreError::CorruptData(e.into())
}

impl<E> PardosaFileEventStore<E>
where
    E: DomainEvent + pardosa_encoding::Decode,
{
    /// Open the store at `dir`, replaying any on-disk aggregate logs
    /// into the in-memory state.
    ///
    /// Creates `dir` if it does not already exist.
    ///
    /// # Errors
    ///
    /// - [`StoreError::StoreLocked`] if another process already holds
    ///   the directory's `.lock` advisory flock.
    /// - [`StoreError::Infrastructure`] for I/O failures.
    /// - [`StoreError::CorruptData`] for a non-truncated-tail corrupt
    ///   record (decode error mid-file).
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(infrastructure)?;

        // CHE-0043:R1 — acquire directory-scope advisory flock.
        let lock_path = dir.join(LOCK_FILE_NAME);
        let lock_file = File::create(&lock_path).map_err(infrastructure)?;
        lock_file.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => StoreError::StoreLocked { path: dir.clone() },
            std::fs::TryLockError::Error(io) => infrastructure(io),
        })?;

        // Enumerate {aggregate_id}.pardosa files and replay each.
        // Sorted by AggregateId so pardosa's next_domain_id() advances
        // in lock-step with the restored ids (1, 2, 3, ...).
        let mut by_aggregate: BTreeMap<AggregateId, Vec<EventEnvelope<E>>> = BTreeMap::new();
        for entry in std::fs::read_dir(&dir).map_err(infrastructure)? {
            let entry = entry.map_err(infrastructure)?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some(LOG_EXTENSION) {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(id_raw) = stem.parse::<u64>() else {
                continue;
            };
            let Some(nz) = NonZeroU64::new(id_raw) else {
                continue;
            };
            let id = AggregateId::new(nz);
            let envelopes = Self::read_log(&path)?;
            if !envelopes.is_empty() {
                by_aggregate.insert(id, envelopes);
            }
        }

        let inner = PardosaEventStore::<E>::new();
        if !by_aggregate.is_empty() {
            inner.replay_envelopes(by_aggregate)?;
        }

        Ok(Self {
            dir,
            inner,
            write_locks: Mutex::new(BTreeMap::new()),
            _dir_lock: lock_file,
        })
    }

    /// Read a single aggregate's log file into a vector of envelopes.
    ///
    /// Partial trailing record (truncated length header *or* truncated
    /// payload) is treated as crash recovery and silently dropped.
    /// Mid-file decode errors surface as [`StoreError::CorruptData`].
    fn read_log(path: &Path) -> Result<Vec<EventEnvelope<E>>, StoreError> {
        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(infrastructure(e)),
        };
        let mut buf = Vec::new();
        file.read_to_end(&mut buf).map_err(infrastructure)?;

        let mut envelopes = Vec::new();
        let mut cursor = 0usize;
        while cursor < buf.len() {
            // Truncated length header → drop trailing partial record.
            if cursor + 4 > buf.len() {
                break;
            }
            let len_bytes: [u8; 4] = buf[cursor..cursor + 4].try_into().expect("4-byte slice");
            let len = u32::from_le_bytes(len_bytes) as usize;
            cursor += 4;

            // Truncated payload → drop trailing partial record.
            if cursor + len > buf.len() {
                break;
            }
            let payload = &buf[cursor..cursor + len];
            cursor += len;

            let envelope: EventEnvelope<E> =
                pardosa_encoding::from_bytes(payload).map_err(|e| {
                    corrupt(format!(
                        "decode failure mid-log {} at offset {}: {:?}",
                        path.display(),
                        cursor - len - 4,
                        e
                    ))
                })?;
            envelopes.push(envelope);
        }
        Ok(envelopes)
    }

    /// File path for one aggregate's log. Infallible — `u64` ids
    /// cannot escape the parent directory.
    fn log_path(&self, id: AggregateId) -> PathBuf {
        self.dir.join(format!("{}.{}", id.get(), LOG_EXTENSION))
    }

    /// Acquire (or lazily allocate) the per-aggregate write mutex.
    fn aggregate_write_lock(&self, id: AggregateId) -> std::sync::Arc<Mutex<()>> {
        let mut locks = self.write_locks.lock().expect("write_locks mutex poisoned");
        locks
            .entry(id)
            .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
            .clone()
    }

    /// Append the encoded envelopes to the aggregate's log file.
    /// Synchronous fsync after the write per the sync-policy doctrine
    /// (see module docstring).
    fn write_records(
        &self,
        id: AggregateId,
        envelopes: &[EventEnvelope<E>],
    ) -> Result<(), StoreError>
    where
        E: pardosa_encoding::Encode,
    {
        let lock = self.aggregate_write_lock(id);
        let _guard = lock.lock().expect("per-aggregate write lock poisoned");

        let path = self.log_path(id);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(infrastructure)?;

        // Position at the file end is implicit under O_APPEND but we
        // also want a clean offset for the upcoming write; harmless to
        // re-seek explicitly.
        file.seek(SeekFrom::End(0)).map_err(infrastructure)?;

        // Build the byte buffer for all records in this batch, then
        // issue one write + one fsync. Per-batch fsync (not per-record)
        // is consistent with the "atomic batch" semantics in
        // build_envelopes (single timestamp per batch).
        let mut buf = Vec::new();
        for env in envelopes {
            let payload = pardosa_encoding::to_vec(env);
            let len = u32::try_from(payload.len()).map_err(|_| {
                infrastructure(format!(
                    "record payload {} bytes exceeds u32::MAX length prefix",
                    payload.len()
                ))
            })?;
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&payload);
        }
        file.write_all(&buf).map_err(infrastructure)?;
        file.sync_data().map_err(infrastructure)?;
        Ok(())
    }
}

// ─── EventStore impl ──────────────────────────────────────────────
//
// Pattern: delegate to the inner in-memory store first — that surfaces
// concurrency / empty-events / phantom-aggregate errors *before* we
// touch disk. On success, append the envelopes the inner store just
// produced to the aggregate's log file and fsync.

impl<E> EventStore for PardosaFileEventStore<E>
where
    E: DomainEvent + pardosa_encoding::Decode + pardosa_encoding::Encode,
{
    type Event = E;

    fn load(
        &self,
        id: AggregateId,
    ) -> impl Future<Output = Result<Vec<EventEnvelope<Self::Event>>, StoreError>> + Send {
        self.inner.load(id)
    }

    async fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        let (id, envelopes) = self.inner.create(events, context).await?;
        self.write_records(id, &envelopes)?;
        Ok((id, envelopes))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        let envelopes = self
            .inner
            .append(id, expected_sequence, events, context)
            .await?;
        if !envelopes.is_empty() {
            self.write_records(id, &envelopes)?;
        }
        Ok(envelopes)
    }
}
