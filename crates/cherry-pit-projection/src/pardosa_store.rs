use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::Path;

use cherry_pit_core::AggregateId;
use cherry_pit_core::ProjectionCheckpoint;
use pardosa::store::{Event as PardosaEvent, HasEventSchemaSource};
use pardosa_fiber_store::{FiberStoreError, ObservedFiberStore};
use pardosa_schema::{EventBytes, EventString, GenomeSafe};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{ProjectionError, ProjectionResult};

const PROJECTION_NAME_MAX: usize = 128;
const SNAPSHOT_BYTES_MAX: usize = 262_144;

type ProjectionNameStr = EventString<PROJECTION_NAME_MAX>;
type SnapshotBytesDto = EventBytes<SNAPSHOT_BYTES_MAX>;

const KIND_SNAPSHOT: u8 = 0;
const KIND_CHECKPOINT: u8 = 1;

#[derive(Clone, GenomeSafe)]
#[repr(u8)]
enum PardosaProjectionRecord {
    Snapshot {
        aggregate_id: u64,
        projection_name: ProjectionNameStr,
        bytes: SnapshotBytesDto,
    } = 0,
    Checkpoint {
        aggregate_id: u64,
        projection_name: ProjectionNameStr,
        last_sequence: u64,
    } = 1,
    Tombstone {
        aggregate_id: u64,
        projection_name: ProjectionNameStr,
        kind: u8,
    } = 2,
}

impl HasEventSchemaSource for PardosaProjectionRecord {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = None;
}

fn record_key(event: &PardosaEvent<PardosaProjectionRecord>) -> std::iter::Once<String> {
    let key = match event.domain_event() {
        PardosaProjectionRecord::Snapshot {
            aggregate_id,
            projection_name,
            ..
        } => format!("{aggregate_id}:{}:snapshot", projection_name.as_str()),
        PardosaProjectionRecord::Checkpoint {
            aggregate_id,
            projection_name,
            ..
        } => format!("{aggregate_id}:{}:checkpoint", projection_name.as_str()),
        PardosaProjectionRecord::Tombstone {
            aggregate_id,
            projection_name,
            kind,
        } => {
            let suffix = if *kind == KIND_SNAPSHOT {
                "snapshot"
            } else {
                "checkpoint"
            };
            format!("{aggregate_id}:{}:{suffix}", projection_name.as_str())
        }
    };
    std::iter::once(key)
}

fn snapshot_key(aggregate_id: AggregateId, projection_name: &str) -> String {
    format!("{}:{projection_name}:snapshot", aggregate_id.get())
}

fn checkpoint_key(aggregate_id: AggregateId, projection_name: &str) -> String {
    format!("{}:{projection_name}:checkpoint", aggregate_id.get())
}

fn to_infra(error: FiberStoreError) -> ProjectionError {
    ProjectionError::Infrastructure(Box::new(error))
}

fn to_bounded_name(name: &str) -> ProjectionResult<ProjectionNameStr> {
    ProjectionNameStr::try_from(name.to_string())
        .map_err(|e| ProjectionError::Infrastructure(Box::new(e)))
}

fn encode_snapshot<P: Serialize>(projection: &P) -> ProjectionResult<SnapshotBytesDto> {
    let json =
        serde_json::to_vec(projection).map_err(|e| ProjectionError::Infrastructure(Box::new(e)))?;
    SnapshotBytesDto::try_from(json).map_err(|e| ProjectionError::Infrastructure(Box::new(e)))
}

fn decode_snapshot<P: DeserializeOwned>(bytes: &SnapshotBytesDto) -> ProjectionResult<P> {
    serde_json::from_slice(bytes).map_err(|e| ProjectionError::CorruptData(Box::new(e)))
}

/// Pardosa-backed PERSISTENT projection storage backend (CHE-0048, amended
/// R1/R10). Sibling of [`crate::FileProjectionStore`] — same key shape
/// `(aggregate_id, projection_name)`, same CHE-0048:R2 snapshot-then-
/// checkpoint write ordering, different storage medium: an append-only
/// pardosa fiber store rather than msgpack files.
///
/// Each snapshot/checkpoint pair is recorded onto its own fiber (one fiber
/// per `(aggregate_id, projection_name, kind)` triple); loads take the
/// latest live record per fiber (latest-wins).
pub struct PardosaProjectionStore<P> {
    store: ObservedFiberStore<PardosaProjectionRecord>,
    projection_name: String,
    _projection: PhantomData<fn() -> P>,
}

