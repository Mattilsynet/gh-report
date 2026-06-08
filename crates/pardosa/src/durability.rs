//! Durability boundary types — runtime layer (ADR-0010).
//!
//! [`Lsn(u64)`](Lsn) is the public shape downstream callers pattern-match
//! on. It lives in `pardosa` (runtime), not `pardosa-file` (substrate):
//! the substrate carries `Syncable::sync_data`; the runtime ring composes
//! that into an observable "this event survived fsync" commitment
//! (ADR-0002, ADR-0004). The adopter-facing observation point is
//! [`crate::store::StoreWriter::acked_lsn`] (post-`sync`).
/// Opaque sequence number naming a durable point in the journal.
///
/// Semantically a byte offset into the journal stream after a
/// successful fsync (ADR-0010). The newtype is opaque: concrete
/// semantics (byte offset vs. message count vs. other monotone)
/// belong to the journal format and can evolve without breaking
/// callers, who treat `Lsn` as a black-box token for "everything
/// up to here is durable."
///
/// `Copy` + `Ord`: monotonic comparison is the only operation
/// callers need against a token they did not mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
#[repr(transparent)]
pub struct Lsn(u64);
impl Lsn {
    /// Construct an `Lsn` from a u64 monotone position.
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
    /// Extract the underlying u64. Provided for diagnostics, logging,
    /// and serialisation; not for arithmetic. The journal layer remains
    /// the only authoritative source of `Lsn` values.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}
/// Opaque positional marker into a backend's append stream
/// (ADR-0022 §D2). Returned by `BackendSink::append` / `sync`.
///
/// # Position vs. durability
///
/// Positional only; durability is earned by `sync` returning the
/// `AckPosition` at which preceding bytes are stable. `.pgno`
/// derives it from the post-fsync offset (same as [`Lsn`]);
/// `JetStream` from `PubAck.seq`. `EventId` ↔ `AckPosition`
/// mapping is per backend (ADR-0022 §D3).
///
/// # Cross-instance ordering undefined
///
/// Monotonic within one instance. Values from different instances
/// or across restart compare as `u64` but carry no durability
/// meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[must_use]
#[repr(transparent)]
pub struct AckPosition(u64);
impl AckPosition {
    /// Construct an `AckPosition` from a backend-supplied u64
    /// monotone position. `pub(crate)`: only in-crate
    /// `AuthoritativeBackend` adapter wrappers (ADR-0022 §D11
    /// sealed-trait + in-crate adapter pattern) mint values;
    /// sibling-crate backend handles surface positions through the
    /// sealed trait's `append`/`sync` paths only.
    pub(crate) const fn from_u64(value: u64) -> Self {
        Self(value)
    }
    /// Extract the underlying u64. Provided for diagnostics,
    /// logging, and persistence of backend-local position metadata
    /// (e.g. ADR-0011 §D5 sidecar / ADR-0016 §D5 publish watermark);
    /// not for arithmetic. The backend's append stream remains the
    /// only authoritative source of `AckPosition` values.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ack_position_orders_within_instance() {
        let earlier = AckPosition::from_u64(5);
        let later = AckPosition::from_u64(7);
        assert!(earlier < later);
        assert_eq!(earlier.as_u64(), 5);
        assert_eq!(later.as_u64(), 7);
    }
    #[test]
    fn lsn_and_ack_position_carry_independent_semantics() {
        let lsn = Lsn::new(42);
        let ack = AckPosition::from_u64(42);
        assert_eq!(lsn.value(), ack.as_u64());
    }
}
