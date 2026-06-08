use crate::error::PublishError;
use std::num::NonZeroU64;
pub trait FrontierPublisher: std::fmt::Debug + Send + 'static {
    /// Deliver one anchor `(subject, payload)` pair.
    ///
    /// Failure is non-fatal: `Dragline::sync_data_with_source` re-buffers the
    /// offending anchor (and any later-in-order pending anchors)
    /// for retry on the next `sync_data` (ADR-0015 D3). Local
    /// durability is fenced before any `publish` call (ADR-0015 D2).
    ///
    /// # Errors
    ///
    /// `PublishError::Transport` / `Closed` / `Custom` per the
    /// adopter; runtime only checks success vs. failure.
    ///
    /// # Panics
    ///
    /// Illegal — panics propagate out of `Dragline::sync_data_with_source` and
    /// terminate the journal thread (ADR-0015 D4).
    fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError>;
}
/// Forwarding impl so adopters constructing a publisher box on the
/// public seam (`pardosa::store::FrontierPublisher`,
/// `EventStore::open_with_publisher`) can pass `Box<dyn FrontierPublisher>`
/// where the substrate's generic ctors require `P: FrontierPublisher`.
/// The box is itself `Send + 'static` (trait object inherits the trait
/// supertraits per ADR-0014 F5), so `Send` propagates through.
impl FrontierPublisher for Box<dyn FrontierPublisher> {
    fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError> {
        (**self).publish(subject, payload)
    }
}
/// Rolling BLAKE3 frontier — 32 bytes summarising a Dragline's
/// event line.
///
/// Genesis = all-zero. `roll` chains the next `event_bytes` into
/// the digest. The `[u8; 32]` survives only via `as_bytes`; the
/// newtype forces opt-in to the rolling-hash invariant.
///
/// # Security
///
/// Mutation detection vs. a trusted anchor, not authentication.
/// Unkeyed BLAKE3 over `previous_frontier || to_vec(event)`. An
/// adversary with `.pgno` write access can rewrite and recompute a
/// consistent chain; divergence detection holds only with an
/// out-of-band anchor (ADR-0004 § Security model).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Frontier([u8; 32]);
impl Frontier {
    /// Genesis frontier (all zeros) — value before any event has rolled in.
    pub const GENESIS: Frontier = Frontier([0u8; 32]);
    #[must_use]
    pub fn new() -> Self {
        Self::GENESIS
    }
    /// Chain `event_bytes` into the rolling BLAKE3 digest and return the new frontier.
    #[must_use]
    pub fn roll(self, event_bytes: &[u8]) -> Frontier {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.0);
        hasher.update(event_bytes);
        Frontier(hasher.finalize().into())
    }
    /// View the underlying 32-byte digest. Pointer borrow, not a copy — callers
    /// that need to publish or compare bytes should use this and avoid the
    /// implicit promotion back to `[u8; 32]` where possible.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl Default for Frontier {
    fn default() -> Self {
        Self::GENESIS
    }
}
/// Anchor interval — non-zero by construction; collapses the silent `.max(1)`
/// clamp the pre-newtype `with_publisher` carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnchorInterval(NonZeroU64);
impl AnchorInterval {
    /// One-tick anchor — the most aggressive publish cadence.
    /// Test-only constant; production code constructs via
    /// [`AnchorInterval::try_new`] or [`AnchorInterval::new_or_one`].
    #[cfg(test)]
    pub const ONE: AnchorInterval = AnchorInterval(NonZeroU64::MIN);
    /// Construct from `u64`; `None` if `v == 0`.
    #[must_use]
    pub fn try_new(v: u64) -> Option<Self> {
        NonZeroU64::new(v).map(AnchorInterval)
    }
    /// Construct from `u64`, falling back to `ONE` when zero. The name is
    /// chosen so the fallback is visible to readers: zero means "publish every
    /// event", not a hidden clamp.
    #[must_use]
    pub fn new_or_one(v: u64) -> Self {
        AnchorInterval(NonZeroU64::new(v).unwrap_or(NonZeroU64::MIN))
    }
    #[must_use]
    pub fn get(self) -> u64 {
        self.0.get()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontier_genesis_is_zero() {
        assert_eq!(Frontier::GENESIS.as_bytes(), &[0u8; 32]);
        assert_eq!(Frontier::new().as_bytes(), &[0u8; 32]);
    }
    #[test]
    fn frontier_roll_changes_value() {
        let f0 = Frontier::new();
        let f1 = f0.roll(b"event-1");
        assert_ne!(f0, f1);
    }
    #[test]
    fn frontier_roll_is_deterministic() {
        let a = Frontier::new().roll(b"x").roll(b"y");
        let b = Frontier::new().roll(b"x").roll(b"y");
        assert_eq!(a, b);
    }
    #[test]
    fn frontier_roll_order_dependent() {
        let a = Frontier::new().roll(b"x").roll(b"y");
        let b = Frontier::new().roll(b"y").roll(b"x");
        assert_ne!(a, b, "rolling order must affect digest (chain integrity)");
    }
    #[test]
    fn anchor_interval_rejects_zero() {
        assert!(AnchorInterval::try_new(0).is_none());
        assert_eq!(AnchorInterval::try_new(1).unwrap().get(), 1);
        assert_eq!(AnchorInterval::try_new(1_000).unwrap().get(), 1_000);
    }
    #[test]
    fn anchor_interval_new_or_one_collapses_zero_to_one() {
        assert_eq!(AnchorInterval::new_or_one(0).get(), 1);
        assert_eq!(AnchorInterval::new_or_one(5).get(), 5);
    }
}
/// Narrow JetStream-backed [`FrontierPublisher`] adapter.
///
/// Wraps a [`pardosa_nats::JetStreamHandle`] and forwards each
/// `(subject, payload)` to
/// [`pardosa_nats::JetStreamHandle::append`].
///
/// Subject contract: the handle is bound to one subject
/// (Phase 1.5 §7); mismatch surfaces as `PublishError::Custom`,
/// not silently rerouted.
///
/// # Errors
///
/// Every [`pardosa_nats::JetStreamRuntimeError`] maps to
/// `PublishError::Custom { source: Box::new(err) }`. ADR-0015 §D2.
///
/// # Panics
///
/// None — publisher panic prohibition (ADR-0015 §D4).
#[derive(Debug)]
pub struct JetStreamFrontierPublisher {
    handle: pardosa_nats::JetStreamHandle,
}
impl JetStreamFrontierPublisher {
    /// Construct from an already-built
    /// [`pardosa_nats::JetStreamHandle`]. No network access;
    /// the handle is captured verbatim. The first network call
    /// happens on the first
    /// [`FrontierPublisher::publish`] (substrate-driven).
    #[must_use]
    pub fn open(handle: pardosa_nats::JetStreamHandle) -> Self {
        Self { handle }
    }
    /// Deprecated alias for [`JetStreamFrontierPublisher::open`];
    /// kept for `SemVer` compatibility with the pre-0.5 naming.
    #[must_use]
    #[deprecated(
        since = "0.5.0",
        note = "renamed to `open` for parity with JetStreamBackend::open and PgnoBackend::open; use ::open instead"
    )]
    pub fn new(handle: pardosa_nats::JetStreamHandle) -> Self {
        Self::open(handle)
    }
}
#[derive(Debug)]
struct SubjectMismatch {
    expected: String,
    actual: String,
}
impl std::fmt::Display for SubjectMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JetStream handle subject mismatch: configured \"{}\", caller asked \"{}\"",
            self.expected, self.actual,
        )
    }
}
impl std::error::Error for SubjectMismatch {}
impl FrontierPublisher for JetStreamFrontierPublisher {
    fn publish(&mut self, subject: &str, payload: &[u8]) -> Result<(), PublishError> {
        let configured = self.handle.config().subject();
        if configured != subject {
            return Err(PublishError::Custom {
                source: Box::new(SubjectMismatch {
                    expected: configured.to_owned(),
                    actual: subject.to_owned(),
                }),
            });
        }
        match self.handle.append(payload) {
            Ok(_seq) => Ok(()),
            Err(err) => Err(PublishError::Custom {
                source: Box::new(err),
            }),
        }
    }
}
