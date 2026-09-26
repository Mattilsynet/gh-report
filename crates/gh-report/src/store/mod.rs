//! Native pardosa-backed event store for gh-report.
//!
//! Each repository's natural domain key maps to one pardosa fiber.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pardosa::prelude::*;
use pardosa_nats::NatsStorageAdapter;

use crate::event::{DomainEvent, OrgStateCaptured, TeamStateCaptured};

/// Failure surface of the native pardosa store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store already exists: {0}")]
    AlreadyExists(#[source] OperationFailure),
    #[error("pardosa infrastructure error: {0}")]
    Infrastructure(String),
    #[error("pardosa operation failed: {0}")]
    Operation(#[source] OperationFailure),
    #[error("native creation requires reconciliation: {0}")]
    CreationUnknown(#[source] OperationFailure),
    #[error("payload encoding failed: {0}")]
    Encode(#[source] EncodeError),
    #[error("payload decoding failed: {0}")]
    Decode(#[source] DecodeError),
    #[error("write landing requires reconciliation at epoch {carried_epoch}")]
    Indeterminate { carried_epoch: u64 },
    #[error("write landed, but subsequent operation failed: {0}")]
    AfterLanded(#[source] Box<StoreError>),
    #[error("write landed; cache reconciliation required")]
    LandedNeedsReconciliation,
    #[error("concurrency conflict")]
    ConcurrencyConflict {
        expected_seq: Option<u64>,
        actual_seq: Option<u64>,
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("torn write recovery failed: {source}")]
    TornWriteRecovery {
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("domain key {key:?} maps to multiple fibers")]
    DivergedFiber { key: String },
    #[error("store mutex poisoned")]
    Poisoned,
}

impl StoreError {
    #[must_use]
    pub fn is_already_exists(&self) -> bool {
        matches!(self, Self::AlreadyExists(_))
    }
}

impl From<OperationFailure> for StoreError {
    fn from(err: OperationFailure) -> Self {
        match err.condition() {
            FailureCondition::StoreAlreadyExists => StoreError::AlreadyExists(err),
            FailureCondition::ConcurrencyConflict | FailureCondition::StaleEpoch => {
                StoreError::ConcurrencyConflict {
                    expected_seq: None,
                    actual_seq: None,
                    source: Box::new(err),
                }
            }
            _ => StoreError::Operation(err),
        }
    }
}

impl From<EncodeError> for StoreError {
    fn from(err: EncodeError) -> Self {
        StoreError::Encode(err)
    }
}

impl From<DecodeError> for StoreError {
    fn from(err: DecodeError) -> Self {
        StoreError::Decode(err)
    }
}

fn default_claim(epoch: u64, label: &str) -> OwnershipClaimRecord {
    OwnershipClaimRecord {
        epoch,
        machine_id: [0u8; 16],
        boot_id: [0u8; 16],
        process_id: u64::from(std::process::id()),
        process_start_time_ns: 0,
        claim_time_ns: 0,
        operator_label: label.to_string(),
    }
}

enum WriteKnowledge {
    Ready,
    Unknown(u64),
    Landed,
}

impl WriteKnowledge {
    fn check(&self) -> Result<(), StoreError> {
        match self {
            Self::Ready => Ok(()),
            Self::Unknown(carried_epoch) => Err(StoreError::Indeterminate {
                carried_epoch: *carried_epoch,
            }),
            Self::Landed => Err(StoreError::LandedNeedsReconciliation),
        }
    }

    fn accept<T>(&mut self, verdict: WriteLandingVerdict<T>) -> Result<T, StoreError> {
        match verdict {
            WriteLandingVerdict::Landed(value) => {
                *self = Self::Landed;
                Ok(value)
            }
            WriteLandingVerdict::Undetermined { carried_epoch } => {
                *self = Self::Unknown(carried_epoch);
                Err(StoreError::Indeterminate { carried_epoch })
            }
        }
    }
}

fn after_landed(error: impl Into<StoreError>) -> StoreError {
    StoreError::AfterLanded(Box::new(error.into()))
}

fn creation_failure(error: OperationFailure) -> StoreError {
    StoreError::CreationUnknown(error)
}

fn require_absent(presence: ArtefactPresence) -> Result<(), StoreError> {
    match presence {
        ArtefactPresence::None => Ok(()),
        _ => Err(StoreError::AlreadyExists(OperationFailure::new(
            FailureCondition::StoreAlreadyExists,
            "store artefacts precede create",
        ))),
    }
}

#[derive(Clone)]
enum StorageBackend {
    File(PathBuf),
    Nats(Box<NatsStorageAdapter>),
}

#[cfg(test)]
type FileSessionFault = fn(pardosa::file::FileWriterSession) -> pardosa::file::FileWriterSession;

#[cfg(test)]
type BeforeFileSync = Box<dyn FnOnce(&Path)>;

#[cfg(test)]
thread_local! {
    static BEFORE_FILE_SYNC: std::cell::RefCell<Option<BeforeFileSync>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
fn before_file_sync(path: &Path) {
    if let Some(hook) = BEFORE_FILE_SYNC.take() {
        hook(path);
    }
}

struct StoreInner<E> {
    #[cfg(test)]
    session_fault: Mutex<Option<FileSessionFault>>,
    #[cfg(test)]
    write_failure: Mutex<Option<FailureCondition>>,
    backend: StorageBackend,
    cached: Mutex<CachedEvents<E>>,
    knowledge: Mutex<WriteKnowledge>,
}

type CachedEvents<E> = Vec<(bool, [u8; 16], E)>;

fn verify_schema_descriptor<E: PardosaSchema>(
    actual: Option<&SchemaDescriptor>,
) -> Result<(), StoreError> {
    let expected = SchemaDescriptor::new(E::SCHEMA_VERSION, E::schema_descriptor());
    match actual {
        Some(actual) if actual.identity() == expected.identity() => Ok(()),
        Some(actual) => Err(StoreError::Infrastructure(format!(
            "schema descriptor identity mismatch: expected {}, got {}",
            expected.identity().to_hex(),
            actual.identity().to_hex(),
        ))),
        None => Err(StoreError::Infrastructure(
            "missing schema descriptor in store metadata: refusing to open unadmitted store"
                .to_string(),
        )),
    }
}

impl<E: PardosaSchema> StoreInner<E> {
    fn lock_knowledge(&self) -> Result<std::sync::MutexGuard<'_, WriteKnowledge>, StoreError> {
        let state = self.knowledge.lock().map_err(|_| StoreError::Poisoned)?;
        state.check()?;
        Ok(state)
    }

    fn lock_cached(&self) -> Result<std::sync::MutexGuard<'_, CachedEvents<E>>, StoreError> {
        let _state = self.lock_knowledge()?;
        self.cached.lock().map_err(|_| StoreError::Poisoned)
    }

    fn create_pgno(path: &Path, label: &'static str) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let claim = default_claim(1, label);
        let desc = AdmittedDescriptor::try_from_descriptor(SchemaDescriptor::new(
            E::SCHEMA_VERSION,
            E::schema_descriptor(),
        ))?;
        require_absent(adapter.try_presence()?)?;
        let mut session = adapter.create(&claim, &desc).map_err(creation_failure)?;
        #[cfg(test)]
        before_file_sync(path);
        session.sync().map_err(after_landed)?;
        Ok(Self {
            backend: StorageBackend::File(path.to_path_buf()),
            #[cfg(test)]
            write_failure: Mutex::new(None),
            #[cfg(test)]
            session_fault: Mutex::new(None),
            cached: Mutex::new(Vec::new()),
            knowledge: Mutex::new(WriteKnowledge::Ready),
        })
    }

    fn open_pgno(path: &Path, _label: &'static str) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let mut reader = adapter.open_read()?;
        verify_schema_descriptor::<E>(reader.meta_records().schema_descriptor.as_ref())?;
        let envelopes = reader.read_all_envelopes()?;
        let mut cached = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let event = E::decode_payload(&env.payload)?;
            cached.push((env.header.detached, env.header.fiber_id, event));
        }
        Ok(Self {
            backend: StorageBackend::File(path.to_path_buf()),
            #[cfg(test)]
            write_failure: Mutex::new(None),
            #[cfg(test)]
            session_fault: Mutex::new(None),
            cached: Mutex::new(cached),
            knowledge: Mutex::new(WriteKnowledge::Ready),
        })
    }

    fn create_nats(adapter: NatsStorageAdapter, label: &'static str) -> Result<Self, StoreError> {
        let claim = default_claim(1, label);
        let desc = AdmittedDescriptor::try_from_descriptor(SchemaDescriptor::new(
            E::SCHEMA_VERSION,
            E::schema_descriptor(),
        ))?;
        require_absent(adapter.try_presence()?)?;
        let mut session = adapter.create(&claim, &desc).map_err(creation_failure)?;
        session.sync().map_err(after_landed)?;
        Ok(Self {
            backend: StorageBackend::Nats(Box::new(adapter)),
            #[cfg(test)]
            write_failure: Mutex::new(None),
            #[cfg(test)]
            session_fault: Mutex::new(None),
            cached: Mutex::new(Vec::new()),
            knowledge: Mutex::new(WriteKnowledge::Ready),
        })
    }

    fn open_nats(adapter: NatsStorageAdapter, _label: &'static str) -> Result<Self, StoreError> {
        let mut reader = adapter.open_read()?;
        verify_schema_descriptor::<E>(reader.meta_records().schema_descriptor.as_ref())?;
        let envelopes = reader.read_all_envelopes()?;
        let mut cached = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let event = E::decode_payload(&env.payload)?;
            cached.push((env.header.detached, env.header.fiber_id, event));
        }
        Ok(Self {
            backend: StorageBackend::Nats(Box::new(adapter)),
            #[cfg(test)]
            write_failure: Mutex::new(None),
            #[cfg(test)]
            session_fault: Mutex::new(None),
            cached: Mutex::new(cached),
            knowledge: Mutex::new(WriteKnowledge::Ready),
        })
    }

    fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        let _state = self.lock_knowledge()?;
        let adapter = FileStorageAdapter::new(path);
        let mut reader = adapter.open_read()?;
        verify_schema_descriptor::<E>(reader.meta_records().schema_descriptor.as_ref())?;
        let envelopes = reader.read_all_envelopes()?;
        let mut cached = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let event = E::decode_payload(&env.payload)?;
            cached.push((env.header.detached, env.header.fiber_id, event));
        }
        *self.cached.lock().map_err(|_| StoreError::Poisoned)? = cached;
        Ok(())
    }

    fn resync_from_authoritative(&self) -> Result<(), StoreError> {
        let _state = self.lock_knowledge()?;
        let envelopes = match &self.backend {
            StorageBackend::File(path) => {
                let adapter = FileStorageAdapter::new(path);
                let mut reader = adapter.open_read()?;
                verify_schema_descriptor::<E>(reader.meta_records().schema_descriptor.as_ref())?;
                reader.read_all_envelopes()?
            }
            StorageBackend::Nats(adapter) => {
                let mut reader = adapter.open_read()?;
                verify_schema_descriptor::<E>(reader.meta_records().schema_descriptor.as_ref())?;
                reader.read_all_envelopes()?
            }
        };
        let mut cached = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let event = E::decode_payload(&env.payload)?;
            cached.push((env.header.detached, env.header.fiber_id, event));
        }
        *self.cached.lock().map_err(|_| StoreError::Poisoned)? = cached;
        Ok(())
    }

    fn preflight_write_lease(&self) -> Result<(), StoreError> {
        let _state = self.lock_knowledge()?;
        match &self.backend {
            StorageBackend::File(path) => {
                let adapter = FileStorageAdapter::new(path);
                let epoch = adapter.current_epoch()?;
                let _session = adapter.open_write(epoch)?;
                Ok(())
            }
            StorageBackend::Nats(adapter) => {
                let epoch = adapter.current_epoch()?;
                let _session = adapter.open_write(epoch)?;
                Ok(())
            }
        }
    }

    fn record(&self, domain_key: &str, event: E) -> Result<(), StoreError> {
        #[cfg(test)]
        if let Some(condition) = self.write_failure.lock().unwrap().clone() {
            return Err(OperationFailure::new(condition, "injected pre-write failure").into());
        }
        let mut state = self.lock_knowledge()?;
        let fiber_id = derive_fiber_id(domain_key);
        let mut payload = Vec::new();
        event.encode_payload(&mut payload)?;
        let event_id = *uuid::Uuid::now_v7().as_bytes();

        match &self.backend {
            StorageBackend::File(path) => {
                let adapter = FileStorageAdapter::new(path);
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                #[cfg(test)]
                if let Some(inject) = *self.session_fault.lock().unwrap() {
                    session = inject(session);
                }
                let handle = session.fiber(fiber_id)?;
                if handle.is_detached() {
                    state.accept(session.rescue_fiber(fiber_id, event_id, payload)?)?;
                } else {
                    state.accept(session.append_to_fiber(fiber_id, event_id, payload)?)?;
                }
                #[cfg(test)]
                before_file_sync(path);
                session.sync().map_err(after_landed)?;
            }
            StorageBackend::Nats(adapter) => {
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                let handle = session.fiber(fiber_id)?;
                if handle.is_detached() {
                    state.accept(session.rescue_fiber(fiber_id, event_id, payload)?)?;
                } else {
                    state.accept(session.append_to_fiber(fiber_id, event_id, payload)?)?;
                }
                session.sync().map_err(after_landed)?;
            }
        }

        self.cached
            .lock()
            .map_err(|_| after_landed(StoreError::Poisoned))?
            .push((false, fiber_id, event));
        *state = WriteKnowledge::Ready;
        Ok(())
    }

    fn detach(&self, domain_key: &str, event: E) -> Result<bool, StoreError> {
        let mut state = self.lock_knowledge()?;
        let fiber_id = derive_fiber_id(domain_key);
        let mut payload = Vec::new();
        event.encode_payload(&mut payload)?;
        let event_id = *uuid::Uuid::now_v7().as_bytes();

        match &self.backend {
            StorageBackend::File(path) => {
                let adapter = FileStorageAdapter::new(path);
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                #[cfg(test)]
                if let Some(inject) = *self.session_fault.lock().unwrap() {
                    session = inject(session);
                }
                let handle = session.fiber(fiber_id)?;
                if handle.is_active() {
                    state.accept(session.detach_fiber(fiber_id, event_id, payload)?)?;
                    #[cfg(test)]
                    before_file_sync(path);
                    session.sync().map_err(after_landed)?;
                }
            }
            StorageBackend::Nats(adapter) => {
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                let handle = session.fiber(fiber_id)?;
                if handle.is_active() {
                    state.accept(session.detach_fiber(fiber_id, event_id, payload)?)?;
                    session.sync().map_err(after_landed)?;
                }
            }
        }

        let landed = matches!(*state, WriteKnowledge::Landed);
        if landed {
            self.cached
                .lock()
                .map_err(|_| after_landed(StoreError::Poisoned))?
                .push((true, fiber_id, event));
            *state = WriteKnowledge::Ready;
        }
        Ok(landed)
    }

    fn events(&self) -> Result<Vec<(bool, E)>, StoreError>
    where
        E: Clone,
    {
        let cached = self.lock_cached()?;
        Ok(cached
            .iter()
            .map(|(detached, _, event)| (*detached, event.clone()))
            .collect())
    }

    fn fold_events<R>(
        &self,
        init: R,
        mut fold: impl FnMut(&mut R, bool, &E),
    ) -> Result<R, StoreError> {
        let cached = self.lock_cached()?;
        let mut acc = init;
        for (detached, _, event) in cached.iter() {
            fold(&mut acc, *detached, event);
        }
        Ok(acc)
    }

    fn fold_defined_events<R>(
        &self,
        init: R,
        mut fold: impl FnMut(&mut R, &E),
    ) -> Result<R, StoreError> {
        let cached = self.lock_cached()?;
        let mut acc = init;
        for (detached, _, event) in cached.iter() {
            if !*detached {
                fold(&mut acc, event);
            }
        }
        Ok(acc)
    }

    fn latest_defined(&self, key_fn: impl Fn(&E) -> String) -> Result<Vec<(String, E)>, StoreError>
    where
        E: Clone,
    {
        let cached = self.lock_cached()?;
        let mut latest: HashMap<[u8; 16], (String, E)> = HashMap::new();
        for (detached, fiber_id, event) in cached.iter() {
            if *detached {
                latest.remove(fiber_id);
            } else {
                let key = key_fn(event);
                latest.insert(*fiber_id, (key, event.clone()));
            }
        }
        Ok(latest.into_values().collect())
    }
}

