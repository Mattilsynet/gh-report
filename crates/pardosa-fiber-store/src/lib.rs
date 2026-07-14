//! Synchronous one-key-one-fiber adapter over `pardosa::store`.

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::path::Path;
use std::sync::Mutex;

use pardosa::store::{
    BackendError, BackendOp, Decode, Encode, Event, EventStore as PardosaStore, FiberId,
    FiberLookup, FiberState, GenomeSafe, HasEventSchemaSource, JetStreamBackend, LiveFiber,
    PardosaError, PgnoBackend, RecoveryError, RecoveryOutcome,
};
use tokio::runtime::{Handle, RuntimeFlavor};

/// Extracts the domain key from a pardosa envelope.
pub type KeyFn<E> = fn(&Event<E>) -> std::iter::Once<String>;

/// Failure surface of the generic pardosa fiber store.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FiberStoreError {
    #[error("pardosa fiber store infrastructure error: {0}")]
    Infrastructure(String),
    #[error("pardosa fiber store backend `{op}` infrastructure error: {source}")]
    BackendInfrastructure {
        op: BackendOp,
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    #[error("pardosa fiber store torn-write recovery failed: {source}")]
    TornWriteRecovery {
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    #[error("pardosa fiber store concurrency conflict: {source}")]
    ConcurrencyConflict {
        #[source]
        source: Box<dyn Error + Send + Sync + 'static>,
    },
    #[error("domain key {key:?} maps to multiple fibers; one-fiber-per-key invariant violated")]
    DivergedFiber { key: String },
    #[error("store mutex poisoned")]
    Poisoned,
}

impl FiberStoreError {
    /// Classify a public pardosa store error into the fiber-store taxonomy.
    #[must_use]
    pub fn from_pardosa_error(error: PardosaError) -> Self {
        classify_infrastructure_error(error)
    }

    /// Classify a public pardosa replay error into the fiber-store taxonomy.
    #[must_use]
    pub fn from_replay_error(error: pardosa::store::replay::Error) -> Self {
        classify_infrastructure_error(error)
    }
}

/// Pardosa-native fiber store: one fiber per caller-defined domain key.
pub struct FiberStore<E> {
    inner: Mutex<Inner<E>>,
    last_recovery: Option<RecoveryOutcome>,
}

impl<E> FiberStore<E>
where
    E: Encode + Decode + GenomeSafe + HasEventSchemaSource,
{
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, FiberStoreError> {
        let store = PardosaStore::<E>::create(path).map_err(classify_infrastructure_error)?;
        Ok(Self::from_store(store, false, None))
    }

    /// Create a fresh JetStream-backed store.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot author
    /// the canonical-empty container on the backend.
    ///
    /// # Panics
    ///
    /// Panics when the returned store is used from inside a Tokio
    /// `current_thread` runtime; the sync bridge uses
    /// `tokio::task::block_in_place`, which requires a multi-thread runtime.
    pub fn create_jetstream(backend: JetStreamBackend) -> Result<Self, FiberStoreError> {
        let store = PardosaStore::<E>::create_with_backend(backend)
            .map_err(classify_infrastructure_error)?;
        Ok(Self::from_store(store, true, None))
    }
}

impl<E> FiberStore<E>
where
    E: Decode + GenomeSafe,
{
    /// Open an existing `.pgno`-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot open or
    /// fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, FiberStoreError> {
        let store = PardosaStore::<E>::open_with_backend(PgnoBackend::open(path))
            .map_err(classify_infrastructure_error)?;
        let last_recovery = store.last_recovery().cloned();
        Ok(Self::from_store(store, false, last_recovery))
    }

    /// Open an existing JetStream-backed store, rehydrating its fibers.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] when pardosa cannot fetch or
    /// rehydrate the JetStream-authoritative line.
    ///
    /// # Panics
    ///
    /// Panics when the returned store is used from inside a Tokio
    /// `current_thread` runtime; the sync bridge uses
    /// `tokio::task::block_in_place`, which requires a multi-thread runtime.
    pub fn open_jetstream(backend: JetStreamBackend) -> Result<Self, FiberStoreError> {
        let store =
            PardosaStore::<E>::open_with_backend(backend).map_err(classify_infrastructure_error)?;
        Ok(Self::from_store(store, true, None))
    }
}

