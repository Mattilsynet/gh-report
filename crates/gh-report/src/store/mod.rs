//! Native pardosa-backed event store for gh-report.
//!
//! Each repository's natural domain key maps to one pardosa fiber. The
//! first capture of a repo begins a fiber; subsequent captures append to
//! the same fiber, recovered across restarts by validated-identity resume
//! (PGN-0014) keyed through a [`pardosa::FiberIndex`] over the domain key.
//! Removal is a soft delete via fiber detach; a returning repo is rescued.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use pardosa::store::{Event, JetStreamBackend, RecoveryOutcome};
use pardosa_fiber_store::FiberStore;
pub use pardosa_fiber_store::FiberStoreError as StoreError;

use crate::event::{DomainEvent, OrgStateCaptured};

/// Pardosa-native event store: one fiber per repository domain key.
pub struct NativeStore {
    inner: FiberStore<DomainEvent>,
    backend_reachable: AtomicBool,
}

/// Pardosa-native org event store: one fiber per org identity.
pub struct NativeOrgStore {
    inner: FiberStore<OrgStateCaptured>,
    backend_reachable: AtomicBool,
}

impl NativeStore {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = FiberStore::<DomainEvent>::create_pgno(path)?;
        Ok(Self::from_store(store))
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = FiberStore::<DomainEvent>::open_pgno(path)?;
        warn_pgno_recovery("repositories", path, store.last_recovery());
        Ok(Self::from_store(store))
    }

    /// [`Self::create_pgno`] sibling threading an opaque
    /// `adopter_epoch` token into the `.pgno` header (PGN-0021, OSF
    /// phase 3). Callers pass
    /// [`crate::config::EVIDENCE_SCHEMA_VERSION`] to fail closed on
    /// the next [`Self::open_pgno_with_epoch`] whenever the
    /// deployed evidence schema drifts silently.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno_with_epoch(path: &Path, epoch: &[u8]) -> Result<Self, StoreError> {
        let store = FiberStore::<DomainEvent>::create_pgno_with_epoch(path, epoch)?;
        Ok(Self::from_store(store))
    }

    /// [`Self::open_pgno`] sibling threading an opaque
    /// `adopter_epoch` token (PGN-0021, OSF phase 3): fails closed
    /// with [`StoreError`] wrapping `SemanticEpochMismatch` when the
    /// stored header epoch does not byte-match `epoch`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open,
    /// gate, or fold the backing container.
    pub fn open_pgno_with_epoch(path: &Path, epoch: &[u8]) -> Result<Self, StoreError> {
        let store = FiberStore::<DomainEvent>::open_pgno_with_epoch(path, epoch)?;
        warn_pgno_recovery("repositories", path, store.last_recovery());
        Ok(Self::from_store(store))
    }

    /// Create a fresh JetStream-backed store.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot author
    /// the canonical-empty container on the backend.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime;
    /// JetStream-backed pardosa calls are bridged with
    /// `tokio::task::block_in_place`, which requires a multi-thread runtime.
    pub fn create_jetstream(backend: JetStreamBackend) -> Result<Self, StoreError> {
        let store = FiberStore::<DomainEvent>::create_jetstream(backend)?;
        Ok(Self::from_store(store))
    }

    /// Open an existing JetStream-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot fetch or
    /// rehydrate the JetStream-authoritative line.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime; see
    /// [`Self::create_jetstream`].
    pub fn open_jetstream(backend: JetStreamBackend) -> Result<Self, StoreError> {
        let store = FiberStore::<DomainEvent>::open_jetstream(backend)?;
        Ok(Self::from_store(store))
    }

    fn from_store(store: FiberStore<DomainEvent>) -> Self {
        Self {
            inner: store,
            backend_reachable: AtomicBool::new(true),
        }
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<&RecoveryOutcome> {
        self.inner.last_recovery()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.backend_reachable.load(Ordering::Acquire)
    }

    fn observe_result<T>(&self, result: &Result<T, StoreError>) {
        if matches!(result, Err(StoreError::BackendInfrastructure { .. })) {
            self.backend_reachable.store(false, Ordering::Release);
        } else if result.is_ok() {
            self.backend_reachable.store(true, Ordering::Release);
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_backend_connect_failure_for_test(&self) {
        let result: Result<(), StoreError> = Err(StoreError::BackendInfrastructure {
            op: pardosa::store::BackendOp::Sync,
            source: Box::new(pardosa::store::BackendError::Connect {
                op: pardosa::store::BackendOp::Sync,
                source: Box::new(std::io::Error::other("nats down")),
            }),
        });
        self.observe_result(&result);
    }

    /// Capture a repository state event onto the repo's fiber, growing it
    /// by one event (or beginning it on first capture), then fence.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DivergedFiber`] when the domain key already
    /// maps to more than one fiber, [`StoreError::Infrastructure`] on
    /// pardosa append/sync failure, or [`StoreError::Poisoned`].
    pub fn record(&self, domain_key: &str, event: DomainEvent) -> Result<(), StoreError> {
        let result = self.inner.record(domain_key, event, key_of);
        self.observe_result(&result);
        result
    }

    /// Soft-delete a repository's fiber (detach), then fence. A later
    /// [`Self::record`] of the same key rescues it back to live.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DivergedFiber`], [`StoreError::Infrastructure`],
    /// or [`StoreError::Poisoned`]. A no-op (key never seen / already
    /// detached) returns `Ok(())`.
    pub fn detach(&self, domain_key: &str, event: DomainEvent) -> Result<(), StoreError> {
        let result = self.inner.detach(domain_key, event, key_of);
        self.observe_result(&result);
        result
    }

    /// The latest event of every live (`Defined`) fiber, paired with its
    /// domain key. Detached fibers are excluded — the soft-delete effect.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] on pardosa read failure or
    /// [`StoreError::Poisoned`].
    pub fn latest_per_repo(&self) -> Result<Vec<(String, DomainEvent)>, StoreError> {
        self.inner.latest_defined(key_of)
    }

    /// Every event in the store, in committed line order — the same
    /// stream an external consumer replaying the journal would observe.
    ///
    /// Each item pairs the pardosa envelope `detached` flag with the
    /// domain event payload.
    ///
    /// A projection folding this sequence behaves identically in-process
    /// or in a separate service (EDA boundary: the log is the sole input).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] on pardosa read failure or
    /// [`StoreError::Poisoned`].
    pub fn events(&self) -> Result<Vec<(bool, DomainEvent)>, StoreError> {
        let result = self.inner.all_events();
        self.observe_result(&result);
        result
    }

    /// Fold every event in committed line order without materialising an
    /// owned event vector.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Poisoned`] when the store mutex is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, bool, &DomainEvent),
    ) -> Result<R, StoreError> {
        let result = self.inner.fold_events(init, fold);
        self.observe_result(&result);
        result
    }
}

impl NativeOrgStore {
    /// Create a fresh `.pgno`-backed org store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = FiberStore::<OrgStateCaptured>::create_pgno(path)?;
        Ok(Self::from_store(store))
    }

    /// Open an existing `.pgno`-backed org store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = FiberStore::<OrgStateCaptured>::open_pgno(path)?;
        warn_pgno_recovery("orgs", path, store.last_recovery());
        Ok(Self::from_store(store))
    }

    /// [`Self::create_pgno`] sibling threading an opaque
    /// `adopter_epoch` token (PGN-0021, OSF phase 3); see
    /// [`NativeStore::create_pgno_with_epoch`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno_with_epoch(path: &Path, epoch: &[u8]) -> Result<Self, StoreError> {
        let store = FiberStore::<OrgStateCaptured>::create_pgno_with_epoch(path, epoch)?;
        Ok(Self::from_store(store))
    }

    /// [`Self::open_pgno`] sibling threading an opaque
    /// `adopter_epoch` token (PGN-0021, OSF phase 3); see
    /// [`NativeStore::open_pgno_with_epoch`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open,
    /// gate, or fold the backing container.
    pub fn open_pgno_with_epoch(path: &Path, epoch: &[u8]) -> Result<Self, StoreError> {
        let store = FiberStore::<OrgStateCaptured>::open_pgno_with_epoch(path, epoch)?;
        warn_pgno_recovery("orgs", path, store.last_recovery());
        Ok(Self::from_store(store))
    }

    /// Create a fresh JetStream-backed org store.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot author
    /// the canonical-empty container on the backend.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime; see
    /// [`NativeStore::create_jetstream`].
    pub fn create_jetstream(backend: JetStreamBackend) -> Result<Self, StoreError> {
        let store = FiberStore::<OrgStateCaptured>::create_jetstream(backend)?;
        Ok(Self::from_store(store))
    }

    /// Open an existing JetStream-backed org store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot fetch or
    /// rehydrate the JetStream-authoritative line.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime; see
    /// [`NativeStore::create_jetstream`].
    pub fn open_jetstream(backend: JetStreamBackend) -> Result<Self, StoreError> {
        let store = FiberStore::<OrgStateCaptured>::open_jetstream(backend)?;
        Ok(Self::from_store(store))
    }

    fn from_store(store: FiberStore<OrgStateCaptured>) -> Self {
        Self {
            inner: store,
            backend_reachable: AtomicBool::new(true),
        }
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<&RecoveryOutcome> {
        self.inner.last_recovery()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.backend_reachable.load(Ordering::Acquire)
    }

    fn observe_result<T>(&self, result: &Result<T, StoreError>) {
        if matches!(result, Err(StoreError::BackendInfrastructure { .. })) {
            self.backend_reachable.store(false, Ordering::Release);
        } else if result.is_ok() {
            self.backend_reachable.store(true, Ordering::Release);
        }
    }

    /// Capture an org state event onto the org fiber, then fence.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DivergedFiber`] when the org key already maps
    /// to more than one fiber, [`StoreError::Infrastructure`] on pardosa
    /// append/sync failure, or [`StoreError::Poisoned`].
    pub fn record(&self, org_key: &str, event: OrgStateCaptured) -> Result<(), StoreError> {
        let result = self.inner.record(org_key, event, org_key_of);
        self.observe_result(&result);
        result
    }

    /// Fold every org event in committed line order without materialising an owned vector.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Poisoned`] when the store mutex is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, &OrgStateCaptured),
    ) -> Result<R, StoreError> {
        let result = self.inner.fold_defined_events(init, fold);
        self.observe_result(&result);
        result
    }
}

