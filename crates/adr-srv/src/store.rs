//! Native pardosa `.pgno` store port for adr-srv (CHE-0098 N-R1/N-R4/N-R5).
//!
//! One pardosa fiber per ADR-file aggregate (CHE-0005:R1), keyed by
//! [`AdrIngestedEvent::domain_key`]. Mirrors gh-report's `NativeStore`
//! facade (`crates/gh-report/src/store/mod.rs`): a thin newtype over
//! [`ObservedFiberStore`], which owns fiber resolution and boot-time
//! resume of already-`Defined` fibers (N-R5) via its `open_pgno` path.

use std::path::Path;

use pardosa::store::Event;
pub use pardosa_fiber_store::FiberStoreError as NativeStoreError;
use pardosa_fiber_store::ObservedFiberStore;

use crate::domain::native_event::AdrIngestedEvent;

/// Pardosa-native `AdrIngested` event store: one fiber per ADR file.
pub struct NativeAdrStore(ObservedFiberStore<AdrIngestedEvent>);

impl NativeAdrStore {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`NativeStoreError::Infrastructure`] when pardosa cannot
    /// create the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, NativeStoreError> {
        Ok(Self(ObservedFiberStore::create_pgno(path)?))
    }

    /// Open an existing `.pgno`-backed store, resuming every already-
    /// `Defined` fiber (N-R5 boot contract).
    ///
    /// # Errors
    ///
    /// Returns [`NativeStoreError::Infrastructure`] when pardosa cannot
    /// open or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, NativeStoreError> {
        Ok(Self(ObservedFiberStore::open_pgno(path)?))
    }

    /// Capture an `AdrIngested` native event onto its ADR file's fiber
    /// (first observation begins the fiber; N-R5 resumes it thereafter).
    ///
    /// # Errors
    ///
    /// Returns [`NativeStoreError::DivergedFiber`] when the domain key
    /// already maps to more than one fiber,
    /// [`NativeStoreError::Infrastructure`] on pardosa append/sync
    /// failure, or [`NativeStoreError::Poisoned`].
    pub fn record(&self, event: AdrIngestedEvent) -> Result<(), NativeStoreError> {
        let key = event.domain_key();
        self.0.record(&key, event, key_of)
    }

    /// Every event in committed line order, paired with the pardosa
    /// envelope `detached` flag — the rebuild-from-corpus replay input
    /// (N-R5).
    ///
    /// # Errors
    ///
    /// Returns [`NativeStoreError::Infrastructure`] on pardosa read
    /// failure or [`NativeStoreError::Poisoned`].
    pub fn events(&self) -> Result<Vec<(bool, AdrIngestedEvent)>, NativeStoreError> {
        self.0.all_events()
    }
}

fn key_of(event: &Event<AdrIngestedEvent>) -> std::iter::Once<String> {
    std::iter::once(event.domain_event().domain_key())
}