impl<E> FiberStore<E> {
    fn from_store(
        store: PardosaStore<E>,
        bridge_runtime: bool,
        last_recovery: Option<RecoveryOutcome>,
    ) -> Self {
        Self {
            inner: Mutex::new(inner(store, bridge_runtime)),
            last_recovery,
        }
    }

    /// The last recovery outcome reported by the underlying `.pgno` open path.
    #[must_use]
    pub fn last_recovery(&self) -> Option<&RecoveryOutcome> {
        self.last_recovery.as_ref()
    }
}

impl<E> FiberStore<E>
where
    E: Clone + Encode + GenomeSafe,
{
    /// Capture an event onto the key's fiber, then fence.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::DivergedFiber`] when the key already maps to
    /// more than one fiber, [`FiberStoreError::Infrastructure`] on pardosa
    /// append/sync failure, or [`FiberStoreError::Poisoned`].
    ///
    /// # Durability
    ///
    /// `Ok(())` means durably captured and fenced. `Err` means the append did
    /// NOT durably land: JetStream-backed stores perform a single
    /// `publish_once` with no automatic retry (PGN-0016:R6), so an error is a
    /// genuine single-shot failure, not an absorbed transient.
    ///
    /// # Recovery
    ///
    /// Recovery from an `Err` is the caller's obligation, per the port
    /// semantics COM-0025:R1 and PGN-0016:R6 require: retry (idempotent
    /// under the subject-sequence fence, PGN-0016:R2), dead-letter, or
    /// accept the loss. [`FiberStoreError::Infrastructure`] and
    /// [`FiberStoreError::BackendInfrastructure`] are the retryable
    /// substrate-failure surfaces; [`FiberStoreError::DivergedFiber`] and
    /// [`FiberStoreError::Poisoned`] are terminal and MUST NOT be retried.
    pub fn record(&self, domain_key: &str, event: E, key: KeyFn<E>) -> Result<(), FiberStoreError> {
        record_defined(&self.inner, domain_key, event, key)
    }

    /// Soft-delete a key's fiber, then fence.
    ///
    /// A later [`Self::record`] of the same key rescues the fiber back to live.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::DivergedFiber`],
    /// [`FiberStoreError::Infrastructure`], or [`FiberStoreError::Poisoned`]. A
    /// no-op key that was never seen or is already detached returns `Ok(())`.
    ///
    /// # Durability
    ///
    /// On `Ok(())` the soft-delete is durably fenced. As with
    /// [`Self::record`], an `Err` from a JetStream-backed store is a
    /// single-shot `publish_once` failure with no substrate-level retry
    /// (PGN-0016:R6); the detach did not durably land.
    ///
    /// # Recovery
    ///
    /// Recovery is the caller's obligation (COM-0025:R1). Retry is
    /// idempotent under the subject-sequence fence (PGN-0016:R2);
    /// alternatively dead-letter or accept the loss. Note a never-seen or
    /// already-detached key returns `Ok(())` (no-op), so `Err` always
    /// denotes a genuine substrate failure, never a missing key.
    pub fn detach(&self, domain_key: &str, event: E, key: KeyFn<E>) -> Result<(), FiberStoreError> {
        let mut guard = self.inner.lock().map_err(|_| FiberStoreError::Poisoned)?;
        let inner = &mut *guard;
        bridge(inner.bridge_runtime, || {
            let fiber = match inner.live.remove(domain_key) {
                Some(fiber) => fiber,
                None => match resolve_fiber(&inner.store, domain_key, key)? {
                    Resolved::Defined(fid) => {
                        match inner.store.writer().resume_defined(fid, event.clone()) {
                            Ok(receipt) => receipt.fiber(),
                            Err(error) => return Err(classify_infrastructure_error(error)),
                        }
                    }
                    Resolved::Detached(_) | Resolved::Absent => return Ok(()),
                },
            };
            let _receipt = inner
                .store
                .writer()
                .detach(fiber, event)
                .map_err(classify_infrastructure_error)?;
            let _position = inner
                .store
                .writer()
                .sync()
                .map_err(classify_infrastructure_error)?;
            Ok(())
        })
    }

    /// The latest event of every live fiber, paired with its domain key.
    ///
    /// Detached fibers are excluded.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Infrastructure`] on pardosa read failure or
    /// [`FiberStoreError::Poisoned`].
    pub fn latest_defined(&self, key: KeyFn<E>) -> Result<Vec<(String, E)>, FiberStoreError> {
        let guard = self.inner.lock().map_err(|_| FiberStoreError::Poisoned)?;
        bridge(guard.bridge_runtime, || latest_defined(&guard.store, key))
    }

    /// Every event in the store, in committed line order.
    ///
    /// Each item pairs the pardosa envelope `detached` flag with the payload.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Poisoned`] when the store mutex is poisoned.
    pub fn all_events(&self) -> Result<Vec<(bool, E)>, FiberStoreError> {
        let guard = self.inner.lock().map_err(|_| FiberStoreError::Poisoned)?;
        Ok(bridge(guard.bridge_runtime, || all_events(&guard.store)))
    }

    /// Fold every event in committed line order without materialising a vector.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Poisoned`] when the store mutex is poisoned.
    pub fn fold_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, bool, &E),
    ) -> Result<R, FiberStoreError> {
        let guard = self.inner.lock().map_err(|_| FiberStoreError::Poisoned)?;
        Ok(bridge(guard.bridge_runtime, || {
            fold_all_events(&guard.store, init, fold)
        }))
    }

    /// Fold every event on live fibers without materialising a vector.
    ///
    /// # Errors
    ///
    /// Returns [`FiberStoreError::Poisoned`] when the store mutex is poisoned.
    pub fn fold_defined_events<R>(
        &self,
        init: R,
        fold: impl FnMut(&mut R, &E),
    ) -> Result<R, FiberStoreError> {
        let guard = self.inner.lock().map_err(|_| FiberStoreError::Poisoned)?;
        Ok(bridge(guard.bridge_runtime, || {
            fold_all_defined_events(&guard.store, init, fold)
        }))
    }
}

