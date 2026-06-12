//! Native pardosa-backed event store for gh-report.
//!
//! Each repository's natural domain key maps to one pardosa fiber. The
//! first capture of a repo begins a fiber; subsequent captures append to
//! the same fiber, recovered across restarts by validated-identity resume
//! (PGN-0014) keyed through a [`pardosa::FiberIndex`] over the domain key.
//! Removal is a soft delete via fiber detach; a returning repo is rescued.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use pardosa::store::{
    Event, EventStore as PardosaStore, FiberId, FiberLookup, FiberState, JetStreamBackend,
    LiveFiber, PgnoBackend,
};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::event::DomainEvent;

/// Failure surface of the native pardosa store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("pardosa store infrastructure error: {0}")]
    Infrastructure(String),
    #[error("domain key {key:?} maps to multiple fibers; one-fiber-per-repo invariant violated")]
    DivergedFiber { key: String },
    #[error("store mutex poisoned")]
    Poisoned,
}

fn infra<E: std::fmt::Display>(e: E) -> StoreError {
    StoreError::Infrastructure(e.to_string())
}

struct Inner {
    store: PardosaStore<DomainEvent>,
    live: HashMap<String, LiveFiber>,
    bridge_runtime: bool,
}

/// Pardosa-native event store: one fiber per repository domain key.
pub struct NativeStore {
    inner: Mutex<Inner>,
}

impl NativeStore {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = PardosaStore::<DomainEvent>::create(path).map_err(infra)?;
        Ok(Self::from_store(store, false))
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store =
            PardosaStore::<DomainEvent>::open_with_backend(PgnoBackend::open(path)).map_err(infra)?;
        Ok(Self::from_store(store, false))
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
        let store = PardosaStore::<DomainEvent>::create_with_backend(backend).map_err(infra)?;
        Ok(Self::from_store(store, true))
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
        let store = PardosaStore::<DomainEvent>::open_with_backend(backend).map_err(infra)?;
        Ok(Self::from_store(store, true))
    }

    fn from_store(store: PardosaStore<DomainEvent>, bridge_runtime: bool) -> Self {
        Self {
            inner: Mutex::new(Inner {
                store,
                live: HashMap::new(),
                bridge_runtime,
            }),
        }
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
        let mut guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let inner = &mut *guard;
        bridge(inner.bridge_runtime, || {
            let receipt = if let Some(fiber) = inner.live.remove(domain_key) {
                inner.store.writer().append(fiber, event)
            } else {
                match resolve_fiber(&inner.store, domain_key)? {
                    Resolved::Defined(fid) => inner.store.writer().resume_defined(fid, event),
                    Resolved::Detached(fid) => inner.store.writer().rescue_detached(fid, event),
                    Resolved::Absent => inner.store.writer().begin(event),
                }
            }
            .map_err(infra)?;
            inner.live.insert(domain_key.to_string(), receipt.fiber());
            let _ = inner.store.writer().sync().map_err(infra)?;
            Ok(())
        })
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
        let mut guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let inner = &mut *guard;
        bridge(inner.bridge_runtime, || {
            let fiber = match inner.live.remove(domain_key) {
                Some(fiber) => fiber,
                None => match resolve_fiber(&inner.store, domain_key)? {
                    Resolved::Defined(fid) => {
                        match inner.store.writer().resume_defined(fid, event.clone()) {
                            Ok(receipt) => receipt.fiber(),
                            Err(e) => return Err(infra(e)),
                        }
                    }
                    Resolved::Detached(_) | Resolved::Absent => return Ok(()),
                },
            };
            let _ = inner.store.writer().detach(fiber, event).map_err(infra)?;
            let _ = inner.store.writer().sync().map_err(infra)?;
            Ok(())
        })
    }

    /// The latest event of every live (`Defined`) fiber, paired with its
    /// domain key. Detached fibers are excluded — the soft-delete effect.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] on pardosa read failure or
    /// [`StoreError::Poisoned`].
    pub fn latest_per_repo(&self) -> Result<Vec<(String, DomainEvent)>, StoreError> {
        let guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        bridge(guard.bridge_runtime, || latest_defined(&guard.store))
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
        let guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        bridge(guard.bridge_runtime, || Ok(all_events(&guard.store)))
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
        let guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        bridge(guard.bridge_runtime, || {
            Ok(fold_all_events(&guard.store, init, fold))
        })
    }
}

