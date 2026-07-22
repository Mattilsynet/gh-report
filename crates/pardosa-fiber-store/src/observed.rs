use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arc_swap::ArcSwap;
use pardosa::store::{
    Decode, Encode, GenomeSafe, HasEventSchemaSource, JetStreamBackend, RecoveryOutcome,
};

use crate::{FiberStore, FiberStoreError, KeyFn};

/// Swappable, backend-health-observing wrapper over [`FiberStore`].
///
/// `inner` is swappable (not a plain field) so callers can atomically
/// replace the whole fiber-store instance with a freshly re-opened one on
/// a consumer-owned re-seed (Design-Y). `backend_reachable` tracks whether
/// the most recent delegated call observed a `BackendInfrastructure`
/// error; it carries no application policy beyond that flip.
pub struct ObservedFiberStore<E> {
    inner: ArcSwap<FiberStore<E>>,
    backend_reachable: AtomicBool,
}

impl<E> ObservedFiberStore<E>
where
    E: Encode + Decode + GenomeSafe + HasEventSchemaSource,
{
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot
    /// create the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, FiberStoreError> {
        let store = FiberStore::<E>::create_pgno(path)?;
        Ok(Self::from_store(store))
    }

    /// Create a fresh JetStream-backed store.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot
    /// author the canonical-empty container on the backend.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime;
    /// JetStream-backed pardosa calls are bridged with
    /// `tokio::task::block_in_place`, which requires a multi-thread runtime.
    pub fn create_jetstream(backend: JetStreamBackend) -> Result<Self, FiberStoreError> {
        let store = FiberStore::<E>::create_jetstream(backend)?;
        Ok(Self::from_store(store))
    }
}

impl<E> ObservedFiberStore<E>
where
    E: Decode + GenomeSafe,
{
    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// Label-free: does not report recovery. Callers wanting app-level
    /// recovery telemetry read [`Self::last_recovery`] after this returns.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot open
    /// or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, FiberStoreError> {
        let store = FiberStore::<E>::open_pgno(path)?;
        Ok(Self::from_store(store))
    }

    /// Open an existing JetStream-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot
    /// fetch or rehydrate the JetStream-authoritative line.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime; see
    /// [`Self::create_jetstream`].
    pub fn open_jetstream(backend: JetStreamBackend) -> Result<Self, FiberStoreError> {
        let store = FiberStore::<E>::open_jetstream(backend)?;
        Ok(Self::from_store(store))
    }

    /// Re-seed from a fresh authoritative read of the same `.pgno` backing
    /// file, atomically replacing the fiber-store snapshot in place.
    ///
    /// Label-free, same as [`Self::open_pgno`]: callers wanting recovery
    /// telemetry read [`Self::last_recovery`] after this returns (it
    /// reflects the freshly-swapped-in store).
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot
    /// re-open the backing container.
    pub fn resync_pgno_from_authoritative(&self, path: &Path) -> Result<(), FiberStoreError> {
        let fresh = FiberStore::<E>::open_pgno(path)?;
        self.inner.store(Arc::new(fresh));
        self.backend_reachable.store(true, Ordering::Release);
        Ok(())
    }

    /// Re-seed from a fresh authoritative `JetStream` replay, atomically
    /// replacing the fiber-store snapshot in place.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot
    /// fetch or rehydrate the JetStream-authoritative line.
    ///
    /// # Panics
    ///
    /// Panics when called from inside a Tokio `current_thread` runtime; see
    /// [`Self::create_jetstream`].
    pub fn resync_jetstream_from_authoritative(
        &self,
        backend: JetStreamBackend,
    ) -> Result<(), FiberStoreError> {
        let fresh = FiberStore::<E>::open_jetstream(backend)?;
        self.inner.store(Arc::new(fresh));
        self.backend_reachable.store(true, Ordering::Release);
        Ok(())
    }
}

impl<E> ObservedFiberStore<E> {
    fn from_store(store: FiberStore<E>) -> Self {
        Self {
            inner: ArcSwap::new(Arc::new(store)),
            backend_reachable: AtomicBool::new(true),
        }
    }