struct Inner<E> {
    store: PardosaStore<E>,
    live: HashMap<String, LiveFiber>,
    bridge_runtime: bool,
}

enum Resolved {
    Defined(FiberId),
    Detached(FiberId),
    Absent,
}

trait InfrastructureError: Error + Send + Sync + 'static {
    fn backend_op(&self) -> Option<BackendOp>;
}

impl InfrastructureError for PardosaError {
    fn backend_op(&self) -> Option<BackendOp> {
        backend_op_from_error_chain(self)
    }
}

impl InfrastructureError for pardosa::store::replay::Error {
    fn backend_op(&self) -> Option<BackendOp> {
        backend_op_from_error_chain(self)
    }
}

fn inner<E>(store: PardosaStore<E>, bridge_runtime: bool) -> Inner<E> {
    Inner {
        store,
        live: HashMap::new(),
        bridge_runtime,
    }
}

fn backend_op_from_backend_error(error: &BackendError) -> Option<BackendOp> {
    match error {
        BackendError::Timeout { op, .. }
        | BackendError::Connect { op, .. }
        | BackendError::Replay { op, .. }
        | BackendError::Publish { op, .. } => Some(*op),
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
        if error.downcast_ref::<RecoveryError>().is_some() {
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

fn classify_infrastructure_error<E: InfrastructureError>(error: E) -> FiberStoreError {
    let source = &error as &dyn Error;
    if matches!(
        source.downcast_ref::<PardosaError>(),
        Some(PardosaError::ConcurrencyConflict { .. })
    ) {
        return FiberStoreError::ConcurrencyConflict {
            source: Box::new(error),
        };
    }
    if has_pardosa_concurrency_conflict(source) {
        return FiberStoreError::ConcurrencyConflict {
            source: Box::new(PardosaError::ConcurrencyConflict {
                source: Box::new(error),
            }),
        };
    }
    if has_torn_write_recovery(source) {
        return FiberStoreError::TornWriteRecovery {
            source: Box::new(error),
        };
    }
    if let Some(op) = error.backend_op() {
        FiberStoreError::BackendInfrastructure {
            op,
            source: Box::new(error),
        }
    } else {
        FiberStoreError::Infrastructure(error.to_string())
    }
}

fn record_defined<E: Clone + Encode + GenomeSafe>(
    inner: &Mutex<Inner<E>>,
    domain_key: &str,
    event: E,
    key: KeyFn<E>,
) -> Result<(), FiberStoreError> {
    let mut guard = inner.lock().map_err(|_| FiberStoreError::Poisoned)?;
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
        .map_err(classify_infrastructure_error)?;
        inner.live.insert(domain_key.to_string(), receipt.fiber());
        let _position = inner
            .store
            .writer()
            .sync()
            .map_err(classify_infrastructure_error)?;
        Ok(())
    })
}

fn resolve_fiber<E>(
    store: &PardosaStore<E>,
    domain_key: &str,
    key: KeyFn<E>,
) -> Result<Resolved, FiberStoreError> {
    let index = store.reader().fiber_index::<String, _, _>(key);
    match index.lookup(&domain_key.to_string()) {
        FiberLookup::Unique(fid) => {
            let reader = store.reader();
            match reader.fiber(fid).state() {
                FiberState::Detached => Ok(Resolved::Detached(fid)),
                _ => Ok(Resolved::Defined(fid)),
            }
        }
        FiberLookup::Diverged { .. } => Err(FiberStoreError::DivergedFiber {
            key: domain_key.to_string(),
        }),
        _ => Ok(Resolved::Absent),
    }
}

fn latest_defined<E: Clone>(
    store: &PardosaStore<E>,
    key: KeyFn<E>,
) -> Result<Vec<(String, E)>, FiberStoreError> {
    let keys = RefCell::new(Vec::<String>::new());
    let index = store.reader().fiber_index::<String, _, _>(|event| {
        let key = key(event).next().into_iter();
        key.inspect(|key| keys.borrow_mut().push(key.clone()))
    });
    let reader = store.reader();
    let mut latest = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for key in keys.into_inner() {
        if !seen.insert(key.clone()) {
            continue;
        }
        if let FiberLookup::Unique(fid) = index.lookup(&key) {
            let history = reader.fiber(fid);
            if history.state() != FiberState::Defined {
                continue;
            }
            let mut stream = history.iter_rev().map_err(classify_infrastructure_error)?;
            if let Some(event) = stream.next() {
                latest.insert(key, event.domain_event().clone());
            }
        }
    }
    Ok(latest.into_iter().collect())
}

fn all_events<E: Clone>(store: &PardosaStore<E>) -> Vec<(bool, E)> {
    let collected = RefCell::new(Vec::new());
    let _index = store.reader().fiber_index::<u8, _, _>(|event| {
        collected
            .borrow_mut()
            .push((event.detached(), event.domain_event().clone()));
        std::iter::empty()
    });
    collected.into_inner()
}

fn fold_all_events<E, R>(
    store: &PardosaStore<E>,
    init: R,
    fold: impl FnMut(&mut R, bool, &E),
) -> R {
    let accumulated = RefCell::new(init);
    let fold = RefCell::new(fold);
    let _index = store.reader().fiber_index::<u8, _, _>(|event| {
        fold.borrow_mut()(
            &mut accumulated.borrow_mut(),
            event.detached(),
            event.domain_event(),
        );
        std::iter::empty()
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
        std::iter::empty()
    });
    accumulated.into_inner()
}

fn bridge<T>(bridge_runtime: bool, f: impl FnOnce() -> T) -> T {
    if bridge_runtime && let Ok(handle) = Handle::try_current() {
        debug_assert_eq!(
            handle.runtime_flavor(),
            RuntimeFlavor::MultiThread,
            "FiberStore JetStream bridge requires a multi-thread Tokio runtime"
        );
        tokio::task::block_in_place(f)
    } else {
        f()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn backend_timeout(op: BackendOp) -> pardosa::store::replay::Error {
        pardosa::store::replay::Error::Io(std::io::Error::other(BackendError::Timeout {
            op,
            elapsed: Duration::from_millis(750),
            configured: Duration::from_millis(500),
        }))
    }

    fn backend_publish(op: BackendOp) -> pardosa::store::replay::Error {
        pardosa::store::replay::Error::Io(std::io::Error::other(BackendError::Publish {
            op,
            source: Box::new(std::io::Error::other("authorization violation")),
        }))
    }

    #[test]
    fn timeout_on_sync_renders_distinctly_from_timeout_on_append_at_store_boundary() {
        let append = FiberStoreError::from_replay_error(backend_timeout(BackendOp::Append));
        let sync = FiberStoreError::from_replay_error(backend_timeout(BackendOp::Sync));

        match &append {
            FiberStoreError::BackendInfrastructure { op, .. } => {
                assert!(matches!(*op, BackendOp::Append), "append op carried");
            }
            other => panic!("expected BackendInfrastructure for append timeout, got {other:?}"),
        }
        match &sync {
            FiberStoreError::BackendInfrastructure { op, .. } => {
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
    fn publish_failure_on_append_leg_carries_append_op_not_flattened_to_infrastructure() {
        let error = FiberStoreError::from_replay_error(backend_publish(BackendOp::Append));

        match &error {
            FiberStoreError::BackendInfrastructure { op, .. } => {
                assert!(matches!(*op, BackendOp::Append), "append op carried");
            }
            other => panic!(
                "expected BackendInfrastructure for append-leg publish failure, got {other:?}"
            ),
        }
    }

    #[test]
    fn pardosa_concurrency_conflict_is_typed_at_store_boundary() {
        let error = FiberStoreError::from_pardosa_error(PardosaError::ConcurrencyConflict {
            source: Box::new(std::io::Error::other("wrong last sequence")),
        });

        assert!(
            matches!(error, FiberStoreError::ConcurrencyConflict { .. }),
            "typed PardosaError::ConcurrencyConflict must not be flattened to Infrastructure"
        );
    }

    #[test]
    fn persisted_backend_concurrency_conflict_wraps_existing_pardosa_variant() {
        let error = FiberStoreError::from_replay_error(pardosa::store::replay::Error::Io(
            std::io::Error::other(BackendError::ConcurrencyConflict {
                source: Box::new(std::io::Error::other("wrong last sequence")),
            }),
        ));

        let FiberStoreError::ConcurrencyConflict { source } = error else {
            panic!("expected FiberStoreError::ConcurrencyConflict");
        };
        assert!(
            source.to_string().contains("concurrency conflict"),
            "wrapped persist error must expose a concurrency conflict in the source chain"
        );
    }

    #[test]
    fn recovery_error_source_chain_marks_torn_write_recovery() {
        let recovery = RecoveryError::DataEndExceedsFile {
            manifest_data_end: 12,
            pgno_len: 8,
        };

        assert!(
            has_torn_write_recovery(&recovery),
            "public RecoveryError in source chain must classify as torn-write recovery"
        );
    }
}
