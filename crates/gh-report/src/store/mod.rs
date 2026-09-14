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
    AlreadyExists(String),
    #[error("pardosa infrastructure error: {0}")]
    Infrastructure(String),
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
        if err.is_already_exists() {
            StoreError::AlreadyExists(err.to_string())
        } else {
            StoreError::Infrastructure(err.to_string())
        }
    }
}

impl From<EncodeError> for StoreError {
    fn from(err: EncodeError) -> Self {
        StoreError::Infrastructure(err.to_string())
    }
}

impl From<DecodeError> for StoreError {
    fn from(err: DecodeError) -> Self {
        StoreError::Infrastructure(err.to_string())
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

pub(crate) struct NatsAdapterHandle(Option<Box<NatsStorageAdapter>>);

impl NatsAdapterHandle {
    pub(crate) fn new(adapter: NatsStorageAdapter) -> Self {
        Self(Some(Box::new(adapter)))
    }
}

impl Clone for NatsAdapterHandle {
    fn clone(&self) -> Self {
        Self(self.0.as_ref().map(|a| Box::new((**a).clone())))
    }
}

impl std::ops::Deref for NatsAdapterHandle {
    type Target = NatsStorageAdapter;
    fn deref(&self) -> &Self::Target {
        self.0
            .as_deref()
            .expect("adapter present during store lifetime")
    }
}

impl std::ops::DerefMut for NatsAdapterHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0
            .as_deref_mut()
            .expect("adapter present during store lifetime")
    }
}

impl Drop for NatsAdapterHandle {
    fn drop(&mut self) {
        if let Some(adapter) = self.0.take() {
            if tokio::runtime::Handle::try_current().is_ok() {
                let _ = std::thread::Builder::new()
                    .name("nats-adapter-drop".to_string())
                    .spawn(move || {
                        drop(adapter);
                    });
            } else {
                drop(adapter);
            }
        }
    }
}

#[derive(Clone)]
enum StorageBackend {
    File(PathBuf),
    Nats(NatsAdapterHandle),
}

struct StoreInner<E> {
    backend: StorageBackend,
    cached: Mutex<Vec<(bool, [u8; 16], E)>>,
}