    /// The last recovery outcome reported by the current snapshot's
    /// underlying `.pgno` open path.
    #[must_use]
    pub fn last_recovery(&self) -> Option<RecoveryOutcome> {
        self.inner.load().last_recovery().cloned()
    }

    /// Whether the most recent delegated call observed the backend as
    /// reachable (no [`FiberStoreError::BackendInfrastructure`]).
    #[must_use]
    pub fn backend_reachable(&self) -> bool {
        self.backend_reachable.load(Ordering::Acquire)
    }

    fn observe_result<T>(&self, result: &Result<T, FiberStoreError>) {
        if matches!(result, Err(FiberStoreError::BackendInfrastructure { .. })) {
            self.backend_reachable.store(false, Ordering::Release);
        } else if result.is_ok() {
            self.backend_reachable.store(true, Ordering::Release);
        }
    }

    /// Force `backend_reachable` to `false`, as if the last delegated call
    /// had observed a [`FiberStoreError::BackendInfrastructure`]. Test
    /// support only — gated per the CHE-0058 cross-crate test-seam carve-out
    /// (mirrors the `cherry-pit-core` `testing` feature precedent), so
    /// downstream crates can exercise it from their own `#[cfg(test)]` code
    /// by activating this crate's `testing` feature under
    /// `[dev-dependencies]`.
    #[cfg(any(test, feature = "testing"))]
    pub fn mark_backend_unreachable_for_test(&self) {
        self.backend_reachable.store(false, Ordering::Release);
    }
}

impl<E> ObservedFiberStore<E>
where
    E: Clone + Encode + GenomeSafe,
{
    /// Capture an event onto the key's fiber, then fence.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::DivergedFiber`] when the key already
    /// maps to more than one fiber, [`FiberStoreError::Infrastructure`] on
    /// pardosa append/sync failure, or [`FiberStoreError::Poisoned`].
    pub fn record(&self, domain_key: &str, event: E, key: KeyFn<E>) -> Result<(), FiberStoreError> {
        let result = self.inner.load().record(domain_key, event, key);
        self.observe_result(&result);
        result
    }

    /// Soft-delete a key's fiber (detach), then fence. A later
    /// [`Self::record`] of the same key rescues it back to live.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::DivergedFiber`],
    /// [`FiberStoreError::Infrastructure`], or [`FiberStoreError::Poisoned`].
    /// A no-op (key never seen / already detached) returns `Ok(())`.
    pub fn detach(&self, domain_key: &str, event: E, key: KeyFn<E>) -> Result<(), FiberStoreError> {
        let result = self.inner.load().detach(domain_key, event, key);
        self.observe_result(&result);
        result
    }

    /// The latest event of every live fiber, paired with its domain key.
    ///
    /// Detached fibers are excluded. Does not flip `backend_reachable`
    /// (matches the pre-extraction behaviour precisely).
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] on pardosa read failure
    /// or [`FiberStoreError::Poisoned`].
    pub fn latest_defined(&self, key: KeyFn<E>) -> Result<Vec<(String, E)>, FiberStoreError> {
        self.inner.load().latest_defined(key)
    }

    /// Every event in the store, in committed line order. Each item pairs
    /// the pardosa envelope `detached` flag with the domain event payload.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] on pardosa read failure
    /// or [`FiberStoreError::Poisoned`].
    pub fn all_events(&self) -> Result<Vec<(bool, E)>, FiberStoreError> {
        let result = self.inner.load().all_events();
        self.observe_result(&result);
        result
    }

    /// Fold every event in committed line order without materialising an
    /// owned event vector.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Poisoned`] when the store mutex is
    /// poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, bool, &E),
    ) -> Result<R, FiberStoreError> {
        let result = self.inner.load().fold_events(init, fold);
        self.observe_result(&result);
        result
    }

    /// Fold every event on live fibers without materialising an owned
    /// vector.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Poisoned`] when the store mutex is
    /// poisoned.
    pub fn fold_defined_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, &E),
    ) -> Result<R, FiberStoreError> {
        let result = self.inner.load().fold_defined_events(init, fold);
        self.observe_result(&result);
        result
    }
}
