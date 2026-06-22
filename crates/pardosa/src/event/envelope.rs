use super::{ENVELOPE_SHAPE_HASH, EnvelopeError, EventId, FiberId, Precursor};
use pardosa_schema::{GenomeSafe, schema_hash_combine};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode, Validate};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::struct_field_names,
    reason = "pub field names on prelude-reachable Event<T> are PGN-0012 breaking surface; renaming for clippy taste is deferred under the PGN-0009 clean-break posture and out of scope for a lint sweep"
)]
pub struct Event<T> {
    event_id: EventId,
    fiber_id: FiberId,
    detached: bool,
    precursor: Precursor,
    precursor_hash: [u8; 32],
    domain_event: T,
}
impl<T> Event<T> {
    /// Construct an `Event` envelope from its parts without
    /// per-envelope validation.
    ///
    /// Restricted to substrate write paths (`Dragline::{create,
    /// update, detach, rescue, migrate_fiber}`) which
    /// build envelopes consistent by construction, plus in-crate
    /// tests and the optional `test-support` gate (ADR-0017,
    /// mission `event-trust-split-20260524`). Default-feature
    /// public API exposes no envelope constructor: adopters never
    /// construct an `Event<T>` directly; the writer verbs on
    /// [`crate::store::StoreWriter`] mint envelopes substrate-side
    /// (mission `pardosa-api-hardening-20260604`).
    ///
    /// Accepts `impl Into<EventId>` for callsite ergonomics. See
    /// ADR-0014 for the sealed-trait stance.
    #[cfg(not(any(test, feature = "test-support")))]
    #[must_use]
    pub(crate) fn new_unchecked(
        event_id: impl Into<EventId>,
        fiber_id: FiberId,
        detached: bool,
        precursor: Precursor,
        precursor_hash: [u8; 32],
        domain_event: T,
    ) -> Self {
        Event {
            event_id: event_id.into(),
            fiber_id,
            detached,
            precursor,
            precursor_hash,
            domain_event,
        }
    }
    /// Test-support variant of [`Event::new_unchecked`]: same
    /// construction, broader visibility so integration tests and
    /// adopters under `feature = "test-support"` can fabricate raw or
    /// tampered envelopes (fixtures, proptest harnesses, replay
    /// scaffolding). Mirrors the `pub(crate)` form bit-for-bit; the
    /// cfg split only widens visibility under the gate.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn new_unchecked(
        event_id: impl Into<EventId>,
        fiber_id: FiberId,
        detached: bool,
        precursor: Precursor,
        precursor_hash: [u8; 32],
        domain_event: T,
    ) -> Self {
        Event {
            event_id: event_id.into(),
            fiber_id,
            detached,
            precursor,
            precursor_hash,
            domain_event,
        }
    }
    /// Validating envelope constructor.
    ///
    /// Same field set as `new_unchecked`, but rejects per-envelope
    /// structural violations up front: `Precursor::Genesis` ⟹
    /// `precursor_hash == [0u8; 32]`. Cross-event invariants live
    /// in [`persist::stream_checked`](crate::persist::stream_checked).
    ///
    /// `pub(crate)` because adopter code never constructs an
    /// envelope directly — writer verbs take payload `T` and mint
    /// via `new_unchecked`.
    ///
    /// # Errors
    ///
    /// [`EnvelopeError::GenesisHasNonZeroPrecursorHash`] when
    /// `precursor == Genesis` but `precursor_hash != [0; 32]`.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(
        dead_code,
        reason = "test-support constructor: exercised by the in-crate test module and exposed under the `test-support` feature for downstream test code, so it reads as dead under a non-test --all-features build but is live under --test"
    )]
    pub(crate) fn try_new(
        event_id: impl Into<EventId>,
        fiber_id: FiberId,
        detached: bool,
        precursor: Precursor,
        precursor_hash: [u8; 32],
        domain_event: T,
    ) -> Result<Self, EnvelopeError> {
        check_envelope_shape(precursor, &precursor_hash)?;
        Ok(Event {
            event_id: event_id.into(),
            fiber_id,
            detached,
            precursor,
            precursor_hash,
            domain_event,
        })
    }
    /// Re-check the per-envelope structural-shape invariants enforced
    /// by `Event::try_new` (test/test-support-only). Cheap (constant-time field inspection);
    /// safe to call on a decoded envelope before handing it to a
    /// validator-aware replay path.
    ///
    /// # Errors
    /// Returns [`EnvelopeError`] if the envelope violates a
    /// per-envelope shape invariant.
    pub fn validate_envelope(&self) -> Result<(), EnvelopeError> {
        check_envelope_shape(self.precursor, &self.precursor_hash)
    }
    #[must_use]
    pub fn event_id(&self) -> EventId {
        self.event_id
    }
    #[must_use]
    pub fn fiber_id(&self) -> FiberId {
        self.fiber_id
    }
    #[must_use]
    pub fn detached(&self) -> bool {
        self.detached
    }
    #[must_use]
    pub fn precursor(&self) -> Precursor {
        self.precursor
    }
    #[must_use]
    pub fn precursor_hash(&self) -> [u8; 32] {
        self.precursor_hash
    }
    #[must_use]
    pub fn domain_event(&self) -> &T {
        &self.domain_event
    }
    #[must_use]
    pub fn into_inner(self) -> T {
        self.domain_event
    }
    /// Deprecated alias for [`Event::into_inner`]; kept for `SemVer`
    /// compatibility with the pre-0.5 naming.
    #[must_use]
    #[deprecated(
        since = "0.5.0",
        note = "renamed to `into_inner` for parity with the std consume-and-return convention; use ::into_inner instead"
    )]
    pub fn into_payload(self) -> T {
        self.into_inner()
    }
}
impl<T: GenomeSafe> Event<T> {
    /// Composed schema-hash discriminator written to the `.pgno`
    /// container's header slot (ADR-0005 / ADR-0006). Combines the
    /// payload-type hash with the envelope-shape hash so that any change
    /// to either factor — payload-type evolution *or* envelope-field
    /// reorder/add/remove/wrapper-swap — surfaces as
    /// `persist::Error::SchemaHashMismatch` at `Reader::open`.
    pub const ENVELOPE_HASH: u128 = schema_hash_combine(T::SCHEMA_HASH, ENVELOPE_SHAPE_HASH);
}
impl<T: Encode> Encode for Event<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        self.event_id.encode(out);
        self.fiber_id.encode(out);
        self.detached.encode(out);
        self.precursor.encode(out);
        self.precursor_hash.encode(out);
        self.domain_event.encode(out);
    }
}
impl<T: Decode> Decode for Event<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let event_id = EventId::decode(d)?;
        let fiber_id = FiberId::decode(d)?;
        let detached = bool::decode(d)?;
        let precursor = Precursor::decode(d)?;
        let precursor_hash = <[u8; 32]>::decode(d)?;
        let domain_event = T::decode(d)?;
        Ok(Event {
            event_id,
            fiber_id,
            detached,
            precursor,
            precursor_hash,
            domain_event,
        })
    }
}
/// `Validate` impl for `Event<T>` (o1ix.5 + o1ix.15). Re-checks the
/// per-envelope structural-shape invariants from `Event::try_new`
/// (test/test-support-only).
///
/// The `Validate` trait is `OPEN` (ADR-0014, F6); this impl lets
/// callers fold `Event<T>` envelope validation into the same
/// `Validate::validate()` call shape they use for payload
/// vocabulary types (`EventString`, `EventBytes`, …). Cross-event
/// invariants are out of scope — those belong to
/// `persist::stream_checked`.
impl<T> Validate for Event<T> {
    type Error = EnvelopeError;
    const COST: pardosa_wire::ValidationCost = pardosa_wire::ValidationCost::Cheap;
    fn validate(&self) -> Result<(), EnvelopeError> {
        self.validate_envelope()
    }
}
/// Per-envelope structural-shape check shared by `Event::try_new`
/// (test/test-support-only) and [`Event::validate_envelope`]. Constant-time.
fn check_envelope_shape(
    precursor: Precursor,
    precursor_hash: &[u8; 32],
) -> Result<(), EnvelopeError> {
    if matches!(precursor, Precursor::Genesis) && precursor_hash != &[0u8; 32] {
        return Err(EnvelopeError::GenesisHasNonZeroPrecursorHash {
            hash: *precursor_hash,
        });
    }
    Ok(())
}
