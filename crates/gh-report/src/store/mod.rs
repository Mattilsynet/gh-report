//! Native pardosa-backed event store for gh-report.
//!
//! Each repository's natural domain key maps to one pardosa fiber. The
//! first capture of a repo begins a fiber; subsequent captures append to
//! the same fiber, recovered across restarts by validated-identity resume
//! (PGN-0014) keyed through a [`pardosa::FiberIndex`] over the domain key.
//! Removal is a soft delete via fiber detach; a returning repo is rescued.

use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use pardosa::store::{
    BackendError, BackendOp, Encode, Event, EventStore as PardosaStore, FiberId, FiberLookup,
    FiberState, GenomeSafe, JetStreamBackend, LiveFiber, PardosaError, PgnoBackend,
    RecoveryOutcome,
};
use tokio::runtime::{Handle, RuntimeFlavor};

use crate::event::{DomainEvent, OrgStateCaptured};

/// Failure surface of the native pardosa store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("pardosa store infrastructure error: {0}")]
    Infrastructure(String),
    #[error("pardosa store backend `{op}` infrastructure error: {source}")]
    BackendInfrastructure {
        op: BackendOp,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("pardosa store torn-write recovery failed: {source}")]
    TornWriteRecovery {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("pardosa store concurrency conflict: {source}")]
    ConcurrencyConflict {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
    #[error("domain key {key:?} maps to multiple fibers; one-fiber-per-repo invariant violated")]
    DivergedFiber { key: String },
    #[error("store mutex poisoned")]
    Poisoned,
}

fn backend_op_from_backend_error(error: &BackendError) -> Option<BackendOp> {
    match error {
        BackendError::Timeout { op, .. }
        | BackendError::Connect { op, .. }
        | BackendError::Replay { op, .. } => Some(*op),
        _ => None,
    }
}

fn backend_op_from_error_chain(error: &(dyn Error + 'static)) -> Option<BackendOp> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(backend) = error.downcast_ref::<BackendError>() {
            return backend_op_from_backend_error(backend);
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
            && let Some(backend) = inner.downcast_ref::<BackendError>()
        {
            return backend_op_from_backend_error(backend);
        }
        current = error.source();
    }
    None
}

fn has_pardosa_concurrency_conflict(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if matches!(
            error.downcast_ref::<PardosaError>(),
            Some(PardosaError::ConcurrencyConflict { .. })
        ) || matches!(
            error.downcast_ref::<BackendError>(),
            Some(BackendError::ConcurrencyConflict { .. })
        ) {
            return true;
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
            && has_pardosa_concurrency_conflict(inner)
        {
            return true;
        }
        current = error.source();
    }
    false
}

fn has_torn_write_recovery(error: &(dyn Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if matches!(
            error.downcast_ref::<pardosa_file::FileError>(),
            Some(pardosa_file::FileError::TornWriteRecovery { .. })
        ) || matches!(
            error.downcast_ref::<pardosa::store::replay::Error>(),
            Some(pardosa::store::replay::Error::File(
                pardosa_file::FileError::TornWriteRecovery { .. }
            ))
        ) {
            return true;
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>()
            && let Some(inner) = io.get_ref()
            && has_torn_write_recovery(inner)
        {
            return true;
        }
        current = error.source();
    }
    false
}

trait StoreInfrastructureError: std::error::Error + Send + Sync + 'static {
    fn backend_op(&self) -> Option<BackendOp>;
}

impl StoreInfrastructureError for PardosaError {
    fn backend_op(&self) -> Option<BackendOp> {
        backend_op_from_error_chain(self)
    }
}

impl StoreInfrastructureError for pardosa::store::replay::Error {
    fn backend_op(&self) -> Option<BackendOp> {
        backend_op_from_error_chain(self)
    }
}

fn infra<E: StoreInfrastructureError>(e: E) -> StoreError {
    let error = &e as &dyn std::error::Error;
    if matches!(
        error.downcast_ref::<PardosaError>(),
        Some(PardosaError::ConcurrencyConflict { .. })
    ) {
        return StoreError::ConcurrencyConflict {
            source: Box::new(e),
        };
    }
    if has_pardosa_concurrency_conflict(error) {
        return StoreError::ConcurrencyConflict {
            source: Box::new(PardosaError::ConcurrencyConflict {
                source: Box::new(e),
            }),
        };
    }
    if has_torn_write_recovery(error) {
        return StoreError::TornWriteRecovery {
            source: Box::new(e),
        };
    }
    if let Some(op) = e.backend_op() {
        StoreError::BackendInfrastructure {
            op,
            source: Box::new(e),
        }
    } else {
        StoreError::Infrastructure(e.to_string())
    }
}