/// Pardosa-native event store: one fiber per repository domain key.
pub struct NativeStore {
    inner: StoreInner<DomainEvent>,
    backend_reachable: std::sync::atomic::AtomicBool,
}

pub struct NativeOrgStore {
    inner: StoreInner<OrgStateCaptured>,
    backend_reachable: std::sync::atomic::AtomicBool,
}

pub struct NativeTeamStore {
    inner: StoreInner<TeamStateCaptured>,
    backend_reachable: std::sync::atomic::AtomicBool,
}

impl NativeStore {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    /// Returns [`StoreError`] if creating or claiming the store file fails.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::create_pgno(path, "gh-report-repos")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding existing envelopes fails.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::open_pgno(path, "gh-report-repos")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Create a fresh NATS-backed store.
    ///
    /// # Errors
    /// Returns [`StoreError`] if creating or claiming the store streams fails.
    pub fn create_nats(adapter: NatsStorageAdapter) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::create_nats(adapter, "gh-report-repos")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Open an existing NATS-backed store, rehydrating its fibers.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding existing envelopes fails.
    pub fn open_nats(adapter: NatsStorageAdapter) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::open_nats(adapter, "gh-report-repos")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing file.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding from `path` fails.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        self.inner.resync_pgno_from_authoritative(path)
    }

    pub(crate) fn resync_from_authoritative(&self) -> Result<(), StoreError> {
        self.inner.resync_from_authoritative()
    }

    /// Preflight writer lease ownership on the authoritative store.
    ///
    /// # Errors
    /// Returns [`StoreError`] if another writer session holds the lease, or the meta stream is unreadable.
    pub fn preflight_write_lease(&self) -> Result<(), StoreError> {
        self.inner.preflight_write_lease()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.backend_reachable
            .load(std::sync::atomic::Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn mark_backend_connect_failure_for_test(&self) {
        self.backend_reachable
            .store(false, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn release_exclusion_for_test(&self) {
        let _ = self;
    }

    /// Capture a repository state event onto the repo's fiber.
    ///
    /// # Errors
    /// Returns [`StoreError`] if encoding, appending, or syncing the event fails.
    pub fn record(&self, domain_key: &str, event: DomainEvent) -> Result<(), StoreError> {
        self.inner.record(domain_key, event)
    }

    /// Soft-delete a repository's fiber (detach).
    ///
    /// # Errors
    /// Returns [`StoreError`] if encoding, detaching, or syncing the fiber fails.
    pub fn detach(&self, domain_key: &str, event: DomainEvent) -> Result<(), StoreError> {
        self.inner.detach(domain_key, event).map(|_| ())
    }

    pub(crate) fn detach_effect(
        &self,
        domain_key: &str,
        event: DomainEvent,
    ) -> Result<bool, StoreError> {
        self.inner.detach(domain_key, event)
    }

    /// Return all cached events with their detached status and fiber ID.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn events_with_fibers(&self) -> Result<Vec<(bool, [u8; 16], DomainEvent)>, StoreError> {
        let cached = self.inner.lock_cached()?;
        Ok(cached.clone())
    }

    /// The latest event of every live fiber, paired with its domain key.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn latest_per_repo(&self) -> Result<Vec<(String, DomainEvent)>, StoreError> {
        self.inner.latest_defined(|event| match event {
            DomainEvent::RepositoryStateCaptured { domain_key, .. }
            | DomainEvent::RepositoryDeleted { domain_key, .. } => domain_key.as_str().to_string(),
            DomainEvent::OrgStateCaptured(org) => {
                org.assessment_metadata.organization.as_str().to_string()
            }
            DomainEvent::TeamStateCaptured(team) => {
                crate::event::team_domain_key(team.org.as_str(), team.team_slug.as_str())
                    .unwrap_or_default()
            }
        })
    }

    /// Every event in the store, in committed line order.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn events(&self) -> Result<Vec<(bool, DomainEvent)>, StoreError> {
        self.inner.events()
    }

    /// Fold every event in committed line order without materialising an owned vector.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, bool, &DomainEvent),
    ) -> Result<R, StoreError> {
        self.inner.fold_events(init, fold)
    }
}

