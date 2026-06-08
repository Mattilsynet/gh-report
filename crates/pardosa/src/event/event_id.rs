use super::IndexTooLargeForUsize;
use crate::error::PardosaError;
use pardosa_schema::{GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode, EventSafe};
use serde::{Deserialize, Serialize};
use std::fmt;
/// Monotonic, per-`Dragline` event sequence number.
///
/// Wire bytes are byte-equivalent to the inner `u64` so the field-type
/// promotion is invisible to persisted GENOME records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
#[repr(transparent)]
pub struct EventId(u64);
impl EventId {
    /// The literal zero `EventId`. Not a sentinel — the substrate's
    /// first commit produces `EventId(0)` and dragline state initialises
    /// `next_event_id` to this value (see `dragline::state`). Adopters
    /// observe `EventId::ZERO` as a valid, encodable, persistable id.
    ///
    /// For "no acked offset yet" the cursor API uses `Option<EventId>`
    /// returning `None`; do not overload `ZERO` for absent watermarks.
    /// Pinned by `tests/h4_api_semantics.rs::event_id_zero_is_zero`.
    pub const ZERO: EventId = EventId(0);
    /// Construct an `EventId` from a raw `u64`.
    ///
    /// Removed from the default-feature public API (ADR-0017 §D1):
    /// `EventId` is the substrate's monotonic per-dragline sequence
    /// number. Legitimate mint paths are [`EventId::checked_next`]
    /// (write-path) and the internal [`Decode`] impl. External
    /// callers must not fabricate values; the commit pipeline issues
    /// them. Available under `feature = "test-support"` (and
    /// `cfg(test)`) for raw / tamper fixtures and adopter-facing
    /// CLI / debug tooling.
    #[must_use]
    #[cfg(any(test, feature = "test-support"))]
    pub fn new(v: u64) -> Self {
        EventId(v)
    }
    /// Substrate-internal constructor used by the [`Decode`] impl
    /// (and by the persistence rebuild / publish-watermark paths).
    /// The one legitimate raw-`u64` entry point on the default
    /// feature set.
    #[must_use]
    pub(crate) fn from_decoded(v: u64) -> Self {
        EventId(v)
    }
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
    /// Return the next `EventId` value.
    ///
    /// # Errors
    /// Returns `PardosaError::EventIdOverflow` when incrementing would overflow `u64`.
    pub fn checked_next(self) -> Result<EventId, PardosaError> {
        self.0
            .checked_add(1)
            .map(EventId)
            .ok_or(PardosaError::EventIdOverflow)
    }
}
impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
/// `From<u64>` is gated behind `feature = "test-support"` (and
/// `cfg(test)` inside the crate) so the default-feature public API
/// does not expose a raw-`u64` mint entry. See [`EventId::new`].
#[cfg(any(test, feature = "test-support"))]
impl From<u64> for EventId {
    fn from(v: u64) -> Self {
        EventId(v)
    }
}
impl From<EventId> for u64 {
    fn from(e: EventId) -> u64 {
        e.0
    }
}
/// Substrate-wide invariant: within a single dragline, `EventId`
/// values are dense, monotonic, and zero-based — the `i`-th
/// committed event has `EventId(i)`.
///
/// Established by the commit path
/// (`dragline::state::next_event_id` initialises to `EventId::ZERO`
/// and advances by `+1`); preserved across `.pgno` round-trip by
/// the rehydrate path. New read paths resolving an `EventId` to a
/// `&Event<T>` via the line slice MUST go through this function
/// so the invariant is named at every consumer site (ADR-0003 §1).
pub(crate) fn event_id_to_line_position(id: EventId) -> Result<usize, IndexTooLargeForUsize> {
    usize::try_from(id.value()).map_err(|_| IndexTooLargeForUsize(id.value()))
}
/// Companion to [`event_id_to_line_position`] for the rare call
/// sites that cannot raise a typed conversion error (e.g.
/// in-tree dragline tests where the value is bounded by
/// construction). Returns `None` rather than `Err` so the
/// invariant-name is still visible at the call site without
/// forcing the test author to thread an unused error path.
#[cfg(test)]
#[must_use]
pub(crate) fn event_id_to_line_position_or_panic(id: EventId) -> usize {
    event_id_to_line_position(id).expect("EventId == line position invariant: value fits usize")
}
impl Encode for EventId {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}
impl Decode for EventId {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        u64::decode(d).map(EventId::from_decoded)
    }
}
impl pardosa_wire::sealed::Sealed for EventId {}
impl EventSafe for EventId {}
impl GenomeSafe for EventId {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"EventId");
    const SCHEMA_SOURCE: &'static str = "EventId";
}