impl<P> PardosaProjectionStore<P> {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Infrastructure`] when pardosa cannot
    /// create the backing container.
    pub fn create_pgno(path: &Path, projection_name: impl Into<String>) -> ProjectionResult<Self> {
        let store = ObservedFiberStore::create_pgno(path).map_err(to_infra)?;
        Ok(Self {
            store,
            projection_name: projection_name.into(),
            _projection: PhantomData,
        })
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Infrastructure`] when pardosa cannot open
    /// or fold the backing container.
    pub fn open_pgno(path: &Path, projection_name: impl Into<String>) -> ProjectionResult<Self> {
        let store = ObservedFiberStore::open_pgno(path).map_err(to_infra)?;
        Ok(Self {
            store,
            projection_name: projection_name.into(),
            _projection: PhantomData,
        })
    }

    /// Stable projection identity used as part of every record key.
    #[must_use]
    pub fn projection_name(&self) -> &str {
        &self.projection_name
    }
}

impl<P> PardosaProjectionStore<P>
where
    P: Serialize + DeserializeOwned,
{
    /// Persist `projection` and then its checkpoint (CHE-0048:R2 ordering:
    /// snapshot append strictly before checkpoint append).
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Infrastructure`] for pardosa write
    /// failures or oversized snapshot encodings. Returns
    /// [`ProjectionError::CheckpointRegression`] (CHE-0097:R1) when
    /// `last_sequence` is lower than the existing checkpoint.
    pub async fn persist(
        &self,
        aggregate_id: AggregateId,
        projection: &P,
        last_sequence: NonZeroU64,
    ) -> ProjectionResult<()> {
        if let Some(existing) = self.load_checkpoint(aggregate_id).await?
            && last_sequence < existing.last_sequence()
        {
            return Err(ProjectionError::CheckpointRegression {
                existing: existing.last_sequence(),
                attempted: last_sequence,
            });
        }

        let projection_name = to_bounded_name(&self.projection_name)?;
        let bytes = encode_snapshot(projection)?;
        let snapshot = PardosaProjectionRecord::Snapshot {
            aggregate_id: aggregate_id.get(),
            projection_name: projection_name.clone(),
            bytes,
        };
        self.store
            .record(
                &snapshot_key(aggregate_id, &self.projection_name),
                snapshot,
                record_key,
            )
            .map_err(to_infra)?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "snapshot_written",
            "pardosa snapshot persisted",
        );

        let checkpoint = PardosaProjectionRecord::Checkpoint {
            aggregate_id: aggregate_id.get(),
            projection_name,
            last_sequence: last_sequence.get(),
        };
        self.store
            .record(
                &checkpoint_key(aggregate_id, &self.projection_name),
                checkpoint,
                record_key,
            )
            .map_err(to_infra)?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "checkpoint_written",
            "pardosa checkpoint persisted",
        );
        Ok(())
    }

    /// Load the latest persisted projection snapshot, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::CorruptData`] when snapshot bytes cannot
    /// deserialize as `P`.
    #[expect(
        clippy::unused_async,
        reason = "async fn matches FileProjectionStore's call-site-compatible async surface; pardosa's facade is sync (PGN-0010:R5 bridge convention)"
    )]
    pub async fn load_snapshot(&self, aggregate_id: AggregateId) -> ProjectionResult<Option<P>> {
        let latest = self.store.latest_defined(record_key).map_err(to_infra)?;
        let key = snapshot_key(aggregate_id, &self.projection_name);
        for (found_key, record) in latest {
            if found_key != key {
                continue;
            }
            if let PardosaProjectionRecord::Snapshot { bytes, .. } = record {
                return decode_snapshot(&bytes).map(Some);
            }
        }
        Ok(None)
    }

    /// Load the latest persisted checkpoint, if one exists.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::CorruptData`] when checkpoint identity
    /// does not match this backend.
    #[expect(
        clippy::unused_async,
        reason = "async fn matches FileProjectionStore's call-site-compatible async surface; pardosa's facade is sync (PGN-0010:R5 bridge convention)"
    )]
    pub async fn load_checkpoint(
        &self,
        aggregate_id: AggregateId,
    ) -> ProjectionResult<Option<ProjectionCheckpoint>> {
        let latest = self.store.latest_defined(record_key).map_err(to_infra)?;
        let key = checkpoint_key(aggregate_id, &self.projection_name);
        for (found_key, record) in latest {
            if found_key != key {
                continue;
            }
            let PardosaProjectionRecord::Checkpoint {
                aggregate_id: found_aggregate_id,
                projection_name,
                last_sequence,
            } = record
            else {
                continue;
            };
            let last_sequence = NonZeroU64::new(last_sequence).ok_or_else(|| {
                ProjectionError::CorruptData("checkpoint sequence must be non-zero".into())
            })?;
            let found_aggregate_id = NonZeroU64::new(found_aggregate_id).ok_or_else(|| {
                ProjectionError::CorruptData("checkpoint aggregate id must be non-zero".into())
            })?;
            let checkpoint = ProjectionCheckpoint::new(
                AggregateId::new(found_aggregate_id),
                projection_name.as_str(),
                last_sequence,
            );
            if checkpoint.aggregate_id() != aggregate_id
                || checkpoint.projection_name() != self.projection_name
            {
                return Err(ProjectionError::CorruptData(
                    "checkpoint identity mismatch".into(),
                ));
            }
            return Ok(Some(checkpoint));
        }
        Ok(None)
    }

    /// Delete the snapshot and checkpoint for `aggregate_id`.
    ///
    /// Order is the inverse of [`Self::persist`]: the checkpoint fiber is
    /// detached first, then the snapshot fiber, preserving the invariant
    /// `checkpoint exists => snapshot exists` across a crash mid-delete.
    ///
    /// # Errors
    ///
    /// Returns [`ProjectionError::Infrastructure`] for pardosa write
    /// failures.
    #[expect(
        clippy::unused_async,
        reason = "async fn matches FileProjectionStore's call-site-compatible async surface; pardosa's facade is sync (PGN-0010:R5 bridge convention)"
    )]
    pub async fn delete(&self, aggregate_id: AggregateId) -> ProjectionResult<()> {
        let projection_name = to_bounded_name(&self.projection_name)?;
        let checkpoint_tombstone = PardosaProjectionRecord::Tombstone {
            aggregate_id: aggregate_id.get(),
            projection_name: projection_name.clone(),
            kind: KIND_CHECKPOINT,
        };
        self.store
            .detach(
                &checkpoint_key(aggregate_id, &self.projection_name),
                checkpoint_tombstone,
                record_key,
            )
            .map_err(to_infra)?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "checkpoint_removed",
            "pardosa checkpoint deleted",
        );

        let snapshot_tombstone = PardosaProjectionRecord::Tombstone {
            aggregate_id: aggregate_id.get(),
            projection_name,
            kind: KIND_SNAPSHOT,
        };
        self.store
            .detach(
                &snapshot_key(aggregate_id, &self.projection_name),
                snapshot_tombstone,
                record_key,
            )
            .map_err(to_infra)?;
        tracing::info!(
            target: "cherry_pit_projection",
            boundary = "snapshot_removed",
            "pardosa snapshot deleted",
        );
        Ok(())
    }
}