impl NativeOrgStore {
    #[cfg(test)]
    pub(crate) fn fail_writes_for_test(&self, condition: FailureCondition) {
        *self.inner.write_failure.lock().unwrap() = Some(condition);
    }
    /// Create a fresh `.pgno`-backed org store, truncating any existing file.
    ///
    /// # Errors
    /// Returns [`StoreError`] if creating or claiming the store file fails.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::create_pgno(path, "gh-report-orgs")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Open an existing `.pgno`-backed org store, rehydrating its fibers.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding existing envelopes fails.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::open_pgno(path, "gh-report-orgs")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Create a fresh NATS-backed org store.
    ///
    /// # Errors
    /// Returns [`StoreError`] if creating or claiming the store streams fails.
    pub fn create_nats(adapter: NatsStorageAdapter) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::create_nats(adapter, "gh-report-orgs")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Open an existing NATS-backed org store, rehydrating its fibers.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding existing envelopes fails.
    pub fn open_nats(adapter: NatsStorageAdapter) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::open_nats(adapter, "gh-report-orgs")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing file.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding from `path` fails.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        self.inner.resync_pgno_from_authoritative(path)
    }

    pub(crate) fn resync_from_authoritative(&self) -> Result<(), StoreError> {
        self.inner.resync_from_authoritative()
    }

    /// Preflight writer lease ownership on the authoritative store.
    ///
    /// # Errors
    /// Returns [`StoreError`] if another writer session holds the lease, or the meta stream is unreadable.
    pub fn preflight_write_lease(&self) -> Result<(), StoreError> {
        self.inner.preflight_write_lease()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.backend_reachable
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Capture an org state event onto the org fiber.
    ///
    /// # Errors
    /// Returns [`StoreError`] if encoding, appending, or syncing the event fails.
    pub fn record(&self, org_key: &str, event: OrgStateCaptured) -> Result<(), StoreError> {
        self.inner.record(org_key, event)
    }

    /// Fold every org event in committed line order without materialising an owned vector.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, &OrgStateCaptured),
    ) -> Result<R, StoreError> {
        self.inner.fold_defined_events(init, fold)
    }
}