struct Inner<E> {
    store: PardosaStore<E>,
    live: HashMap<String, LiveFiber>,
    bridge_runtime: bool,
}

/// Pardosa-native event store: one fiber per repository domain key.
pub struct NativeStore {
    inner: Mutex<Inner<DomainEvent>>,
    backend_reachable: AtomicBool,
    last_recovery: Option<RecoveryOutcome>,
}

/// Pardosa-native org event store: one fiber per org identity.
pub struct NativeOrgStore {
    inner: Mutex<Inner<OrgStateCaptured>>,
    backend_reachable: AtomicBool,
    last_recovery: Option<RecoveryOutcome>,
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
        Ok(Self::from_store(store, false, None))
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = PardosaStore::<DomainEvent>::open_with_backend(PgnoBackend::open(path))
            .map_err(infra)?;
        let last_recovery = store.last_recovery().cloned();
        warn_pgno_recovery("repositories", path, last_recovery.as_ref());
        Ok(Self::from_store(store, false, last_recovery))
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
        Ok(Self::from_store(store, true, None))
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
        Ok(Self::from_store(store, true, None))
    }

    fn from_store(
        store: PardosaStore<DomainEvent>,
        bridge_runtime: bool,
        last_recovery: Option<RecoveryOutcome>,
    ) -> Self {
        Self {
            inner: Mutex::new(inner(store, bridge_runtime)),
            backend_reachable: AtomicBool::new(true),
            last_recovery,
        }
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<&RecoveryOutcome> {
        self.last_recovery.as_ref()
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
            op: BackendOp::Sync,
            source: Box::new(BackendError::Connect {
                op: BackendOp::Sync,
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
        let result = record_defined(&self.inner, domain_key, event, key_of);
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
        let mut guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let inner = &mut *guard;
        let result = bridge(inner.bridge_runtime, || {
            let fiber = match inner.live.remove(domain_key) {
                Some(fiber) => fiber,
                None => match resolve_fiber(&inner.store, domain_key, key_of)? {
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
        });
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
        let result = bridge(guard.bridge_runtime, || Ok(all_events(&guard.store)));
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
        let guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let result = bridge(guard.bridge_runtime, || {
            Ok(fold_all_events(&guard.store, init, fold))
        });
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
        let store = PardosaStore::<OrgStateCaptured>::create(path).map_err(infra)?;
        Ok(Self::from_store(store, false, None))
    }

    /// Open an existing `.pgno`-backed org store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        let store = PardosaStore::<OrgStateCaptured>::open_with_backend(PgnoBackend::open(path))
            .map_err(infra)?;
        let last_recovery = store.last_recovery().cloned();
        warn_pgno_recovery("orgs", path, last_recovery.as_ref());
        Ok(Self::from_store(store, false, last_recovery))
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
        let store =
            PardosaStore::<OrgStateCaptured>::create_with_backend(backend).map_err(infra)?;
        Ok(Self::from_store(store, true, None))
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
        let store = PardosaStore::<OrgStateCaptured>::open_with_backend(backend).map_err(infra)?;
        Ok(Self::from_store(store, true, None))
    }

    fn from_store(
        store: PardosaStore<OrgStateCaptured>,
        bridge_runtime: bool,
        last_recovery: Option<RecoveryOutcome>,
    ) -> Self {
        Self {
            inner: Mutex::new(inner(store, bridge_runtime)),
            backend_reachable: AtomicBool::new(true),
            last_recovery,
        }
    }

    #[must_use]
    pub(crate) fn last_recovery(&self) -> Option<&RecoveryOutcome> {
        self.last_recovery.as_ref()
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
        let result = record_defined(&self.inner, org_key, event, org_key_of);
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
        let guard = self.inner.lock().map_err(|_| StoreError::Poisoned)?;
        let result = bridge(guard.bridge_runtime, || {
            Ok(fold_all_defined_events(&guard.store, init, fold))
        });
        self.observe_result(&result);
        result
    }
}

fn inner<E>(store: PardosaStore<E>, bridge_runtime: bool) -> Inner<E> {
    Inner {
        store,
        live: HashMap::new(),
        bridge_runtime,
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

fn record_defined<E: Clone + Encode + GenomeSafe>(
    inner: &Mutex<Inner<E>>,
    domain_key: &str,
    event: E,
    key: fn(&Event<E>) -> std::iter::Once<String>,
) -> Result<(), StoreError> {
    let mut guard = inner.lock().map_err(|_| StoreError::Poisoned)?;
    let inner = &mut *guard;
    bridge(inner.bridge_runtime, || {
        let receipt = if let Some(fiber) = inner.live.remove(domain_key) {
            inner.store.writer().append(fiber, event)
        } else {
            match resolve_fiber(&inner.store, domain_key, key)? {
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

enum Resolved {
    Defined(FiberId),
    Detached(FiberId),
    Absent,
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

fn resolve_fiber<E>(
    store: &PardosaStore<E>,
    domain_key: &str,
    key: fn(&Event<E>) -> std::iter::Once<String>,
) -> Result<Resolved, StoreError> {
    let index = store.reader().fiber_index::<String, _, _>(key);
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
        let domain_key = match event.domain_event() {
            DomainEvent::RepositoryStateCaptured { domain_key, .. }
            | DomainEvent::RepositoryDeleted { domain_key, .. } => domain_key,
        };
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

fn fold_all_defined_events<E, R>(
    store: &PardosaStore<E>,
    init: R,
    fold: impl FnMut(&mut R, &E),
) -> R {
    let accumulated = RefCell::new(init);
    let fold = RefCell::new(fold);
    let _index = store.reader().fiber_index::<u8, _, _>(|event| {
        if !event.detached() {
            fold.borrow_mut()(&mut accumulated.borrow_mut(), event.domain_event());
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
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

    fn backend_timeout(op: BackendOp) -> pardosa::store::replay::Error {
        pardosa::store::replay::Error::Io(std::io::Error::other(BackendError::Timeout {
            op,
            elapsed: Duration::from_millis(750),
            configured: Duration::from_millis(500),
        }))
    }

    #[test]
    fn timeout_on_sync_renders_distinctly_from_timeout_on_append_at_store_boundary() {
        let append = infra(backend_timeout(BackendOp::Append));
        let sync = infra(backend_timeout(BackendOp::Sync));

        match &append {
            StoreError::BackendInfrastructure { op, .. } => {
                assert!(matches!(*op, BackendOp::Append), "append op carried");
            }
            other => panic!("expected BackendInfrastructure for append timeout, got {other:?}"),
        }
        match &sync {
            StoreError::BackendInfrastructure { op, .. } => {
                assert!(matches!(*op, BackendOp::Sync), "sync op carried");
            }
            other => panic!("expected BackendInfrastructure for sync timeout, got {other:?}"),
        }

        let append_rendered = append.to_string();
        let sync_rendered = sync.to_string();
        assert!(
            append_rendered.contains("append"),
            "render: {append_rendered}"
        );
        assert!(sync_rendered.contains("sync"), "render: {sync_rendered}");
        assert_ne!(append_rendered, sync_rendered);
    }

    #[test]
    fn pardosa_concurrency_conflict_is_typed_at_store_boundary() {
        let err = infra(PardosaError::ConcurrencyConflict {
            source: Box::new(std::io::Error::other("wrong last sequence")),
        });

        assert!(
            matches!(err, StoreError::ConcurrencyConflict { .. }),
            "typed PardosaError::ConcurrencyConflict must not be flattened to Infrastructure"
        );
    }

    #[test]
    fn persisted_backend_concurrency_conflict_wraps_existing_pardosa_variant() {
        let err = infra(pardosa::store::replay::Error::Io(std::io::Error::other(
            BackendError::ConcurrencyConflict {
                source: Box::new(std::io::Error::other("wrong last sequence")),
            },
        )));

        let StoreError::ConcurrencyConflict { source } = err else {
            panic!("expected StoreError::ConcurrencyConflict");
        };
        assert!(
            has_pardosa_concurrency_conflict(source.as_ref()),
            "wrapped persist error must expose PardosaError::ConcurrencyConflict in the source chain"
        );
    }

    #[test]
    fn torn_write_recovery_is_typed_at_store_boundary() {
        let err = infra(PardosaError::CursorRead {
            source: Box::new(pardosa::store::replay::Error::File(
                pardosa_file::FileError::TornWriteRecovery {
                    source: Box::new(pardosa_file::manifest::RecoveryError::DataEndExceedsFile {
                        manifest_data_end: 12,
                        pgno_len: 8,
                    }),
                },
            )),
        });

        assert!(
            matches!(err, StoreError::TornWriteRecovery { .. }),
            "typed FileError::TornWriteRecovery must not be flattened to Infrastructure"
        );
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
}
