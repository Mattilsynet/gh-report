use pardosa_schema::{GenomeSafe, schema_hash_bytes, schema_hash_combine};
pub(crate) mod envelope;
pub(crate) mod event_id;
pub(crate) mod fiber_id;
pub(crate) mod index;
pub(crate) mod precursor;
pub use envelope::Event;
pub use event_id::EventId;
pub(crate) use event_id::event_id_to_line_position;
#[cfg(test)]
pub(crate) use event_id::event_id_to_line_position_or_panic;
pub use fiber_id::FiberId;
pub use index::{Index, IndexTooLargeForUsize};
pub use precursor::Precursor;
/// Structural-shape failure for an `Event<T>` envelope, detectable
/// without consulting any other event in the line.
///
/// This is the per-envelope half of the validation surface; cross-event
/// invariants (precursor-index bounds, same-fiber precursor, precursor
/// hash chain) belong to the replay-time checked stream
/// (`persist::stream_checked` / `persist::CheckedReplayKind`).
///
/// Returned by `Event::try_new` (test/test-support-only constructor)
/// and [`Event::validate_envelope`], and
/// surfaced as the `Validate` impl's error type on `Event<T>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EnvelopeError {
    /// `Precursor::Genesis` events have no in-line predecessor; the
    /// substrate writes `[0u8; 32]` into `precursor_hash` for every
    /// Genesis event (see `dragline::write::create` and the
    /// `Locked → Rescue` arm in `rescue`). A decoded or
    /// adopter-fabricated envelope with `Precursor::Genesis` but a
    /// non-zero `precursor_hash` either originates from a buggy
    /// producer or a tampered byte stream; in either case it should
    /// not enter replay.
    #[error(
        "envelope has Precursor::Genesis but precursor_hash is non-zero (got {hash:?}); Genesis envelopes must have a zero precursor_hash"
    )]
    GenesisHasNonZeroPrecursorHash { hash: [u8; 32] },
}
/// Schema-hash of the `Event<T>` envelope tuple
/// `(EventId, FiberId, bool, Precursor, [u8; 32])`.
///
/// Folded left-to-right via `schema_hash_combine` from
/// `b"EventEnvelope"`, mirroring `pardosa-schema`'s tuple impls.
/// Composed with `T::SCHEMA_HASH` into `Event::<T>::ENVELOPE_HASH`,
/// stored in the `.pgno` schema-hash slot (ADR-0005 / ADR-0006).
/// Field-set, type, or order changes surface as
/// `persist::Error::SchemaHashMismatch`.
///
/// ADR-0013 removed `Timestamp` from the envelope. F1 (mission
/// `f1-sentinel-removal-20260524`): slot folds
/// `Precursor::SCHEMA_HASH`; `Index(u64::MAX)` is a legal value.
pub(crate) const ENVELOPE_SHAPE_HASH: u128 = {
    let h = schema_hash_bytes(b"EventEnvelope");
    let h = schema_hash_combine(h, EventId::SCHEMA_HASH);
    let h = schema_hash_combine(h, FiberId::SCHEMA_HASH);
    let h = schema_hash_combine(h, <bool as GenomeSafe>::SCHEMA_HASH);
    let h = schema_hash_combine(h, Precursor::SCHEMA_HASH);
    schema_hash_combine(h, <[u8; 32] as GenomeSafe>::SCHEMA_HASH)
};
#[cfg(test)]
mod tests;