impl NativeTeamStore {
    #[cfg(test)]
    pub(crate) fn fail_writes_for_test(&self, condition: FailureCondition) {
        *self.inner.write_failure.lock().unwrap() = Some(condition);
    }
    /// Create a fresh `.pgno`-backed team store, truncating any existing file.
    ///
    /// # Errors
    /// Returns [`StoreError`] if creating or claiming the store file fails.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::create_pgno(path, "gh-report-teams")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Open an existing `.pgno`-backed team store, rehydrating its fibers.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding existing envelopes fails.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::open_pgno(path, "gh-report-teams")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Create a fresh NATS-backed team store.
    ///
    /// # Errors
    /// Returns [`StoreError`] if creating or claiming the store streams fails.
    pub fn create_nats(adapter: NatsStorageAdapter) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::create_nats(adapter, "gh-report-teams")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Open an existing NATS-backed team store, rehydrating its fibers.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding existing envelopes fails.
    pub fn open_nats(adapter: NatsStorageAdapter) -> Result<Self, StoreError> {
        Ok(Self {
            inner: StoreInner::open_nats(adapter, "gh-report-teams")?,
            backend_reachable: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing file.
    ///
    /// # Errors
    /// Returns [`StoreError`] if reading or decoding from `path` fails.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        self.inner.resync_pgno_from_authoritative(path)
    }

    pub(crate) fn resync_from_authoritative(&self) -> Result<(), StoreError> {
        self.inner.resync_from_authoritative()
    }

    /// Preflight writer lease ownership on the authoritative store.
    ///
    /// # Errors
    /// Returns [`StoreError`] if another writer session holds the lease, or the meta stream is unreadable.
    pub fn preflight_write_lease(&self) -> Result<(), StoreError> {
        self.inner.preflight_write_lease()
    }

    #[must_use]
    pub(crate) fn backend_reachable(&self) -> bool {
        self.backend_reachable
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Capture a team roster event onto the team's own fiber.
    ///
    /// # Errors
    /// Returns [`StoreError`] if encoding, appending, or syncing the event fails.
    pub fn record(&self, team_key: &str, event: TeamStateCaptured) -> Result<(), StoreError> {
        self.inner.record(team_key, event)
    }

    /// Soft-delete a team's fiber (detach).
    ///
    /// # Errors
    /// Returns [`StoreError`] if encoding, detaching, or syncing the fiber fails.
    pub fn detach(&self, team_key: &str, event: TeamStateCaptured) -> Result<(), StoreError> {
        self.inner.detach(team_key, event).map(|_| ())
    }

    /// Fold every team event in committed line order without materialising an owned vector.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, &TeamStateCaptured),
    ) -> Result<R, StoreError> {
        self.inner.fold_defined_events(init, fold)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::event::team_domain_key;

    fn mismatched_descriptor() -> AdmittedDescriptor {
        AdmittedDescriptor::try_from_descriptor(SchemaDescriptor::new(999, DescriptorNode::U64))
            .unwrap()
    }

    #[test]
    fn unknown_latch_blocks_later_reads_writes_and_resync() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unknown.pgno");
        let store = NativeStore::create_pgno(&path).unwrap();
        let mut state = store.inner.lock_knowledge().unwrap();
        assert!(
            state
                .accept::<()>(WriteLandingVerdict::Undetermined { carried_epoch: 7 })
                .is_err()
        );
        drop(state);
        assert!(matches!(
            store.events(),
            Err(StoreError::Indeterminate { carried_epoch: 7 })
        ));
        assert!(store.record("repo", synthetic_domain_event(1)).is_err());
        assert!(store.detach("repo", synthetic_domain_event(2)).is_err());
        assert!(store.resync_pgno_from_authoritative(&path).is_err());
        assert!(
            NativeStore::open_pgno(&path)
                .unwrap()
                .events()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn detaching_absent_fiber_does_not_publish_a_phantom_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noop.pgno");
        let store = NativeStore::create_pgno(&path).unwrap();
        store.detach("absent", synthetic_domain_event(1)).unwrap();
        assert!(store.events().unwrap().is_empty());
        assert!(
            NativeStore::open_pgno(&path)
                .unwrap()
                .events()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn native_fault_append_rescue_detach_retains_unknown_without_second_write() {
        for action in ["append", "rescue", "detach"] {
            for fault in [0, 1, 2] {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("fault.pgno");
                let store = NativeStore::create_pgno(&path).unwrap();
                store.record("repo", synthetic_domain_event(1)).unwrap();
                if action == "rescue" {
                    store.detach("repo", synthetic_domain_event(2)).unwrap();
                }
                let before = store.inner.cached.lock().unwrap().len();
                *store.inner.session_fault.lock().unwrap() = Some(match fault {
                    0 => |session| session.with_simulate_indeterminate(true),
                    1 => |session| session.with_simulate_write_error(true),
                    _ => |session| session.with_simulate_sync_error(true),
                });
                let result = if action == "detach" {
                    store.detach("repo", synthetic_domain_event(3))
                } else {
                    store.record("repo", synthetic_domain_event(3))
                };
                assert!(
                    matches!(result, Err(StoreError::Indeterminate { .. })),
                    "{action}/{fault}: {result:?}"
                );
                assert_eq!(store.inner.cached.lock().unwrap().len(), before);
                let persisted = std::fs::read(&path).unwrap();
                assert!(matches!(
                    store.record("repo", synthetic_domain_event(4)),
                    Err(StoreError::Indeterminate { .. })
                ));
                assert_eq!(std::fs::read(&path).unwrap(), persisted);
                let reopened = NativeStore::open_pgno(&path).unwrap();
                assert_eq!(
                    reopened.events().unwrap().len(),
                    before + usize::from(fault == 2)
                );
            }
        }
    }

    #[test]
    fn already_detached_noop_preserves_cache_and_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noop.pgno");
        let store = NativeStore::create_pgno(&path).unwrap();
        store.record("repo", synthetic_domain_event(1)).unwrap();
        store.detach("repo", synthetic_domain_event(2)).unwrap();
        let before = store.events().unwrap();
        store.detach("repo", synthetic_domain_event(3)).unwrap();
        assert_eq!(store.events().unwrap(), before);
        assert_eq!(
            NativeStore::open_pgno(&path).unwrap().events().unwrap(),
            before
        );
    }

    #[test]
    fn creation_failure_after_admission_is_unknown_not_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked.pgno");
        let path = path.join("missing-parent.pgno");
        let Err(error) = NativeStore::create_pgno(&path) else {
            panic!("data path is a directory");
        };
        assert!(matches!(error, StoreError::CreationUnknown(_)));
        assert!(!FileStorageAdapter::new(&path).meta_path().exists());
    }

    fn invalidate_authority_before_sync(path: &Path) {
        let meta = FileStorageAdapter::new(path).meta_path().to_path_buf();
        std::fs::write(meta, b"invalid authority after landing").unwrap();
    }

    #[test]
    fn postland_sync_failure_preserves_landing_and_blocks_replay() {
        for detach in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("postland.pgno");
            let store = NativeStore::create_pgno(&path).unwrap();
            store.record("repo", synthetic_domain_event(1)).unwrap();
            let meta = FileStorageAdapter::new(&path).meta_path().to_path_buf();
            let metadata = std::fs::read(&meta).unwrap();
            BEFORE_FILE_SYNC.set(Some(Box::new(invalidate_authority_before_sync)));
            let result = if detach {
                store.detach("repo", synthetic_domain_event(2))
            } else {
                store.record("repo", synthetic_domain_event(2))
            };
            assert!(matches!(result, Err(StoreError::AfterLanded(_))));
            assert_eq!(store.inner.cached.lock().unwrap().len(), 1);
            let persisted = std::fs::read(&path).unwrap();
            assert!(matches!(
                store.record("repo", synthetic_domain_event(3)),
                Err(StoreError::LandedNeedsReconciliation)
            ));
            assert_eq!(std::fs::read(&path).unwrap(), persisted);
            std::fs::write(meta, metadata).unwrap();
            assert_eq!(
                NativeStore::open_pgno(&path)
                    .unwrap()
                    .events()
                    .unwrap()
                    .len(),
                2
            );
        }
    }

    #[test]
    fn successful_create_followed_by_sync_failure_is_after_landed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("created.pgno");
        BEFORE_FILE_SYNC.set(Some(Box::new(invalidate_authority_before_sync)));
        assert!(matches!(
            NativeStore::create_pgno(&path),
            Err(StoreError::AfterLanded(_))
        ));
        assert_eq!(
            std::fs::read(&path).unwrap(),
            ContainerHeader::new().to_bytes()
        );
        assert!(FileStorageAdapter::new(&path).meta_path().exists());
    }

    struct InvalidDescriptor;

    impl PardosaSchema for InvalidDescriptor {
        const SCHEMA_VERSION: u32 = 1;
        fn schema_descriptor() -> DescriptorNode {
            (0..17).fold(DescriptorNode::U64, |inner, _| DescriptorNode::Option {
                inner: Box::new(inner),
            })
        }
        fn encode_payload(&self, _: &mut Vec<u8>) -> Result<(), EncodeError> {
            unreachable!()
        }
        fn decode_payload(_: &[u8]) -> Result<Self, DecodeError> {
            unreachable!()
        }
    }

    #[test]
    fn invalid_descriptor_rejection_creates_no_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invalid.pgno");
        assert!(StoreInner::<InvalidDescriptor>::create_pgno(&path, "invalid").is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn create_preflight_preserves_existing_artifacts_without_uncertainty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.pgno");
        let store = NativeStore::create_pgno(&path).unwrap();
        store.record("repo", synthetic_domain_event(1)).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert!(matches!(
            NativeStore::create_pgno(&path),
            Err(StoreError::AlreadyExists(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), data);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancelled_caller_does_not_cancel_owned_blocking_write_or_replay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cancel.pgno");
        let store = std::sync::Arc::new(NativeStore::create_pgno(&path).unwrap());
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let writer = std::sync::Arc::clone(&store);
        let caller = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                BEFORE_FILE_SYNC.set(Some(Box::new(move |path| {
                    entered_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    invalidate_authority_before_sync(path);
                })));
                result_tx
                    .send(writer.record("repo", synthetic_domain_event(1)))
                    .unwrap();
            })
            .await
            .unwrap();
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
            .await
            .unwrap()
            .unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        release_tx.send(()).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), result_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(result, Err(StoreError::AfterLanded(_))));
        assert!(store.inner.cached.lock().unwrap().is_empty());
        let persisted = std::fs::read(&path).unwrap();
        assert!(persisted.len() > ContainerHeader::new().to_bytes().len());
        assert!(matches!(
            store.record("repo", synthetic_domain_event(2)),
            Err(StoreError::LandedNeedsReconciliation)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), persisted);
    }

    #[test]
    fn cache_poison_after_landing_retains_committed_event_and_stops_instance() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache-poison.pgno");
        let store = NativeStore::create_pgno(&path).unwrap();
        let _ = std::panic::catch_unwind(|| {
            let _cache = store.inner.cached.lock().unwrap();
            panic!("inject cache poison");
        });
        assert!(matches!(
            store.record("repo", synthetic_domain_event(1)),
            Err(StoreError::AfterLanded(_))
        ));
        assert!(matches!(
            store.record("repo", synthetic_domain_event(2)),
            Err(StoreError::LandedNeedsReconciliation)
        ));
        assert!(matches!(
            store.events(),
            Err(StoreError::LandedNeedsReconciliation)
        ));
        assert_eq!(
            NativeStore::open_pgno(&path)
                .unwrap()
                .events()
                .unwrap()
                .len(),
            1
        );
    }

    fn create_unadmitted(adapter: &FileStorageAdapter, claim: &OwnershipClaimRecord) {
        adapter.create_incomplete_meta_only(claim).unwrap();
        std::fs::write(adapter.pgno_path(), ContainerHeader::new().to_bytes()).unwrap();
    }

    pub(crate) fn synthetic_domain_event(i: u64) -> DomainEvent {
        let domain_key = format!("domain-{i}");
        let repo_name = format!("repo-{i}");
        DomainEvent::RepositoryStateCaptured {
            domain_key: NonEmptyEventString::new(&domain_key).expect("domain key fits"),
            repo_name: NonEmptyEventString::new(&repo_name).expect("repo name fits"),
            timestamp: Timestamp::new(i + 1).expect("timestamp fits"),
            evidence: None,
        }
    }

    #[test]
    fn typed_concurrency_conflict_maps_to_conflict_variant_without_inventing_sequences() {
        let failure = OperationFailure::new(
            FailureCondition::ConcurrencyConflict,
            "two-writer concurrency collision: expected sequence mismatch",
        );

        match StoreError::from(failure) {
            StoreError::ConcurrencyConflict {
                expected_seq,
                actual_seq,
                source,
            } => {
                assert_eq!(
                    expected_seq, None,
                    "no typed expected sequence is available"
                );
                assert_eq!(actual_seq, None, "no typed actual sequence is available");
                assert!(
                    source
                        .downcast_ref::<OperationFailure>()
                        .is_some_and(OperationFailure::is_concurrency_conflict),
                    "the typed pardosa failure must survive as the error source"
                );
            }
            other => panic!("expected ConcurrencyConflict, got {other:?}"),
        }
    }

    #[test]
    fn already_exists_condition_still_maps_to_already_exists() {
        let failure = OperationFailure::new(FailureCondition::StoreAlreadyExists, "already there");

        assert!(StoreError::from(failure).is_already_exists());
    }

    #[test]
    fn ordinary_infrastructure_condition_still_maps_to_infrastructure() {
        let failure =
            OperationFailure::new(FailureCondition::TransportUnavailable, "connection refused");

        assert!(
            matches!(StoreError::from(failure), StoreError::Operation(_)),
            "transport failures preserve their typed condition"
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

        long_lived.release_exclusion_for_test();

        {
            let other_writer = NativeStore::open_pgno(&path).expect("second handle opens");
            other_writer
                .record("domain-1", synthetic_domain_event(1))
                .expect("record via second handle");
        }

        assert_eq!(
            long_lived.events().expect("events before resync").len(),
            1,
            "long-lived handle must not see the externally-durable write before resync"
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

    pub(crate) fn synthetic_team_state(org: &str, team_slug: &str) -> TeamStateCaptured {
        use crate::event::{
            OrgMembershipFetchStatus, OrphanAttributionInputs, TeamRosterStatusEvent,
        };

        TeamStateCaptured {
            org: NonEmptyEventString::new(org).expect("org fits"),
            team_slug: NonEmptyEventString::new(team_slug).expect("team_slug fits"),
            members: EventVec::new(Vec::new()).expect("empty members fits"),
            orphan_attribution_inputs: OrphanAttributionInputs {
                org_membership_fetch_status: OrgMembershipFetchStatus::Fetched,
            },
            fetched_at: EventString::new("2026-07-16T00:00:00Z".to_string())
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

    #[test]
    fn open_pgno_rejects_unadmitted_store_without_schema_descriptor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("unadmitted.pgno");
        let adapter = FileStorageAdapter::new(&path);
        let claim = default_claim(1, "unadmitted");
        create_unadmitted(&adapter, &claim);

        let Err(err) = NativeStore::open_pgno(&path) else {
            panic!("opening unadmitted store must fail closed");
        };
        assert!(matches!(
            err,
            StoreError::Infrastructure(ref msg)
                if msg == "missing schema descriptor in store metadata: refusing to open unadmitted store"
        ));
    }

    #[test]
    fn open_pgno_rejects_mismatched_schema_descriptor_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("mismatched.pgno");
        let adapter = FileStorageAdapter::new(&path);
        let claim = default_claim(1, "mismatched");
        let mut session = adapter
            .create(&claim, &mismatched_descriptor())
            .expect("create mismatched store");
        session.sync().expect("sync mismatched descriptor");
        drop(session);

        let Err(err) = NativeStore::open_pgno(&path) else {
            panic!("opening mismatched store must fail closed");
        };
        assert!(matches!(
            err,
            StoreError::Infrastructure(ref msg)
                if msg.contains("schema descriptor identity mismatch: expected ")
        ));
    }

    #[test]
    fn resync_pgno_rejects_unadmitted_or_mismatched_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid_path = dir.path().join("valid.pgno");
        let unadmitted_path = dir.path().join("unadmitted.pgno");
        let mismatched_path = dir.path().join("mismatched.pgno");

        let store = NativeStore::create_pgno(&valid_path).expect("create valid store");
        store
            .record(
                "repo-1",
                DomainEvent::RepositoryDeleted {
                    domain_key: nes("repo-1"),
                    repo_name: nes("repo-1"),
                    detected_at: ts(10),
                },
            )
            .expect("record initial event");
        assert_eq!(store.events().expect("events").len(), 1);

        let adapter_unadmitted = FileStorageAdapter::new(&unadmitted_path);
        let claim = default_claim(1, "unadmitted");
        create_unadmitted(&adapter_unadmitted, &claim);

        let err_unadmitted = store
            .resync_pgno_from_authoritative(&unadmitted_path)
            .expect_err("resync against unadmitted store must fail closed");
        assert!(matches!(
            err_unadmitted,
            StoreError::Infrastructure(ref msg)
                if msg == "missing schema descriptor in store metadata: refusing to open unadmitted store"
        ));
        assert_eq!(
            store.events().expect("events").len(),
            1,
            "cache must be preserved on unadmitted store error"
        );

        let adapter_mismatched = FileStorageAdapter::new(&mismatched_path);
        let claim_mismatched = default_claim(1, "mismatched");
        let mut session_mismatched = adapter_mismatched
            .create(&claim_mismatched, &mismatched_descriptor())
            .expect("create mismatched store");
        session_mismatched
            .sync()
            .expect("sync mismatched descriptor");
        drop(session_mismatched);

        let err_mismatched = store
            .resync_pgno_from_authoritative(&mismatched_path)
            .expect_err("resync against mismatched store must fail closed");
        assert!(matches!(
            err_mismatched,
            StoreError::Infrastructure(ref msg)
                if msg.contains("schema descriptor identity mismatch: expected ")
        ));
        assert_eq!(
            store.events().expect("events").len(),
            1,
            "cache must be preserved on mismatched identity error"
        );
    }

    fn remove_artefact_at(target: &Path) {
        let parent = target.parent().expect("target has a parent directory");
        let stem = format!(
            "{}.",
            target
                .file_stem()
                .expect("target has a file stem")
                .to_string_lossy()
        );
        for entry in std::fs::read_dir(parent).expect("read events dir") {
            let entry = entry.expect("dir entry");
            if entry.file_name().to_string_lossy().starts_with(&stem) {
                std::fs::remove_file(entry.path()).expect("remove artefact file");
            }
        }
    }

    pub(crate) fn overwrite_with_mismatched_descriptor_store(target: &Path) {
        remove_artefact_at(target);
        let adapter = FileStorageAdapter::new(target);
        let claim = default_claim(1, "mismatched");
        let mut session = adapter
            .create(&claim, &mismatched_descriptor())
            .expect("create mismatched store");
        session.sync().expect("sync mismatched descriptor");
    }

    pub(crate) fn overwrite_with_unadmitted_store(target: &Path) {
        remove_artefact_at(target);
        let adapter = FileStorageAdapter::new(target);
        let claim = default_claim(1, "unadmitted");
        create_unadmitted(&adapter, &claim);
    }

    #[test]
    fn resync_from_authoritative_preserves_its_own_cache_when_its_backend_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("owned.pgno");

        let store = NativeStore::create_pgno(&path).expect("create valid store");
        store
            .record(
                "repo-owned-1",
                DomainEvent::RepositoryDeleted {
                    domain_key: nes("repo-owned-1"),
                    repo_name: nes("repo-owned-1"),
                    detected_at: ts(10),
                },
            )
            .expect("seed a good cache before any failing refresh");
        assert_eq!(store.events().expect("events").len(), 1);

        overwrite_with_unadmitted_store(&path);
        let err_unadmitted = store
            .resync_from_authoritative()
            .expect_err("backend-owned refresh of an unadmitted store must fail closed");
        assert!(matches!(
            err_unadmitted,
            StoreError::Infrastructure(ref msg)
                if msg == "missing schema descriptor in store metadata: refusing to open unadmitted store"
        ));
        assert_eq!(
            store.events().expect("events").len(),
            1,
            "a failed backend-owned refresh must not replace this store's cache"
        );

        overwrite_with_mismatched_descriptor_store(&path);
        let err_mismatched = store
            .resync_from_authoritative()
            .expect_err("backend-owned refresh of a mismatched store must fail closed");
        assert!(matches!(
            err_mismatched,
            StoreError::Infrastructure(ref msg)
                if msg.contains("schema descriptor identity mismatch: expected ")
        ));
        assert_eq!(
            store.events().expect("events").len(),
            1,
            "a failed backend-owned refresh must not replace this store's cache"
        );
    }

    #[test]
    fn nats_resync_from_authoritative_preserves_cache_when_its_backend_is_unreachable() {
        let Some(server) = TestNatsServer::spawn() else {
            return;
        };

        let rt = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let client = rt
            .block_on(async_nats::connect(&server.url))
            .expect("connect to live nats");
        let stem = format!("test_nats_unreachable_{}", uuid::Uuid::now_v7());
        let adapter =
            NatsStorageAdapter::from_client_with_runtime(client.clone(), stem, rt.clone());
        let store = NativeStore::create_nats(adapter).expect("create nats store");
        store
            .record(
                "repo-nats-cached",
                DomainEvent::RepositoryDeleted {
                    domain_key: nes("repo-nats-cached"),
                    repo_name: nes("repo-nats-cached"),
                    detected_at: ts(40),
                },
            )
            .expect("seed a good cache while the backend is reachable");
        assert_eq!(store.events().expect("events").len(), 1);

        drop(server);

        let err = store
            .resync_from_authoritative()
            .expect_err("backend-owned refresh must fail when its own NATS backend is gone");
        assert!(
            !matches!(err, StoreError::Poisoned),
            "an unreachable backend must surface as a backend error, got: {err:?}"
        );
        assert_eq!(
            store.events().expect("events").len(),
            1,
            "a failed NATS refresh must not clear or replace this store's cache"
        );
    }

    pub(crate) fn overwrite_with_valid_schema_but_undecodable_payload(target: &Path) {
        remove_artefact_at(target);
        let adapter = FileStorageAdapter::new(target);
        let claim = default_claim(1, "decode-failure");
        let desc = SchemaDescriptor::new(
            DomainEvent::SCHEMA_VERSION,
            DomainEvent::schema_descriptor(),
        );
        let desc = AdmittedDescriptor::try_from_descriptor(desc).unwrap();
        let mut session = adapter.create(&claim, &desc).expect("create store");
        session.sync().expect("sync descriptor");
        drop(session);

        let mut decodable = Vec::new();
        DomainEvent::RepositoryDeleted {
            domain_key: nes("repo-decodes-fine"),
            repo_name: nes("repo-decodes-fine"),
            detected_at: ts(70),
        }
        .encode_payload(&mut decodable)
        .expect("encode a genuinely valid payload");

        let epoch = adapter.current_epoch().expect("current epoch");
        let mut session = adapter.open_write(epoch).expect("open write");
        let good_fiber = derive_fiber_id("repo-decodes-fine");
        session.fiber(good_fiber).expect("good fiber");
        session
            .append_to_fiber(good_fiber, *uuid::Uuid::now_v7().as_bytes(), decodable)
            .expect("append the decodable event first");
        let bad_fiber = derive_fiber_id("repo-payload-is-garbage");
        session.fiber(bad_fiber).expect("bad fiber");
        session
            .append_to_fiber(
                bad_fiber,
                *uuid::Uuid::now_v7().as_bytes(),
                vec![0xFF_u8; 16],
            )
            .expect("append an undecodable payload after it");
        session.sync().expect("sync appended envelopes");
    }

    #[test]
    fn resync_from_authoritative_preserves_cache_when_a_payload_fails_to_decode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("decode.pgno");

        let store = NativeStore::create_pgno(&path).expect("create valid store");
        store
            .record(
                "repo-decode-seed",
                DomainEvent::RepositoryDeleted {
                    domain_key: nes("repo-decode-seed"),
                    repo_name: nes("repo-decode-seed"),
                    detected_at: ts(60),
                },
            )
            .expect("seed a known good cache before the failing refresh");
        let seeded = store.events().expect("events");
        assert_eq!(seeded.len(), 1);

        overwrite_with_valid_schema_but_undecodable_payload(&path);

        let err = store
            .resync_from_authoritative()
            .expect_err("a payload that cannot be decoded must fail the refresh closed");
        assert!(
            !matches!(err, StoreError::Poisoned),
            "the failure must come from decoding, not from a poisoned lock: {err:?}"
        );
        let message = err.to_string();
        assert!(
            !message.contains("schema descriptor"),
            "this test must reach the payload decode loop, NOT stop at descriptor \
             verification — descriptor rejection is already covered elsewhere and is not \
             decode coverage: {message}"
        );
        assert!(
            message.contains("UnknownVariantDiscriminant"),
            "the failure must be the undecodable payload itself: {message}"
        );

        let preserved = store.events().expect("events after the failed refresh");
        assert_eq!(
            preserved.len(),
            1,
            "a decode failure must not publish a partial prefix, and must not clear the cache"
        );
        match &preserved[0].1 {
            DomainEvent::RepositoryDeleted {
                domain_key,
                detected_at,
                ..
            } => {
                assert_eq!(
                    domain_key.as_str(),
                    "repo-decode-seed",
                    "the previous cache CONTENTS must survive unchanged; the decodable event \
                     from the failing read must never be published"
                );
                assert_eq!(detected_at.as_nanos(), 60);
            }
            other => panic!("unexpected preserved event: {other:?}"),
        }
    }

    fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
        NonEmptyEventString::new(s.to_string()).expect("valid non-empty string")
    }

    fn ts(nanos: u64) -> Timestamp {
        Timestamp::new(nanos).expect("valid timestamp")
    }

    pub(crate) const PINNED_NATS_VERSION: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tools/.nats-server-version"
    ));

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum NatsVersionError {
        VersionMismatch { expected: String, actual: String },
        UnsuccessfulStatus(String),
        InvalidUtf8(String),
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(crate) enum ProbeOutcome {
        Ready(std::path::PathBuf),
        Unavailable(String),
        Fatal(String),
    }

    pub(crate) fn parse_nats_version(
        status_success: bool,
        stdout_bytes: &[u8],
        expected_pin: &str,
    ) -> Result<String, NatsVersionError> {
        if !status_success {
            return Err(NatsVersionError::UnsuccessfulStatus(
                "probe process exited with non-zero status".to_string(),
            ));
        }
        let stdout = std::str::from_utf8(stdout_bytes).map_err(|e| {
            NatsVersionError::InvalidUtf8(format!("probe output is not valid UTF-8: {e}"))
        })?;
        let trimmed = stdout.trim();
        let version = trimmed
            .strip_prefix("nats-server: v")
            .or_else(|| trimmed.strip_prefix("nats-server: "))
            .or_else(|| trimmed.strip_prefix('v'))
            .unwrap_or(trimmed);
        if version == expected_pin {
            Ok(version.to_string())
        } else {
            Err(NatsVersionError::VersionMismatch {
                expected: expected_pin.to_string(),
                actual: version.to_string(),
            })
        }
    }

    pub(crate) fn classify_probe_result(
        result: Result<std::process::Output, std::io::Error>,
        expected_pin: &str,
        bin_path: std::path::PathBuf,
    ) -> ProbeOutcome {
        match result {
            Ok(output) => {
                match parse_nats_version(output.status.success(), &output.stdout, expected_pin) {
                    Ok(_) => ProbeOutcome::Ready(bin_path),
                    Err(NatsVersionError::VersionMismatch { expected, actual }) => {
                        ProbeOutcome::Unavailable(format!(
                            "version mismatch (expected {expected}, got {actual})"
                        ))
                    }
                    Err(
                        NatsVersionError::UnsuccessfulStatus(msg)
                        | NatsVersionError::InvalidUtf8(msg),
                    ) => ProbeOutcome::Fatal(msg),
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                ProbeOutcome::Unavailable(format!("executable absent: {err}"))
            }
            Err(err) => ProbeOutcome::Fatal(format!(
                "failed to execute probe for {}: {err}",
                bin_path.display()
            )),
        }
    }

    fn resolve_pinned_nats_server() -> Result<std::path::PathBuf, String> {
        let expected_pin = PINNED_NATS_VERSION.trim();
        let candidate_path = std::path::PathBuf::from("../../tools/bin/nats-server");
        let alt_candidate = std::path::PathBuf::from("tools/bin/nats-server");
        let bin_path = if candidate_path.is_file() {
            candidate_path
        } else if alt_candidate.is_file() {
            alt_candidate
        } else {
            std::path::PathBuf::from("nats-server")
        };
        let output = std::process::Command::new(&bin_path)
            .arg("--version")
            .output();
        match classify_probe_result(output, expected_pin, bin_path) {
            ProbeOutcome::Ready(path) => Ok(path),
            ProbeOutcome::Unavailable(reason) => Err(reason),
            ProbeOutcome::Fatal(fatal) => panic!("fatal nats-server probe failure: {fatal}"),
        }
    }

    fn cleanup_child_process(child: &mut std::process::Child) {
        match child.try_wait() {
            Ok(Some(_status)) => {}
            Ok(None) => {
                if let Err(err) = child.kill() {
                    eprintln!("warn: failed to kill test nats-server: {err}");
                    return;
                }
                if let Err(err) = child.wait() {
                    eprintln!("warn: failed to wait on test nats-server: {err}");
                }
            }
            Err(err) => {
                eprintln!("warn: failed to query status of test nats-server: {err}");
                if let Err(kill_err) = child.kill() {
                    eprintln!(
                        "warn: failed to kill test nats-server after query error: {kill_err}"
                    );
                    return;
                }
                if let Err(wait_err) = child.wait() {
                    eprintln!(
                        "warn: failed to wait on test nats-server after query error: {wait_err}"
                    );
                }
            }
        }
    }

    pub(crate) struct TestNatsServer {
        pub(crate) url: String,
        child: std::process::Child,
        _tempdir: tempfile::TempDir,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum TestPortChoice {
        Ephemeral,
        LoopbackSentinelPrefix,
    }

    const SENTINEL_PREFIX_FIRST_PORT: u16 = 10000;
    const SENTINEL_PREFIX_LAST_PORT: u16 = 19999;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum PortSelectionFault {
        Exhausted { first: u16, last: u16 },
        BindFailed { port: u16, kind: std::io::ErrorKind },
    }

    fn select_sentinel_prefixed_port<F>(mut bind: F) -> Result<u16, PortSelectionFault>
    where
        F: FnMut(u16) -> std::io::Result<()>,
    {
        for port in SENTINEL_PREFIX_FIRST_PORT..=SENTINEL_PREFIX_LAST_PORT {
            match bind(port) {
                Ok(()) => return Ok(port),
                Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {}
                Err(err) => {
                    return Err(PortSelectionFault::BindFailed {
                        port,
                        kind: err.kind(),
                    });
                }
            }
        }
        Err(PortSelectionFault::Exhausted {
            first: SENTINEL_PREFIX_FIRST_PORT,
            last: SENTINEL_PREFIX_LAST_PORT,
        })
    }

    fn reserve_test_port(choice: TestPortChoice) -> u16 {
        match choice {
            TestPortChoice::Ephemeral => {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test port");
                let port = listener.local_addr().expect("local addr").port();
                drop(listener);
                port
            }
            TestPortChoice::LoopbackSentinelPrefix => {
                match select_sentinel_prefixed_port(|port| {
                    std::net::TcpListener::bind(("127.0.0.1", port)).map(drop)
                }) {
                    Ok(port) => port,
                    Err(fault) => {
                        panic!("fatal harness failure: sentinel-prefixed port selection: {fault:?}")
                    }
                }
            }
        }
    }

    #[test]
    fn sentinel_port_selection_exhaustion_is_fatal_not_skip() {
        let fault = select_sentinel_prefixed_port(|_| {
            Err(std::io::Error::from(std::io::ErrorKind::AddrInUse))
        })
        .expect_err("all candidates occupied must not yield a port");

        assert_eq!(
            fault,
            PortSelectionFault::Exhausted {
                first: SENTINEL_PREFIX_FIRST_PORT,
                last: SENTINEL_PREFIX_LAST_PORT,
            }
        );
    }

    #[test]
    fn sentinel_port_selection_non_addr_in_use_bind_failure_is_fatal() {
        let fault = select_sentinel_prefixed_port(|port| {
            if port == SENTINEL_PREFIX_FIRST_PORT {
                Err(std::io::Error::from(std::io::ErrorKind::AddrInUse))
            } else {
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            }
        })
        .expect_err("a non-AddrInUse bind failure must not be treated as an occupied port");

        assert_eq!(
            fault,
            PortSelectionFault::BindFailed {
                port: SENTINEL_PREFIX_FIRST_PORT + 1,
                kind: std::io::ErrorKind::PermissionDenied,
            }
        );
    }

    #[test]
    fn sentinel_port_selection_skips_occupied_candidates_until_one_binds() {
        let port = select_sentinel_prefixed_port(|port| {
            if port < SENTINEL_PREFIX_FIRST_PORT + 3 {
                Err(std::io::Error::from(std::io::ErrorKind::AddrInUse))
            } else {
                Ok(())
            }
        })
        .expect("a bindable candidate must be selected");

        assert_eq!(port, SENTINEL_PREFIX_FIRST_PORT + 3);
    }

    impl TestNatsServer {
        fn spawn_internal(custom_config: Option<&str>) -> Option<Self> {
            Self::spawn_internal_on(custom_config, TestPortChoice::Ephemeral)
        }

        fn spawn_internal_on(custom_config: Option<&str>, choice: TestPortChoice) -> Option<Self> {
            let bin_path = match resolve_pinned_nats_server() {
                Ok(path) => path,
                Err(reason) => {
                    eprintln!("SKIP: live nats-server unavailable: {reason}");
                    return None;
                }
            };

            let port = reserve_test_port(choice);

            let tempdir = tempfile::TempDir::new().expect("tempdir");
            let mut cmd = std::process::Command::new(bin_path);
            cmd.arg("-a")
                .arg("127.0.0.1")
                .arg("-p")
                .arg(port.to_string())
                .arg("-js")
                .arg("-sd")
                .arg(tempdir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());

            if let Some(conf) = custom_config {
                let config_path = tempdir.path().join("nats.conf");
                std::fs::write(&config_path, conf).expect("write nats.conf");
                cmd.arg("-c").arg(&config_path);
            }

            let mut child = cmd.spawn().expect("spawn nats-server");

            let url = format!("nats://127.0.0.1:{port}");
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_secs(5) {
                if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
                    return Some(Self {
                        url,
                        child,
                        _tempdir: tempdir,
                    });
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            cleanup_child_process(&mut child);
            panic!("nats-server readiness timeout on 127.0.0.1:{port}");
        }

        pub(crate) fn spawn() -> Option<Self> {
            Self::spawn_internal(None)
        }

        pub(crate) fn spawn_on_sentinel_prefixed_port() -> Option<Self> {
            Self::spawn_internal_on(None, TestPortChoice::LoopbackSentinelPrefix)
        }

        pub(crate) fn spawn_with_auth_config(config_content: &str) -> Option<Self> {
            Self::spawn_internal(Some(config_content))
        }
    }

    impl Drop for TestNatsServer {
        fn drop(&mut self) {
            cleanup_child_process(&mut self.child);
        }
    }

    #[test]
    fn nats_store_admission_roundtrip_persists_across_reopen() {
        let Some(server) = TestNatsServer::spawn() else {
            return;
        };

        let rt = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let client = rt
            .block_on(async_nats::connect(&server.url))
            .expect("connect to live nats");

        let stem_valid = format!("test_nats_valid_{}", uuid::Uuid::now_v7());
        let adapter_valid = NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            stem_valid.clone(),
            rt.clone(),
        );
        let store = NativeStore::create_nats(adapter_valid).expect("create valid nats store");
        store
            .record(
                "repo-nats-1",
                DomainEvent::RepositoryDeleted {
                    domain_key: nes("repo-nats-1"),
                    repo_name: nes("repo-nats-1"),
                    detected_at: ts(20),
                },
            )
            .expect("record event in nats");
        drop(store);

        let adapter_reopen = NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            stem_valid.clone(),
            rt.clone(),
        );
        let reopened = NativeStore::open_nats(adapter_reopen).expect("reopen valid nats store");
        assert_eq!(reopened.events().expect("events").len(), 1);
        reopened
            .record(
                "repo-nats-2",
                DomainEvent::RepositoryDeleted {
                    domain_key: nes("repo-nats-2"),
                    repo_name: nes("repo-nats-2"),
                    detected_at: ts(30),
                },
            )
            .expect("append second event after reopen");
        drop(reopened);

        let adapter_second_reopen =
            NatsStorageAdapter::from_client_with_runtime(client.clone(), stem_valid, rt.clone());
        let second_reopened =
            NativeStore::open_nats(adapter_second_reopen).expect("second reopen valid nats store");
        let events = second_reopened.events().expect("events");
        assert_eq!(events.len(), 2);
        match &events[0].1 {
            DomainEvent::RepositoryDeleted {
                domain_key,
                repo_name,
                detected_at,
            } => {
                assert_eq!(domain_key.as_str(), "repo-nats-1");
                assert_eq!(repo_name.as_str(), "repo-nats-1");
                assert_eq!(detected_at.as_nanos(), 20);
            }
            _ => panic!("unexpected event 0 variant"),
        }
        match &events[1].1 {
            DomainEvent::RepositoryDeleted {
                domain_key,
                repo_name,
                detected_at,
            } => {
                assert_eq!(domain_key.as_str(), "repo-nats-2");
                assert_eq!(repo_name.as_str(), "repo-nats-2");
                assert_eq!(detected_at.as_nanos(), 30);
            }
            _ => panic!("unexpected event 1 variant"),
        }
    }

    #[test]
    fn nats_store_open_rejects_unadmitted_or_mismatched_descriptor() {
        let Some(server) = TestNatsServer::spawn() else {
            return;
        };

        let rt = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
        );
        let client = rt
            .block_on(async_nats::connect(&server.url))
            .expect("connect to live nats");

        let stem_mismatched = format!("test_nats_mismatched_{}", uuid::Uuid::now_v7());
        let adapter_mismatched_raw = NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            stem_mismatched.clone(),
            rt.clone(),
        );
        let claim = default_claim(1, "mismatched-nats");
        let mut session = adapter_mismatched_raw
            .create(&claim, &mismatched_descriptor())
            .expect("create bare nats store");
        session.sync().expect("sync mismatched nats descriptor");
        drop(session);

        let adapter_mismatched = NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            stem_mismatched,
            rt.clone(),
        );
        let Err(err_mismatched) = NativeStore::open_nats(adapter_mismatched) else {
            panic!("open mismatched nats store must fail closed");
        };
        assert!(matches!(
            err_mismatched,
            StoreError::Infrastructure(ref msg)
                if msg.contains("schema descriptor identity mismatch: expected ")
        ));

        let stem_unadmitted = format!("test_nats_unadmitted_{}", uuid::Uuid::now_v7());
        let adapter_unadmitted_raw = NatsStorageAdapter::from_client_with_runtime(
            client.clone(),
            stem_unadmitted.clone(),
            rt.clone(),
        );
        let claim_unadmitted = default_claim(1, "unadmitted-nats");
        adapter_unadmitted_raw
            .create_incomplete_meta_only(&claim_unadmitted)
            .expect("create unadmitted nats store");

        let adapter_unadmitted =
            NatsStorageAdapter::from_client_with_runtime(client, stem_unadmitted, rt);
        let Err(err_unadmitted) = NativeStore::open_nats(adapter_unadmitted) else {
            panic!("open unadmitted nats store must fail closed");
        };
        assert!(matches!(
            err_unadmitted,
            StoreError::Infrastructure(ref msg)
                if msg == "missing schema descriptor in store metadata: refusing to open unadmitted store"
        ));
    }

    #[test]
    fn nats_store_drops_safely_when_root_owner_outlives_application_runtime() {
        let Some(server) = TestNatsServer::spawn() else {
            return;
        };
        let url = server.url.clone();

        let nats_runtime = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("root nats runtime"),
        );
        let root_nats_guard = std::sync::Arc::clone(&nats_runtime);

        let client = nats_runtime
            .block_on(async_nats::connect(&url))
            .expect("connect");
        let stem = format!("test_root_owner_{}", uuid::Uuid::now_v7());
        let adapter = NatsStorageAdapter::from_client_with_runtime(client, stem, root_nats_guard);

        let app_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("app runtime");

        app_runtime.block_on(async move {
            assert_eq!(
                tokio::runtime::Handle::current().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            );
            let store = NativeStore::create_nats(adapter).expect("create test nats store");
            drop(store);
        });

        drop(app_runtime);
        drop(nats_runtime);
    }

    #[test]
    fn nats_store_early_error_drops_safely_when_root_owner_outlives_application_runtime() {
        let Some(server) = TestNatsServer::spawn() else {
            return;
        };
        let url = server.url.clone();

        let nats_runtime = std::sync::Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("root nats runtime"),
        );
        let root_nats_guard = std::sync::Arc::clone(&nats_runtime);

        let client = nats_runtime
            .block_on(async_nats::connect(&url))
            .expect("connect");
        let stem = format!("test_early_error_{}", uuid::Uuid::now_v7());
        let adapter = NatsStorageAdapter::from_client_with_runtime(client, stem, root_nats_guard);

        let app_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("app runtime");

        app_runtime.block_on(async move {
            assert_eq!(
                tokio::runtime::Handle::current().runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            );
            let res = NativeStore::open_nats(adapter);
            assert!(res.is_err());
        });

        drop(app_runtime);
        drop(nats_runtime);
    }

    #[test]
    fn parse_nats_version_extracts_canonical_pin() {
        assert_eq!(
            parse_nats_version(true, b"nats-server: v2.14.5\n", "2.14.5").unwrap(),
            "2.14.5"
        );
        assert_eq!(
            parse_nats_version(true, b"v2.14.5\n", "2.14.5").unwrap(),
            "2.14.5"
        );
        assert_eq!(
            parse_nats_version(true, b"2.14.5\n", "2.14.5").unwrap(),
            "2.14.5"
        );
    }

    #[test]
    fn parse_nats_version_rejects_version_mismatches() {
        let err = parse_nats_version(true, b"nats-server: v2.14.6\n", "2.14.5").unwrap_err();
        assert_eq!(
            err,
            NatsVersionError::VersionMismatch {
                expected: "2.14.5".to_string(),
                actual: "2.14.6".to_string(),
            }
        );

        let err_near = parse_nats_version(true, b"nats-server: v2.14.50\n", "2.14.5").unwrap_err();
        assert_eq!(
            err_near,
            NatsVersionError::VersionMismatch {
                expected: "2.14.5".to_string(),
                actual: "2.14.50".to_string(),
            }
        );
    }

    #[test]
    fn parse_nats_version_rejects_nonzero_status_or_invalid_utf8() {
        let err_status =
            parse_nats_version(false, b"nats-server: v2.14.5\n", "2.14.5").unwrap_err();
        assert_eq!(
            err_status,
            NatsVersionError::UnsuccessfulStatus(
                "probe process exited with non-zero status".to_string()
            )
        );

        let err_utf8 = parse_nats_version(true, b"\xFF\xFE\xFD", "2.14.5").unwrap_err();
        assert!(matches!(err_utf8, NatsVersionError::InvalidUtf8(_)));
    }

    #[cfg(unix)]
    fn make_test_output(status_code: i32, stdout: &[u8]) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(status_code << 8),
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn classify_probe_result_covers_concrete_output_variants() {
        let dummy = std::path::PathBuf::from("nats-server");

        let out_exact = make_test_output(0, b"nats-server: v2.14.5\n");
        assert_eq!(
            classify_probe_result(Ok(out_exact), "2.14.5", dummy.clone()),
            ProbeOutcome::Ready(dummy.clone())
        );

        let out_mismatch = make_test_output(0, b"nats-server: v2.14.6\n");
        assert!(matches!(
            classify_probe_result(Ok(out_mismatch), "2.14.5", dummy.clone()),
            ProbeOutcome::Unavailable(msg) if msg.contains("expected 2.14.5, got 2.14.6")
        ));

        let out_near = make_test_output(0, b"nats-server: v2.14.50\n");
        assert!(matches!(
            classify_probe_result(Ok(out_near), "2.14.5", dummy.clone()),
            ProbeOutcome::Unavailable(msg) if msg.contains("expected 2.14.5, got 2.14.50")
        ));

        let out_fail_status = make_test_output(1, b"nats-server: v2.14.5\n");
        assert!(matches!(
            classify_probe_result(Ok(out_fail_status), "2.14.5", dummy.clone()),
            ProbeOutcome::Fatal(msg) if msg.contains("non-zero status")
        ));

        let out_invalid_utf8 = make_test_output(0, b"\xFF\xFE\xFD");
        assert!(matches!(
            classify_probe_result(Ok(out_invalid_utf8), "2.14.5", dummy.clone()),
            ProbeOutcome::Fatal(msg) if msg.contains("valid UTF-8")
        ));
    }

    #[test]
    fn classify_probe_result_distinguishes_unavailable_from_fatal() {
        let dummy_path = std::path::PathBuf::from("nats-server");

        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
        let outcome_absent = classify_probe_result(Err(not_found), "2.14.5", dummy_path.clone());
        assert!(matches!(outcome_absent, ProbeOutcome::Unavailable(_)));

        let perm_denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let outcome_fatal = classify_probe_result(Err(perm_denied), "2.14.5", dummy_path.clone());
        assert!(matches!(outcome_fatal, ProbeOutcome::Fatal(_)));
    }
}
