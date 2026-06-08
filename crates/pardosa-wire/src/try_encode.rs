//! Fallible sibling of [`Encode`] for length-prefixed
//! encoders.
//!
//! Mission `rescue-pardosa-b8cb`. Variable-length payloads
//! (`Vec<T>`, `String`, `[u8]`, `BTreeMap`, `BTreeSet`) carry
//! a LE `u32` length prefix. [`Encode::encode`] panics when
//! length exceeds `u32::MAX` — signal that the caller
//! violated `DEFAULT_DECODE_CAP` (1 MiB).
//!
//! Downstream callers building unbounded payloads need a typed
//! error: [`TryEncode`].
//!
//! Doctrine: [`Encode`] stays open per ADR-0014; `encode`
//! keeps `-> ()` (switching breaks public API). [`TryEncode`]
//! is the additive sibling, hand-impl'd for every prefixed
//! type. [`EncodeOverflow`] is alloc-only, mirroring
//! [`DecodeError`](crate::DecodeError). ADR-0009 §additive.
use alloc::vec::Vec;
use core::fmt;
/// Encoding failed because a length-prefixed payload's `usize` length
/// exceeds the wire protocol's `u32` LE length prefix.
///
/// The infallible [`Encode::encode`](crate::Encode::encode) path
/// panics on this condition; [`TryEncode::try_encode`] returns this
/// error instead. The carried [`len`](Self::len) is the offending
/// `usize` value the caller attempted to encode.
///
/// See [ADR-0009](../../../docs/adr/0009-semver-policy.md) for the
/// semver classification of this surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct EncodeOverflow {
    /// The `usize` length that exceeded `u32::MAX`.
    pub len: usize,
}
impl fmt::Display for EncodeOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "encode overflow: length {} exceeds u32::MAX wire prefix",
            self.len
        )
    }
}
impl core::error::Error for EncodeOverflow {}
/// Fallible sibling of [`Encode`](crate::Encode) for
/// length-prefixed encoders.
///
/// Implementors mirror [`Encode`](crate::Encode) but route length-prefix
/// emission through [`try_encode_len_prefix`], returning
/// [`EncodeOverflow`] on `usize`-to-`u32` overflow instead of
/// panicking.
///
/// Open trait (ADR-0014); downstream may add fallible encoders.
///
/// # Errors
/// Implementors must return [`EncodeOverflow`] iff a length
/// prefix cannot fit in `u32`. No-prefix types (primitives,
/// fixed-size tuples) should not impl this — use [`Encode`](crate::Encode).
pub trait TryEncode {
    /// Append the fallible encoding of `self` to `out`.
    ///
    /// # Errors
    /// Returns [`EncodeOverflow`] when any length prefix this
    /// encoder would emit exceeds `u32::MAX`. On error, `out` is
    /// left in an unspecified intermediate state — callers must
    /// either discard `out` or truncate it to a pre-call length
    /// snapshot.
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow>;
}
/// Append a `usize` length as the wire-protocol `u32` little-endian
/// length prefix, returning [`EncodeOverflow`] on overflow.
///
/// Fallible mirror of the in-crate `composites::encode_len_prefix`
/// helper. Public so downstream adopters writing their own
/// length-prefixed [`TryEncode`] impls can reuse the canonical
/// prefix-emission discipline.
///
/// # Errors
/// Returns `EncodeOverflow { len }` if `len > u32::MAX`.
pub fn try_encode_len_prefix(len: usize, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
    let prefix = u32::try_from(len).map_err(|_| EncodeOverflow { len })?;
    out.extend_from_slice(&prefix.to_le_bytes());
    Ok(())
}
/// Encode `value` into a fresh `Vec<u8>` via [`TryEncode`],
/// returning [`EncodeOverflow`] if any length prefix exceeds
/// `u32::MAX`.
///
/// Fallible mirror of [`to_vec`](crate::to_vec). On error the
/// internal buffer is dropped; the caller observes a typed
/// `Err(EncodeOverflow)` rather than a panic.
///
/// # Errors
/// Forwards any [`EncodeOverflow`] raised by `T::try_encode`.
pub fn try_to_vec<T: TryEncode + ?Sized>(value: &T) -> Result<Vec<u8>, EncodeOverflow> {
    let mut buf = Vec::new();
    value.try_encode(&mut buf)?;
    Ok(buf)
}
#[cfg(test)]
mod tests {
    use super::{EncodeOverflow, try_encode_len_prefix, try_to_vec};
    use crate::{Encode, to_vec};
    use alloc::collections::{BTreeMap, BTreeSet};
    use alloc::string::{String, ToString};
    use alloc::vec;
    use alloc::vec::Vec;
    /// Boundary: `u32::MAX` is representable; the helper emits the
    /// canonical 4-byte LE prefix and returns `Ok(())`.
    #[test]
    fn try_encode_len_prefix_accepts_u32_max() {
        let mut out = Vec::new();
        try_encode_len_prefix(u32::MAX as usize, &mut out).expect("u32::MAX fits");
        assert_eq!(out, u32::MAX.to_le_bytes());
    }
    /// Boundary: one above `u32::MAX` returns `EncodeOverflow`
    /// carrying the offending `usize` and writes nothing.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn try_encode_len_prefix_rejects_over_u32_max() {
        let mut out = Vec::new();
        let bad = (u32::MAX as usize) + 1;
        let err = try_encode_len_prefix(bad, &mut out).unwrap_err();
        assert_eq!(err, EncodeOverflow { len: bad });
        assert!(out.is_empty(), "no bytes written on overflow");
    }
    /// `try_to_vec` on a representable `Vec<u8>` produces
    /// byte-identical output to `to_vec`.
    #[test]
    fn try_to_vec_vec_u8_byte_identical_to_to_vec() {
        let v: Vec<u8> = vec![0xAA, 0xBB, 0xCC];
        let fallible = try_to_vec(&v).expect("under cap");
        let infallible = to_vec(&v);
        assert_eq!(fallible, infallible);
    }
    /// `try_to_vec` on a representable `String` produces
    /// byte-identical output to `to_vec`.
    #[test]
    fn try_to_vec_string_byte_identical_to_to_vec() {
        let s = String::from("hello, world");
        assert_eq!(try_to_vec(&s).expect("under cap"), to_vec(&s));
    }
    /// `try_to_vec` on a `[u8]` slice produces byte-identical output
    /// to `to_vec`.
    #[test]
    fn try_to_vec_slice_byte_identical_to_to_vec() {
        let s: &[u8] = &[1u8, 2, 3, 4, 5];
        assert_eq!(try_to_vec(s).expect("under cap"), to_vec(&s));
    }
    /// `try_to_vec` on `BTreeMap` preserves canonical-byte ordering
    /// and matches `to_vec` byte-for-byte.
    #[test]
    fn try_to_vec_btreemap_canonical_and_byte_identical() {
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        m.insert(String::from("alpha"), 1);
        m.insert(String::from("beta"), 2);
        m.insert(String::from("gamma"), 3);
        assert_eq!(try_to_vec(&m).expect("under cap"), to_vec(&m));
    }
    /// `try_to_vec` on `BTreeSet` preserves canonical-byte ordering
    /// and matches `to_vec` byte-for-byte.
    #[test]
    fn try_to_vec_btreeset_canonical_and_byte_identical() {
        let mut s: BTreeSet<u32> = BTreeSet::new();
        s.insert(1);
        s.insert(2);
        s.insert(3);
        assert_eq!(try_to_vec(&s).expect("under cap"), to_vec(&s));
    }
    /// `try_to_vec` on an empty `Vec` produces the 4-byte zero
    /// prefix, byte-identical to `to_vec`.
    #[test]
    fn try_to_vec_empty_vec_zero_prefix() {
        let v: Vec<u32> = Vec::new();
        assert_eq!(
            try_to_vec(&v).expect("trivially under cap"),
            vec![0, 0, 0, 0]
        );
    }
    /// `EncodeOverflow` carries the offending `usize` and `Display`s
    /// usefully.
    #[test]
    fn encode_overflow_display() {
        let e = EncodeOverflow { len: 0x1_0000_0000 };
        assert!(e.to_string().contains("encode overflow: length 4294967296"));
    }
    /// `EncodeOverflow` implements `core::error::Error`.
    #[test]
    fn encode_overflow_impls_core_error() {
        fn assert_impl<T: core::error::Error>() {}
        assert_impl::<EncodeOverflow>();
    }
    /// A nested `Vec<Vec<u8>>` under the cap encodes through the
    /// fallible path and matches the infallible bytes exactly.
    #[test]
    fn try_to_vec_nested_vec_byte_identical() {
        let v: Vec<Vec<u8>> = vec![vec![1, 2], vec![], vec![3, 4, 5]];
        assert_eq!(try_to_vec(&v).expect("under cap"), to_vec(&v));
    }
    /// Sanity: the existing infallible `Encode::encode` still
    /// returns `()` (preserves ADR-0014 open trait shape — no
    /// signature change).
    #[test]
    fn encode_signature_unchanged() {
        let v: Vec<u8> = vec![1, 2, 3];
        let mut out = Vec::new();
        v.encode(&mut out);
    }
}
