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

#[derive(Clone)]
enum StorageBackend {
    File(PathBuf),
    Nats(Box<NatsStorageAdapter>),
}

struct StoreInner<E> {
    backend: StorageBackend,
    cached: Mutex<Vec<(bool, [u8; 16], E)>>,
}

impl<E: PardosaSchema> StoreInner<E> {
    fn create_pgno(path: &Path, label: &'static str) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let claim = default_claim(1, label);
        let _session = adapter.create(&claim)?;
        Ok(Self {
            backend: StorageBackend::File(path.to_path_buf()),
            cached: Mutex::new(Vec::new()),
        })
    }

    fn open_pgno(path: &Path, _label: &'static str) -> Result<Self, StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let mut reader = adapter.open_read()?;
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
        let _session = adapter.create(&claim)?;
        Ok(Self {
            backend: StorageBackend::Nats(Box::new(adapter)),
            cached: Mutex::new(Vec::new()),
        })
    }

    fn open_nats(adapter: NatsStorageAdapter, _label: &'static str) -> Result<Self, StoreError> {
        let mut reader = adapter.open_read()?;
        let envelopes = reader.read_all_envelopes()?;
        let mut cached = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let event = E::decode_payload(&env.payload)?;
            cached.push((env.header.detached, env.header.fiber_id, event));
        }
        Ok(Self {
            backend: StorageBackend::Nats(Box::new(adapter)),
            cached: Mutex::new(cached),
        })
    }

    fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), StoreError> {
        let adapter = FileStorageAdapter::new(path);
        let mut reader = adapter.open_read()?;
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
}
