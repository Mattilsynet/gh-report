use crate::{DEFAULT_DECODE_CAP, DecodeError, Decoder, sealed};
use alloc::vec::Vec;
/// Event-safety marker trait: types implementing this are safe to use in event payloads.
///
/// # Sealing
/// Strong-sealed via the private supertrait [`sealed::Sealed`] per
/// [ADR-0014](../../../docs/adr/0014-sealed-trait-policy.md). Downstream
/// crates cannot satisfy `Sealed` and therefore cannot implement `EventSafe`,
/// even if they implement [`Encode`]. The only in-tree path to extend the
/// event-safe set is via `#[derive(GenomeSafe)]` (which emits both `Sealed`
/// and `EventSafe`) or an explicit workspace-internal `impl sealed::Sealed`
/// next to the `impl EventSafe`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not `EventSafe`",
    label = "needs `#[derive(GenomeSafe)]` or a bounded wrapper",
    note = "Only types blessed by `#[derive(GenomeSafe)]` or workspace-internal impls may implement `EventSafe`. See GEN-0036 and Solon doctrine (ADOPTION.md, 16 Rules).",
    note = "Common substitutions: `String` → `EventString<MAX>` / `NonEmptyEventString<MAX>`; `Vec<T>` → `EventVec<T, MAX>`; `Vec<u8>` → `EventBytes<MAX>` (all from `pardosa_schema::bounded`).",
    note = "Timestamps: use `pardosa_wire::Timestamp` (nonzero-u64 nanos since UNIX epoch).",
    note = "Map- and set-shaped payloads (`HashMap`/`BTreeMap`/`HashSet`/`BTreeSet`) are forbidden by Solon doctrine (STORY §4.2): enumerate keys as named struct fields or an enum, or store the map outside the event store."
)]
pub trait EventSafe: sealed::Sealed {}
pub trait Encode {
    fn encode(&self, out: &mut Vec<u8>);
}
pub trait Decode: Sized {
    /// Decode an instance of `Self` from the cursor.
    ///
    /// # Errors
    /// Returns a `DecodeError` if the byte sequence is malformed, truncated, or violates
    /// the wire contract (out-of-range tag, length overflow, invalid input, etc.).
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError>;
}
#[must_use]
pub fn to_vec<T: Encode>(value: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    value.encode(&mut buf);
    buf
}
/// Decode a `T` from `input` using the default decode cap.
///
/// # Errors
/// Forwards any `DecodeError` raised by `T::decode`, plus `DecodeError::TrailingBytes`
/// if `input` is not fully consumed.
pub fn from_bytes<T: Decode>(input: &[u8]) -> Result<T, DecodeError> {
    from_bytes_with_cap(input, DEFAULT_DECODE_CAP)
}
/// Decode a `T` from `input` with an explicit length-prefix cap.
///
/// # Errors
/// Forwards any `DecodeError` raised by `T::decode`, plus `DecodeError::TrailingBytes`
/// if `input` is not fully consumed.
pub fn from_bytes_with_cap<T: Decode>(input: &[u8], cap: usize) -> Result<T, DecodeError> {
    let mut d = Decoder::with_cap(input, cap);
    let value = T::decode(&mut d)?;
    if !d.is_at_end() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(value)
}
#[cfg(test)]
mod tests {
    use crate::{DecodeError, from_bytes};
    #[test]
    fn trailing_bytes_rejected() {
        let err = from_bytes::<u32>(&[1, 0, 0, 0, 0xFF]).unwrap_err();
        assert_eq!(err, DecodeError::TrailingBytes);
    }
    #[test]
    fn unexpected_eof() {
        let err = from_bytes::<u32>(&[1, 2]).unwrap_err();
        assert_eq!(err, DecodeError::BufferUnderflow);
    }
}
