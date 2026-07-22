//! Native pardosa-backed event store for gh-report.
//!
//! Each repository's natural domain key maps to one pardosa fiber. The
//! first capture of a repo begins a fiber; subsequent captures append to
//! the same fiber, recovered across restarts by validated-identity resume
//! (PGN-0014) keyed through a [`pardosa::FiberIndex`] over the domain key.
//! Removal is a soft delete via fiber detach; a returning repo is rescued.

use std::path::Path;

use pardosa::store::{Event, JetStreamBackend, RecoveryOutcome};
pub use pardosa_fiber_store::FiberStoreError as StoreError;
use pardosa_fiber_store::ObservedFiberStore;

use crate::event::{DomainEvent, OrgStateCaptured, TeamStateCaptured, team_domain_key};

/// Pardosa-native event store: one fiber per repository domain key.
///
/// Thin newtype over [`ObservedFiberStore`], which owns the swappable
/// snapshot and backend-reachability tracking. Swappability backs
/// [`Self::resync_pgno_from_authoritative`] /
/// [`Self::resync_jetstream_from_authoritative`] — the consumer-owned
/// Design-Y re-seed on `FencedConflict` (mission adr-fmt-9a2z7).
pub struct NativeStore(ObservedFiberStore<DomainEvent>);

/// Pardosa-native org event store: one fiber per org identity.
///
/// Thin newtype over [`ObservedFiberStore`]; swappable for the same
/// Design-Y re-seed reason as [`NativeStore`] (mission adr-fmt-9a2z7):
/// `record` can raise the identical `PersistenceError::FencedConflict`
/// the repos store can, through the same generic catch-all — this store
/// needs the same atomic re-seed capability, not just the repos store.
pub struct NativeOrgStore(ObservedFiberStore<OrgStateCaptured>);

/// Pardosa-native team event store: one fiber per `(org, team_slug)` pair,
/// keyed by [`team_domain_key`] (CHE-0089:R2). Team is not the repository
/// aggregate; team-repo is many-to-many via CODEOWNERS, so this store is
/// fully decoupled from [`NativeStore`] and [`NativeOrgStore`].
///
/// Thin newtype over [`ObservedFiberStore`]; swappable for the same
/// Design-Y re-seed reason as [`NativeStore`] (mission adr-fmt-9a2z7):
/// `record` can raise the identical `PersistenceError::FencedConflict`
/// the repos store can.
pub struct NativeTeamStore(ObservedFiberStore<TeamStateCaptured>);