fn verify_schema_descriptor<E: PardosaSchema>(
    actual: Option<&SchemaDescriptor>,
) -> Result<(), StoreError> {
    let expected = SchemaDescriptor::new(E::schema_version(), E::schema_descriptor());
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
    fn create_pgno(path: &Path, label: &'static str) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let claim = default_claim(1, label);
        let mut session = adapter.create(&claim)?;
        let desc = SchemaDescriptor::new(E::schema_version(), E::schema_descriptor());
        session.set_schema_descriptor(&desc)?;
        session.sync()?;
        Ok(Self {
            backend: StorageBackend::File(path.to_path_buf()),
            cached: Mutex::new(Vec::new()),
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
            cached: Mutex::new(cached),
        })
    }

    fn create_nats(adapter: NatsStorageAdapter, label: &'static str) -> Result<Self, StoreError> {
        let claim = default_claim(1, label);
        let mut session = adapter.create(&claim)?;
        let desc = SchemaDescriptor::new(E::schema_version(), E::schema_descriptor());
        session.set_schema_descriptor(&desc)?;
        session.sync()?;
        Ok(Self {
            backend: StorageBackend::Nats(NatsAdapterHandle::new(adapter)),
            cached: Mutex::new(Vec::new()),
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
            backend: StorageBackend::Nats(NatsAdapterHandle::new(adapter)),
            cached: Mutex::new(cached),
        })
    }

    fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
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

    fn record(&self, domain_key: &str, event: E) -> Result<(), StoreError> {
        let fiber_id = derive_fiber_id(domain_key);
        let mut payload = Vec::new();
        event.encode_payload(&mut payload)?;
        let event_id = *uuid::Uuid::now_v7().as_bytes();

        match &self.backend {
            StorageBackend::File(path) => {
                let adapter = FileStorageAdapter::new(path);
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                let handle = session.fiber(fiber_id)?;
                if handle.is_detached() {
                    session.rescue_fiber(fiber_id, event_id, payload)?;
                } else {
                    session.append_to_fiber(fiber_id, event_id, payload)?;
                }
                session.sync()?;
            }
            StorageBackend::Nats(adapter) => {
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                let handle = session.fiber(fiber_id)?;
                if handle.is_detached() {
                    session.rescue_fiber(fiber_id, event_id, payload)?;
                } else {
                    session.append_to_fiber(fiber_id, event_id, payload)?;
                }
                session.sync()?;
            }
        }

        self.cached
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .push((false, fiber_id, event));
        Ok(())
    }

    fn detach(&self, domain_key: &str, event: E) -> Result<(), StoreError> {
        let fiber_id = derive_fiber_id(domain_key);
        let mut payload = Vec::new();
        event.encode_payload(&mut payload)?;
        let event_id = *uuid::Uuid::now_v7().as_bytes();

        match &self.backend {
            StorageBackend::File(path) => {
                let adapter = FileStorageAdapter::new(path);
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                let handle = session.fiber(fiber_id)?;
                if handle.is_active() {
                    session.detach_fiber(fiber_id, event_id, payload)?;
                    session.sync()?;
                }
            }
            StorageBackend::Nats(adapter) => {
                let epoch = adapter.current_epoch()?;
                let mut session = adapter.open_write(epoch)?;
                let handle = session.fiber(fiber_id)?;
                if handle.is_active() {
                    session.detach_fiber(fiber_id, event_id, payload)?;
                    session.sync()?;
                }
            }
        }

        self.cached
            .lock()
            .map_err(|_| StoreError::Poisoned)?
            .push((true, fiber_id, event));
        Ok(())
    }

    fn events(&self) -> Result<Vec<(bool, E)>, StoreError>
    where
        E: Clone,
    {
        let cached = self.cached.lock().map_err(|_| StoreError::Poisoned)?;
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
        let cached = self.cached.lock().map_err(|_| StoreError::Poisoned)?;
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
        let cached = self.cached.lock().map_err(|_| StoreError::Poisoned)?;
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
        let cached = self.cached.lock().map_err(|_| StoreError::Poisoned)?;
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
        self.inner.detach(domain_key, event)
    }

    /// Return all cached events with their detached status and fiber ID.
    ///
    /// # Errors
    /// Returns [`StoreError::Poisoned`] if the store cache lock is poisoned.
    pub fn events_with_fibers(&self) -> Result<Vec<(bool, [u8; 16], DomainEvent)>, StoreError> {
        let cached = self.inner.cached.lock().map_err(|_| StoreError::Poisoned)?;
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
        self.inner.detach(team_key, event)
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
mod tests {
    use super::*;
    use crate::event::team_domain_key;

    fn synthetic_domain_event(i: u64) -> DomainEvent {
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

    fn synthetic_team_state(org: &str, team_slug: &str) -> TeamStateCaptured {
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
        let session = adapter.create(&claim).expect("create bare store");
        drop(session);

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
        let mut session = adapter.create(&claim).expect("create bare store");
        let mismatched_desc = SchemaDescriptor::new(999, DescriptorNode::U64);
        session
            .set_schema_descriptor(&mismatched_desc)
            .expect("set mismatched descriptor");
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
        let session = adapter_unadmitted
            .create(&claim)
            .expect("create bare store");
        drop(session);

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
            .create(&claim_mismatched)
            .expect("create mismatched store");
        let mismatched_desc = SchemaDescriptor::new(999, DescriptorNode::U64);
        session_mismatched
            .set_schema_descriptor(&mismatched_desc)
            .expect("set mismatched descriptor");
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

    fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
        NonEmptyEventString::new(s.to_string()).expect("valid non-empty string")
    }

    fn ts(nanos: u64) -> Timestamp {
        Timestamp::new(nanos).expect("valid timestamp")
    }

    fn resolve_pinned_nats_server() -> Result<std::path::PathBuf, String> {
        let pinned = "2.14.5";
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
            .arg("-v")
            .output()
            .map_err(|e| format!("nats-server absent from PATH: {e}"))?;
        let version_str = String::from_utf8_lossy(&output.stdout);
        let version_err = String::from_utf8_lossy(&output.stderr);
        let full = format!("{version_str} {version_err}");
        if !full.contains(pinned) {
            return Err(format!(
                "version mismatch (expected {pinned}, got {})",
                full.trim()
            ));
        }
        Ok(bin_path)
    }

    struct TestNatsServer {
        url: String,
        child: std::process::Child,
        _tempdir: tempfile::TempDir,
    }

    impl TestNatsServer {
        fn spawn() -> Option<Self> {
            let bin_path = match resolve_pinned_nats_server() {
                Ok(path) => path,
                Err(reason) => {
                    eprintln!(
                        "SKIP nats_store_admission_roundtrip_and_rejection: live nats-server unavailable: {reason}"
                    );
                    return None;
                }
            };

            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind test port");
            let port = listener.local_addr().expect("local addr").port();
            drop(listener);

            let tempdir = tempfile::TempDir::new().expect("tempdir");
            let child = std::process::Command::new(bin_path)
                .arg("-a")
                .arg("127.0.0.1")
                .arg("-p")
                .arg(port.to_string())
                .arg("-js")
                .arg("-sd")
                .arg(tempdir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn nats-server");

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
            let mut dead_child = child;
            let _ = dead_child.kill();
            let _ = dead_child.wait();
            panic!("nats-server readiness timeout on 127.0.0.1:{port}");
        }
    }

    impl Drop for TestNatsServer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
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
            .create(&claim)
            .expect("create bare nats store");
        let mismatched_desc = SchemaDescriptor::new(999, DescriptorNode::U64);
        session
            .set_schema_descriptor(&mismatched_desc)
            .expect("set mismatched nats descriptor");
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
        let session_unadmitted = adapter_unadmitted_raw
            .create(&claim_unadmitted)
            .expect("create unadmitted nats store");
        drop(session_unadmitted);

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

    #[tokio::test]
    async fn nats_adapter_handle_drops_safely_inside_tokio_worker_context() {
        let Some(server) = TestNatsServer::spawn() else {
            return;
        };
        let url = server.url.clone();
        let handle = tokio::task::spawn_blocking(move || {
            let rt = std::sync::Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("adapter runtime"),
            );
            let client = rt
                .block_on(async_nats::connect(&url))
                .expect("connect to live nats");
            let adapter =
                NatsStorageAdapter::from_client_with_runtime(client, "test_handle_drop", rt);
            NatsAdapterHandle::new(adapter)
        })
        .await
        .expect("spawn_blocking construct adapter");

        assert!(tokio::runtime::Handle::try_current().is_ok());
        drop(handle);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