fn warn_pgno_recovery(store: &str, path: &Path, recovery: Option<&RecoveryOutcome>) {
    if let Some(recovery) = recovery {
        tracing::warn!(
            event = "gh_report_pgno_recovery",
            store,
            path = %path.display(),
            reader_error = recovery.reader_error.as_str(),
            recovered_records = recovery.recovered_records,
            truncated_bytes = recovery.truncated_bytes,
            last_durable_offset = recovery.last_durable_offset,
            manifest_message_count = recovery.manifest_message_count,
            "gh-report opened recovered pgno store"
        );
    }
}

fn key_of(event: &Event<DomainEvent>) -> std::iter::Once<String> {
    let domain_key = match event.domain_event() {
        DomainEvent::RepositoryStateCaptured { domain_key, .. }
        | DomainEvent::RepositoryDeleted { domain_key, .. } => domain_key,
    };
    std::iter::once(domain_key.as_str().to_string())
}

fn org_key_of(event: &Event<OrgStateCaptured>) -> std::iter::Once<String> {
    std::iter::once(
        event
            .domain_event()
            .assessment_metadata
            .organization
            .as_str()
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use pardosa::store::{EventStore as PardosaStore, PgnoBackend};
    use tracing_subscriber::fmt::MakeWriter;

    const SYNTHETIC_RECOVERY_RECORDS: u64 = 7;

    #[derive(Clone, Default)]
    struct VecWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl VecWriter {
        fn snapshot(&self) -> String {
            String::from_utf8(self.buf.lock().expect("buffer mutex").clone()).expect("utf-8")
        }
    }

    impl Write for VecWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf
                .lock()
                .expect("buffer mutex")
                .extend_from_slice(data);
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
            .json()
            .with_writer(writer.clone())
            .with_ansi(false)
            .with_target(false)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        writer.snapshot()
    }

    fn synthetic_domain_event(i: u64) -> DomainEvent {
        let domain_key = format!("domain-{i}");
        let repo_name = format!("repo-{i}");
        DomainEvent::RepositoryStateCaptured {
            domain_key: pardosa_schema::NonEmptyEventString::try_new(&domain_key)
                .expect("domain key fits"),
            repo_name: pardosa_schema::NonEmptyEventString::try_new(&repo_name)
                .expect("repo name fits"),
            timestamp: pardosa_schema::Timestamp::from_nanos(i + 1).expect("timestamp fits"),
            evidence: None,
        }
    }

    fn manifest_path(path: &Path) -> std::path::PathBuf {
        let mut os = path.as_os_str().to_os_string();
        os.push(".pgix");
        std::path::PathBuf::from(os)
    }

    fn synthesize_torn_footer_store(path: &Path, records: u64) -> (u64, u64) {
        {
            let store = NativeStore::create_pgno(path).expect("create synthetic store");
            for i in 0..records {
                store
                    .record(&format!("domain-{i}"), synthetic_domain_event(i))
                    .expect("record synthetic event");
            }
        }
        {
            let mut store = PardosaStore::<DomainEvent>::open_with_backend(PgnoBackend::open(path))
                .expect("open backend-backed synthetic store");
            let _ = store.writer().sync().expect("sync synthetic manifest");
        }
        let manifest_path = manifest_path(path);
        let manifest = pardosa_file::manifest::parse_manifest(
            &std::fs::read(&manifest_path).expect("synthetic manifest bytes"),
        )
        .expect("synthetic manifest parses");
        assert_eq!(
            u64::try_from(manifest.records.len()).expect("manifest records fit"),
            records
        );
        {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open synthetic pgno for torn tail");
            file.write_all(b"stale-footer-tail")
                .expect("append torn synthetic tail");
        }
        let original_len = std::fs::metadata(path).expect("pgno metadata").len();
        (manifest.data_end, original_len)
    }

    #[test]
    fn synthetic_torn_footer_store_reports_recovery_outcome_and_gh_report_warn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.pgno");
        let (data_end, original_len) =
            synthesize_torn_footer_store(&path, SYNTHETIC_RECOVERY_RECORDS);

        let mut opened = None;
        let output = capture_tracing(|| {
            opened = Some(NativeStore::open_pgno(&path).expect("open recovered gh-report store"));
        });
        let store = opened.expect("store captured");
        let recovery = store.last_recovery().expect("last recovery");

        assert_eq!(output.matches("pgno_torn_tail_recovered").count(), 1);
        assert_eq!(output.matches("gh_report_pgno_recovery").count(), 1);
        assert_eq!(recovery.truncated_bytes, original_len - data_end);
        assert!(recovery.truncated_bytes > 0);
        assert_eq!(recovery.last_durable_offset, data_end);
        assert_eq!(recovery.recovered_records, SYNTHETIC_RECOVERY_RECORDS);
        assert_eq!(recovery.manifest_message_count, SYNTHETIC_RECOVERY_RECORDS);
        assert_eq!(
            u64::try_from(store.events().expect("events").len()).expect("event count fits"),
            SYNTHETIC_RECOVERY_RECORDS
        );
    }

    /// OSF phase 3 proof: the v0.1.17-class silent mix (15.0 -> 16.0,
    /// no struct change) now fails closed at boot instead of silently
    /// mixing schema-incompatible events on one fiber (PGN-0021).
    #[test]
    fn open_pgno_with_epoch_fails_closed_on_stored_epoch_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.pgno");

        let seeded = NativeStore::create_pgno_with_epoch(&path, b"15.0").expect("seed v15.0 store");
        seeded
            .record("domain-0", synthetic_domain_event(0))
            .expect("record synthetic event");
        drop(seeded);

        let Err(error) = NativeStore::open_pgno_with_epoch(&path, b"16.0") else {
            panic!("opening a 15.0-epoch store with the 16.0 epoch must fail closed")
        };
        let message = error.to_string();
        assert!(
            message.contains("semantic epoch mismatch")
                && message.contains("expected Some([49, 54, 46, 48])")
                && message.contains("found Some([49, 53, 46, 48])"),
            "expected a SemanticEpochMismatch(expected=16.0, found=15.0) error, got: {message}"
        );
    }

    /// Same-epoch happy path: the fence must not false-positive when
    /// the stored and expected epochs match (the normal case).
    #[test]
    fn open_pgno_with_epoch_succeeds_on_matching_epoch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.pgno");

        let seeded = NativeStore::create_pgno_with_epoch(&path, b"16.0").expect("seed 16.0 store");
        seeded
            .record("domain-0", synthetic_domain_event(0))
            .expect("record synthetic event");
        drop(seeded);

        NativeStore::open_pgno_with_epoch(&path, b"16.0")
            .expect("opening a 16.0-epoch store with the 16.0 epoch must succeed");
    }
}
