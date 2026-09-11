//! Native pardosa `.pgno` store port for adr-srv (CHE-0098 N-R1/N-R4/N-R5).
//!
//! One pardosa fiber per ADR-file aggregate (CHE-0005:R1), keyed by
//! [`AdrIngestedEvent::domain_key`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pardosa::prelude::*;

use crate::domain::native_event::AdrIngestedEvent;

/// Native store error taxonomy.
#[derive(Debug, thiserror::Error)]
pub enum NativeStoreError {
    #[error("pardosa infrastructure error: {0}")]
    Infrastructure(String),
    #[error("concurrency conflict: {0}")]
    ConcurrencyConflict(String),
    #[error("domain key {key:?} maps to multiple fibers")]
    DivergedFiber { key: String },
    #[error("store mutex poisoned")]
    Poisoned,
}

impl From<OperationFailure> for NativeStoreError {
    fn from(err: OperationFailure) -> Self {
        NativeStoreError::Infrastructure(err.to_string())
    }
}

impl From<EncodeError> for NativeStoreError {
    fn from(err: EncodeError) -> Self {
        NativeStoreError::Infrastructure(err.to_string())
    }
}

impl From<DecodeError> for NativeStoreError {
    fn from(err: DecodeError) -> Self {
        NativeStoreError::Infrastructure(err.to_string())
    }
}

fn default_claim(epoch: u64) -> OwnershipClaimRecord {
    OwnershipClaimRecord {
        epoch,
        machine_id: [0u8; 16],
        boot_id: [0u8; 16],
        process_id: u64::from(std::process::id()),
        process_start_time_ns: 0,
        claim_time_ns: 0,
        operator_label: "adr-srv".to_string(),
    }
}

/// Pardosa-native `AdrIngested` event store: one fiber per ADR file.
pub struct NativeAdrStore {
    session: Mutex<FileWriterSession>,
    path: PathBuf,
}

impl NativeAdrStore {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    /// Returns [`NativeStoreError::Infrastructure`] when pardosa cannot
    /// create the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, NativeStoreError> {
        let adapter = FileStorageAdapter::new(path);
        let _ = std::fs::remove_file(adapter.meta_path());
        let _ = std::fs::remove_file(adapter.pgno_path());
        let claim = default_claim(1);
        let session = adapter.create(&claim)?;
        Ok(Self {
            session: Mutex::new(session),
            path: path.to_path_buf(),
        })
    }

    /// Open an existing `.pgno`-backed store, resuming every already-
    /// `Defined` fiber (N-R5 boot contract).
    ///
    /// # Errors
    /// Returns [`NativeStoreError::Infrastructure`] when pardosa cannot
    /// open or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, NativeStoreError> {
        let adapter = FileStorageAdapter::new(path);
        let epoch = adapter.current_epoch()?;
        let session = adapter.open_write(epoch)?;
        Ok(Self {
            session: Mutex::new(session),
            path: path.to_path_buf(),
        })
    }

    /// Returns the backing store path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Capture an `AdrIngested` native event onto its ADR file's fiber
    /// (first observation begins the fiber; N-R5 resumes it thereafter).
    ///
    /// # Errors
    /// Returns [`NativeStoreError::Infrastructure`] on pardosa append/sync
    /// failure, or [`NativeStoreError::Poisoned`].
    pub fn record(&self, event: &AdrIngestedEvent) -> Result<(), NativeStoreError> {
        let key = event.domain_key();
        let fiber_id = derive_fiber_id(&key);
        let mut payload = Vec::new();
        event.encode_payload(&mut payload)?;
        let event_id = *uuid::Uuid::now_v7().as_bytes();

        let mut session = self
            .session
            .lock()
            .map_err(|_| NativeStoreError::Poisoned)?;
        let handle = session.fiber(fiber_id)?;
        if handle.is_detached() {
            session.rescue_fiber(fiber_id, event_id, payload)?;
        } else {
            session.append_to_fiber(fiber_id, event_id, payload)?;
        }
        session.sync()?;
        Ok(())
    }

    /// Every event in committed line order, paired with the pardosa
    /// envelope `detached` flag — the rebuild-from-corpus replay input
    /// (N-R5).
    ///
    /// # Errors
    /// Returns [`NativeStoreError::Infrastructure`] on pardosa read
    /// failure or [`NativeStoreError::Poisoned`].
    pub fn events(&self) -> Result<Vec<(bool, AdrIngestedEvent)>, NativeStoreError> {
        let mut session = self
            .session
            .lock()
            .map_err(|_| NativeStoreError::Poisoned)?;
        let envelopes = session.read_all_envelopes()?;
        let mut result = Vec::with_capacity(envelopes.len());
        for env in envelopes {
            let event = AdrIngestedEvent::decode_payload(&env.payload)?;
            result.push((env.header.detached, event));
        }
        Ok(result)
    }
}