impl<P> std::fmt::Debug for PardosaProjectionStore<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PardosaProjectionStore")
            .field("projection_name", &self.projection_name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl<P> PardosaProjectionStore<P>
where
    P: Serialize,
{
    async fn persist_crash_after_snapshot(
        &self,
        aggregate_id: AggregateId,
        projection: &P,
    ) -> ProjectionResult<()> {
        let projection_name = to_bounded_name(&self.projection_name)?;
        let bytes = encode_snapshot(projection)?;
        let snapshot = PardosaProjectionRecord::Snapshot {
            aggregate_id: aggregate_id.get(),
            projection_name,
            bytes,
        };
        self.store
            .record(
                &snapshot_key(aggregate_id, &self.projection_name),
                snapshot,
                record_key,
            )
            .map_err(to_infra)?;
        Err(ProjectionError::Infrastructure(
            "simulated crash after snapshot before checkpoint".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct CounterView {
        total: u64,
    }

    fn aggregate_id(value: u64) -> AggregateId {
        AggregateId::new(NonZeroU64::new(value).expect("non-zero id"))
    }

    fn seq(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).expect("non-zero sequence")
    }

    fn temp_pgno_path() -> tempfile::TempPath {
        let file = tempfile::NamedTempFile::new().expect("create temp file");
        let path = file.into_temp_path();
        std::fs::remove_file(&path).expect("clear placeholder so create_pgno starts fresh");
        path
    }

    #[tokio::test]
    async fn persist_then_load_returns_latest_snapshot_and_checkpoint() {
        let path = temp_pgno_path();
        let store =
            PardosaProjectionStore::<CounterView>::create_pgno(&path, "counter_view").unwrap();
        let id = aggregate_id(1);

        store
            .persist(id, &CounterView { total: 4 }, seq(4))
            .await
            .expect("persist succeeds");

        let snapshot = store.load_snapshot(id).await.expect("load snapshot");
        assert_eq!(snapshot, Some(CounterView { total: 4 }));

        let checkpoint = store
            .load_checkpoint(id)
            .await
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(checkpoint.last_sequence(), seq(4));
        assert_eq!(checkpoint.aggregate_id(), id);
        assert_eq!(checkpoint.projection_name(), "counter_view");

        store
            .persist(id, &CounterView { total: 9 }, seq(9))
            .await
            .expect("second persist succeeds");
        let latest = store
            .load_snapshot(id)
            .await
            .expect("load snapshot")
            .expect("snapshot exists");
        assert_eq!(latest, CounterView { total: 9 }, "latest-wins on load");
    }

    #[tokio::test]
    async fn delete_removes_both_snapshot_and_checkpoint() {
        let path = temp_pgno_path();
        let store =
            PardosaProjectionStore::<CounterView>::create_pgno(&path, "counter_view").unwrap();
        let id = aggregate_id(1);
        store
            .persist(id, &CounterView { total: 4 }, seq(4))
            .await
            .expect("persist succeeds");

        store.delete(id).await.expect("delete succeeds");

        assert_eq!(store.load_snapshot(id).await.expect("load"), None);
        assert_eq!(store.load_checkpoint(id).await.expect("load"), None);
    }

    #[tokio::test]
    async fn ordering_crash_after_snapshot_leaves_checkpoint_absent() {
        let path = temp_pgno_path();
        let store =
            PardosaProjectionStore::<CounterView>::create_pgno(&path, "counter_view").unwrap();
        let id = aggregate_id(1);

        let crash = store
            .persist_crash_after_snapshot(id, &CounterView { total: 7 })
            .await;
        assert!(crash.is_err());

        assert_eq!(
            store.load_snapshot(id).await.expect("load"),
            Some(CounterView { total: 7 }),
            "snapshot must be durably visible after the simulated crash"
        );
        assert_eq!(
            store.load_checkpoint(id).await.expect("load"),
            None,
            "checkpoint must be absent: crash happened before checkpoint append (CHE-0048:R2)"
        );
    }

    #[tokio::test]
    async fn keyed_per_aggregate_and_projection_name() {
        let path = temp_pgno_path();
        let store_a = PardosaProjectionStore::<CounterView>::create_pgno(&path, "view_a").unwrap();
        let id1 = aggregate_id(1);
        let id2 = aggregate_id(2);

        store_a
            .persist(id1, &CounterView { total: 1 }, seq(1))
            .await
            .expect("persist id1");
        store_a
            .persist(id2, &CounterView { total: 2 }, seq(1))
            .await
            .expect("persist id2");

        assert_eq!(
            store_a.load_snapshot(id1).await.expect("load"),
            Some(CounterView { total: 1 })
        );
        assert_eq!(
            store_a.load_snapshot(id2).await.expect("load"),
            Some(CounterView { total: 2 })
        );

        let store_b = PardosaProjectionStore::<CounterView>::open_pgno(&path, "view_b").unwrap();
        assert_eq!(
            store_b.load_snapshot(id1).await.expect("load"),
            None,
            "distinct projection_name must not observe view_a's snapshot"
        );
    }

    #[tokio::test]
    async fn checkpoint_regression_rejected() {
        let path = temp_pgno_path();
        let store =
            PardosaProjectionStore::<CounterView>::create_pgno(&path, "counter_view").unwrap();
        let id = aggregate_id(1);
        store
            .persist(id, &CounterView { total: 4 }, seq(4))
            .await
            .expect("persist succeeds");

        let result = store.persist(id, &CounterView { total: 1 }, seq(2)).await;
        assert!(matches!(
            result,
            Err(ProjectionError::CheckpointRegression {
                existing,
                attempted,
            }) if existing == seq(4) && attempted == seq(2)
        ));
    }
}
