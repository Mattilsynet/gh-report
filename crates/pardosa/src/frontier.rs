//! `FrontierPublisher` trait and in-memory mock implementation.
//!
//! PAR-0021:R4 requires publishing the current `Dragline::frontier` value to
//! `pardosa.{stream}.frontier` on every `anchor_interval` tick.
//!
//! # Production wiring
//!
//! The production NATS implementation is deferred to Phase 3 (SEC-0010).
//! When that work lands, implement `FrontierPublisher` for an async-nats
//! client wrapper and wire it into `Dragline::with_publisher`. No `async-nats`
//! dependency is introduced in this engagement.
//!
//! # Design
//!
//! The trait is synchronous (`publish` is not `async`) so the production path
//! can wrap a channel-based bridge (fire-and-forget into an async task) without
//! forcing the Dragline call sites to be async. The subject string and 32-byte
//! frontier payload are passed by reference; the implementation owns buffering.

use std::sync::{Arc, Mutex};

/// Shared log storage for [`InMemoryFrontierPublisher`]: `(subject, payload)` pairs.
type PublishLog = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// Publish a frontier snapshot to an external transparency anchor.
///
/// Implementors must be cheaply cloneable (`Clone + Send + 'static`) so the
/// Dragline can hand a copy to the caller for inspection in tests.
///
/// The subject is always `pardosa.{stream}.frontier` (PAR-0021:R4).
pub trait FrontierPublisher: std::fmt::Debug + Send + 'static {
    /// Publish `payload` to `subject`. Called once per `anchor_interval` tick.
    ///
    /// Publish failures are advisory in this phase — callers log and continue;
    /// Dragline integrity does not depend on delivery acknowledgment.
    fn publish(&mut self, subject: &str, payload: &[u8]);
}

// ── Frontier rolling ─────────────────────────────────────────────────────────

/// Compute the next frontier value: BLAKE3(current_frontier || event_bytes).
///
/// The concatenation order ensures each new frontier value is a function of
/// both the full prior chain summary (`current_frontier`) and the new event
/// bytes, making any in-order modification of events detectable.
#[must_use]
pub fn frontier_roll(current: [u8; 32], event_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&current);
    hasher.update(event_bytes);
    hasher.finalize().into()
}

/// In-memory `FrontierPublisher` that records every publish call.
///
/// Used by integration tests to assert that `pardosa.{stream}.frontier`
/// receives the expected frontier bytes on each `anchor_interval` tick.
/// Not suitable for production.
#[derive(Clone, Debug)]
pub struct InMemoryFrontierPublisher {
    log: PublishLog,
}

impl InMemoryFrontierPublisher {
    /// Create a new empty mock publisher.
    #[must_use]
    pub fn new() -> Self {
        Self {
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Return a snapshot of all `(subject, payload)` pairs published so far.
    #[must_use]
    pub fn published(&self) -> Vec<(String, Vec<u8>)> {
        self.log.lock().expect("mutex not poisoned").clone()
    }
}

impl Default for InMemoryFrontierPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontierPublisher for InMemoryFrontierPublisher {
    fn publish(&mut self, subject: &str, payload: &[u8]) {
        self.log
            .lock()
            .expect("mutex not poisoned")
            .push((subject.to_owned(), payload.to_vec()));
    }
}