enum Resolved {
    Defined(FiberId),
    Detached(FiberId),
    Absent,
}

fn key_of(event: &Event<DomainEvent>) -> std::iter::Once<String> {
    let DomainEvent::RepositoryStateCaptured { domain_key, .. } = event.domain_event();
    std::iter::once(domain_key.as_str().to_string())
}

fn resolve_fiber(
    store: &PardosaStore<DomainEvent>,
    domain_key: &str,
) -> Result<Resolved, StoreError> {
    let index = store.reader().fiber_index::<String, _, _>(key_of);
    match index.lookup(&domain_key.to_string()) {
        FiberLookup::Unique(fid) => {
            let reader = store.reader();
            match reader.fiber(fid).state() {
                FiberState::Detached => Ok(Resolved::Detached(fid)),
                _ => Ok(Resolved::Defined(fid)),
            }
        }
        FiberLookup::Diverged { .. } => Err(StoreError::DivergedFiber {
            key: domain_key.to_string(),
        }),
        _ => Ok(Resolved::Absent),
    }
}

fn latest_defined(
    store: &PardosaStore<DomainEvent>,
) -> Result<Vec<(String, DomainEvent)>, StoreError> {
    let keys = RefCell::new(Vec::<String>::new());
    let index = store.reader().fiber_index::<String, _, _>(|event| {
        let DomainEvent::RepositoryStateCaptured { domain_key, .. } = event.domain_event();
        let key = domain_key.as_str().to_string();
        keys.borrow_mut().push(key.clone());
        std::iter::once(key)
    });
    let reader = store.reader();
    let mut latest: HashMap<String, DomainEvent> = HashMap::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for key in keys.into_inner() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if let FiberLookup::Unique(fid) = index.lookup(&key) {
            let history = reader.fiber(fid);
            if history.state() != FiberState::Defined {
                continue;
            }
            let mut stream = history.iter_rev().map_err(infra)?;
            if let Some(event) = stream.next() {
                latest.insert(key, event.domain_event().clone());
            }
        }
    }
    Ok(latest.into_iter().collect())
}

fn all_events(store: &PardosaStore<DomainEvent>) -> Vec<(bool, DomainEvent)> {
    let collected = RefCell::new(Vec::<(bool, DomainEvent)>::new());
    let _index = store.reader().fiber_index::<u8, _, _>(|event| {
        collected
            .borrow_mut()
            .push((event.detached(), event.domain_event().clone()));
        std::iter::empty::<u8>()
    });
    collected.into_inner()
}

fn fold_all_events<R>(
    store: &PardosaStore<DomainEvent>,
    init: R,
    fold: impl FnMut(&mut R, bool, &DomainEvent),
) -> R {
    let accumulated = RefCell::new(init);
    let fold = RefCell::new(fold);
    let _index = store.reader().fiber_index::<u8, _, _>(|event| {
        fold.borrow_mut()(
            &mut accumulated.borrow_mut(),
            event.detached(),
            event.domain_event(),
        );
        std::iter::empty::<u8>()
    });
    accumulated.into_inner()
}

fn bridge<T>(bridge_runtime: bool, f: impl FnOnce() -> T) -> T {
    if bridge_runtime && let Ok(handle) = Handle::try_current() {
        debug_assert_eq!(
            handle.runtime_flavor(),
            RuntimeFlavor::MultiThread,
            "NativeStore JetStream bridge requires a multi-thread Tokio runtime"
        );
        tokio::task::block_in_place(f)
    } else {
        f()
    }
}
