//! `Encode` / `Decode` trait surface and the top-level `to_vec` /
//! `from_bytes` / `from_bytes_with_cap` free functions.
//!
//! GEN-0035 §"Wire surface". The traits are open at this stage; sealing
//! is deferred to sub-mission A2 (sealed `EventSafe` parent trait).

use alloc::vec::Vec;

use crate::{DEFAULT_DECODE_CAP, Decoder, EventError};

/// Encode a value to its canonical byte representation.
///
/// Sealing is deferred to sub-mission A2 (sealed `EventSafe` parent
/// trait). At this stage the trait is open; A2 will introduce
/// `EventSafe: seal::Sealed` and bound `Encode`/`Decode` on it.
pub trait Encode {
    /// Append the value's canonical encoding to `out`.
    fn encode(&self, out: &mut Vec<u8>);
}

/// Decode a value from a [`Decoder`] cursor.
pub trait Decode: Sized {
    /// Read one value from the decoder cursor.
    ///
    /// # Errors
    ///
    /// Returns [`EventError`] when the wire bytes are malformed, exceed the
    /// decoder cap, or otherwise fail validation. Specific variants are
    /// implementation-defined.
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError>;
}

/// Encode `value` to a fresh `Vec<u8>`.
#[must_use]
pub fn to_vec<T: Encode>(value: &T) -> Vec<u8> {
    let mut buf = Vec::new();
    value.encode(&mut buf);
    buf
}

/// Decode a value from `input` with the default cap, enforcing strict
/// (no trailing bytes) consumption per GEN-0035:R6.
///
/// # Errors
///
/// Returns [`EventError::InvalidInput`] if the bytes do not decode cleanly
/// to `T`, or if any bytes remain after `T` is read. Propagates errors
/// from `T::decode`.
pub fn from_bytes<T: Decode>(input: &[u8]) -> Result<T, EventError> {
    from_bytes_with_cap(input, DEFAULT_DECODE_CAP)
}

/// Decode a value from `input` with an explicit cap, enforcing strict
/// (no trailing bytes) consumption per GEN-0035:R6.
///
/// # Errors
///
/// Returns [`EventError::InvalidInput`] if the bytes do not decode cleanly
/// to `T`, the decode cap is exceeded, or any bytes remain after `T` is
/// read. Propagates errors from `T::decode`.
pub fn from_bytes_with_cap<T: Decode>(input: &[u8], cap: usize) -> Result<T, EventError> {
    let mut d = Decoder::with_cap(input, cap);
    let value = T::decode(&mut d)?;
    if !d.is_at_end() {
        return Err(EventError::InvalidInput);
    }
    Ok(value)
}