impl NativeStore {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self(ObservedFiberStore::create_pgno(path)?))
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = Self(ObservedFiberStore::open_pgno(path)?);
        warn_pgno_recovery("repositories", path, store.last_recovery().as_ref());
        Ok(store)
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
        Ok(Self(ObservedFiberStore::create_jetstream(backend)?))
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
        Ok(Self(ObservedFiberStore::open_jetstream(backend)?))
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing
    /// file, atomically replacing the fiber-store snapshot in place.
    ///
    /// Design-Y consumer-owned re-arm (adr-fmt-9a2z7): on `FencedConflict`
    /// the caller re-reads authoritative state through this method rather
    /// than patching a cached sequence and redriving the same append
    /// (R10-forbidden). Reuses the existing `FiberStore::open_pgno`
    /// rehydrate path — no pardosa/pardosa-nats changes.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot re-open
    /// the backing container.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        self.0.resync_pgno_from_authoritative(path)?;
        warn_pgno_recovery("repositories", path, self.0.last_recovery().as_ref());
        Ok(())
    }

    /// Re-seed from a fresh authoritative `JetStream` replay, atomically
    /// replacing the fiber-store snapshot in place.
    ///
    /// Same Design-Y re-arm as [`Self::resync_pgno_from_authoritative`],
    /// backed by [`FiberStore::open_jetstream`] — which reaches the
    /// `pardosa-nats` crate's `replay_all` internally on open, correctly
    /// re-seeding the cached fence sequence (adr-fmt-7zpc7 terrain).
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
    pub fn resync_jetstream_from_authoritative(
        &self,
        backend: JetStreamBackend,
    ) -> Result<(), StoreError> {
        self.0.resync_jetstream_from_authoritative(backend)
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<RecoveryOutcome> {
        self.0.last_recovery()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.0.backend_reachable()
    }

    #[cfg(test)]
    pub(crate) fn mark_backend_connect_failure_for_test(&self) {
        self.0.mark_backend_unreachable_for_test();
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
        self.0.record(domain_key, event, key_of)
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
        self.0.detach(domain_key, event, key_of)
    }

    /// The latest event of every live (`Defined`) fiber, paired with its
    /// domain key. Detached fibers are excluded — the soft-delete effect.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] on pardosa read failure or
    /// [`StoreError::Poisoned`].
    pub fn latest_per_repo(&self) -> Result<Vec<(String, DomainEvent)>, StoreError> {
        self.0.latest_defined(key_of)
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
        self.0.all_events()
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
        self.0.fold_events(init, fold)
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
        Ok(Self(ObservedFiberStore::create_pgno(path)?))
    }

    /// Open an existing `.pgno`-backed org store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = Self(ObservedFiberStore::open_pgno(path)?);
        warn_pgno_recovery("orgs", path, store.last_recovery().as_ref());
        Ok(store)
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
        Ok(Self(ObservedFiberStore::create_jetstream(backend)?))
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
        Ok(Self(ObservedFiberStore::open_jetstream(backend)?))
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing
    /// file. See [`NativeStore::resync_pgno_from_authoritative`] for the
    /// Design-Y rationale (mission adr-fmt-9a2z7).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot re-open
    /// the backing container.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        self.0.resync_pgno_from_authoritative(path)?;
        warn_pgno_recovery("orgs", path, self.0.last_recovery().as_ref());
        Ok(())
    }

    /// Re-seed from a fresh authoritative `JetStream` replay. See
    /// [`NativeStore::resync_jetstream_from_authoritative`] for the
    /// Design-Y rationale (mission adr-fmt-9a2z7).
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
    pub fn resync_jetstream_from_authoritative(
        &self,
        backend: JetStreamBackend,
    ) -> Result<(), StoreError> {
        self.0.resync_jetstream_from_authoritative(backend)
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<RecoveryOutcome> {
        self.0.last_recovery()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.0.backend_reachable()
    }

    /// Capture an org state event onto the org fiber, then fence.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DivergedFiber`] when the org key already maps
    /// to more than one fiber, [`StoreError::Infrastructure`] on pardosa
    /// append/sync failure, or [`StoreError::Poisoned`].
    pub fn record(&self, org_key: &str, event: OrgStateCaptured) -> Result<(), StoreError> {
        self.0.record(org_key, event, org_key_of)
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
        self.0.fold_defined_events(init, fold)
    }
}

impl NativeTeamStore {
    /// Create a fresh `.pgno`-backed team store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self(ObservedFiberStore::create_pgno(path)?))
    }

    /// Open an existing `.pgno`-backed team store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = Self(ObservedFiberStore::open_pgno(path)?);
        warn_pgno_recovery("teams", path, store.last_recovery().as_ref());
        Ok(store)
    }

    /// Create a fresh JetStream-backed team store.
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
        Ok(Self(ObservedFiberStore::create_jetstream(backend)?))
    }

    /// Open an existing JetStream-backed team store, rehydrating its fibers.
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
        Ok(Self(ObservedFiberStore::open_jetstream(backend)?))
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing
    /// file. See [`NativeStore::resync_pgno_from_authoritative`] for the
    /// Design-Y rationale (mission adr-fmt-9a2z7).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot re-open
    /// the backing container.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        self.0.resync_pgno_from_authoritative(path)?;
        warn_pgno_recovery("teams", path, self.0.last_recovery().as_ref());
        Ok(())
    }

    /// Re-seed from a fresh authoritative `JetStream` replay. See
    /// [`NativeStore::resync_jetstream_from_authoritative`] for the
    /// Design-Y rationale (mission adr-fmt-9a2z7).
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
    pub fn resync_jetstream_from_authoritative(
        &self,
        backend: JetStreamBackend,
    ) -> Result<(), StoreError> {
        self.0.resync_jetstream_from_authoritative(backend)
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<RecoveryOutcome> {
        self.0.last_recovery()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.0.backend_reachable()
    }

    /// Capture a team roster event onto the team's own fiber
    /// (`team_domain_key`), then fence.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DivergedFiber`] when the team key already maps
    /// to more than one fiber, [`StoreError::Infrastructure`] on pardosa
    /// append/sync failure, or [`StoreError::Poisoned`].
    pub fn record(&self, team_key: &str, event: TeamStateCaptured) -> Result<(), StoreError> {
        self.0.record(team_key, event, team_key_of)
    }

    /// Soft-delete a team's fiber (detach) for a team that no longer
    /// exists or no longer owns any repository, then fence. A later
    /// [`Self::record`] of the same team key rescues it back to live.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::DivergedFiber`], [`StoreError::Infrastructure`],
    /// or [`StoreError::Poisoned`]. A no-op (key never seen / already
    /// detached) returns `Ok(())`.
    pub fn detach(&self, team_key: &str, event: TeamStateCaptured) -> Result<(), StoreError> {
        self.0.detach(team_key, event, team_key_of)
    }

    /// Fold every team event in committed line order without materialising
    /// an owned vector.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Poisoned`] when the store mutex is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, &TeamStateCaptured),
    ) -> Result<R, StoreError> {
        self.0.fold_defined_events(init, fold)
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

fn team_key_of(event: &Event<TeamStateCaptured>) -> std::iter::Once<String> {
    let domain = event.domain_event();
    let key = team_domain_key(domain.org.as_str(), domain.team_slug.as_str())
        .expect("TeamStateCaptured.org/team_slug are NonEmptyEventString, never empty");
    std::iter::once(key)
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

    #[test]
    fn resync_pgno_from_authoritative_observes_writes_the_stale_handle_never_saw() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("events.pgno");

        let long_lived = NativeStore::create_pgno(&path).expect("create long-lived store");
        long_lived
            .record("domain-0", synthetic_domain_event(0))
            .expect("record via long-lived handle");

        {
            let other_writer = NativeStore::open_pgno(&path).expect("second handle opens");
            other_writer
                .record("domain-1", synthetic_domain_event(1))
                .expect("record via second handle");
        }

        assert_eq!(
            long_lived.events().expect("events before resync").len(),
            1,
            "long-lived handle must not see the externally-durable write before resync — \
             this is the staleness the fix targets"
        );

        long_lived
            .resync_pgno_from_authoritative(&path)
            .expect("resync from authoritative pgno");

        assert_eq!(
            long_lived.events().expect("events after resync").len(),
            2,
            "resync must force a fresh authoritative read, not patch the stale cache"
        );
    }

    fn synthetic_team_state(org: &str, team_slug: &str) -> TeamStateCaptured {
        use crate::event::{
            OrgMembershipFetchStatus, OrphanAttributionInputs, TeamRosterStatusEvent,
        };
        use pardosa_schema::{EventVec, NonEmptyEventString};

        TeamStateCaptured {
            org: NonEmptyEventString::try_new(org).expect("org fits"),
            team_slug: NonEmptyEventString::try_new(team_slug).expect("team_slug fits"),
            members: EventVec::try_from(Vec::new()).expect("empty members fits"),
            orphan_attribution_inputs: OrphanAttributionInputs {
                org_membership_fetch_status: OrgMembershipFetchStatus::Fetched,
            },
            fetched_at: pardosa_schema::EventString::try_from("2026-07-16T00:00:00Z".to_string())
                .expect("fetched_at fits"),
            status: TeamRosterStatusEvent::Complete,
        }
    }

    #[test]
    fn team_store_records_and_routes_on_team_domain_key_fiber() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("teams.pgno");
        let store = NativeTeamStore::create_pgno(&path).expect("create team store");

        let key = team_domain_key("acme", "platform").expect("derives key");
        store
            .record(&key, synthetic_team_state("acme", "platform"))
            .expect("record team event onto its own fiber");

        let folded = store
            .fold_events(Vec::new(), |acc, event| acc.push(event.clone()))
            .expect("fold team events");
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].org.as_str(), "acme");
        assert_eq!(folded[0].team_slug.as_str(), "platform");
    }

    #[test]
    fn team_store_is_decoupled_from_repo_and_org_streams() {
        let dir = tempfile::tempdir().expect("tempdir");
        let repo_path = dir.path().join("repos.pgno");
        let team_path = dir.path().join("teams.pgno");

        let repo_store = NativeStore::create_pgno(&repo_path).expect("create repo store");
        repo_store
            .record("domain-1", synthetic_domain_event(1))
            .expect("record repo event");

        let team_store = NativeTeamStore::create_pgno(&team_path).expect("create team store");
        let key = team_domain_key("acme", "platform").expect("derives key");
        team_store
            .record(&key, synthetic_team_state("acme", "platform"))
            .expect("record team event");

        assert_eq!(repo_store.events().expect("repo events").len(), 1);
        assert_eq!(
            team_store
                .fold_events(0_usize, |acc, _| *acc += 1)
                .expect("team fold"),
            1
        );
    }
}
