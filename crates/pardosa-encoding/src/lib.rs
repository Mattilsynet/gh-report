//! In-house canonical encoding for pardosa events (GEN-0035).
//!
//! The wire format is a deterministic sequential canonical encoding
//! (LE primitives, length-prefixed variable-width data, `repr(u8)`
//! enum discriminants) owned by the workspace so we control the spec,
//! the sealing, and the decoder cap semantics. This crate provides the
//! substrate ([`Encode`], [`Decode`], [`DecodeError`], primitive impls);
//! the sealed [`EventSafe`]/[`GenomeSafe`]/[`GenomeOrd`] trait stack is
//! introduced separately in sub-mission A2.
//!
//! See `docs/adr/genome/GEN-0035-in-house-canonical-encoding.md`.

#![forbid(unsafe_code)]
#![no_std]

extern crate alloc;

use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;

// ---------------------------------------------------------------------------
// Decoder cap (GEN-0035 §"Decoder cap")
// ---------------------------------------------------------------------------

/// Default per-decode allocation cap: 1 MiB. Matches the F2 (r2)
/// commander decision; a length-prefix header exceeding the remaining
/// budget is rejected before allocation.
pub const DEFAULT_DECODE_CAP: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors raised by [`Decode::decode`].
///
/// Sub-mission C will subsume this enum into the wider 11-variant
/// `EventError` per F4; for now this is the decoder-local surface.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum DecodeError {
    /// Input ended before the expected number of bytes were available.
    UnexpectedEof,
    /// A length or count header exceeded the remaining decode cap; no
    /// allocation was attempted.
    CapExceeded,
    /// `bool` byte was neither `0u8` nor `1u8`.
    InvalidBool,
    /// `Option` discriminant byte was neither `0u8` (None) nor `1u8` (Some).
    InvalidOptionTag,
    /// Sum-type discriminant byte did not match a known variant.
    InvalidDiscriminant,
    /// String length-prefix payload was not valid UTF-8.
    InvalidUtf8,
    /// A `BTreeMap` entry was emitted out of canonical (ascending
    /// encoded-key-bytes) order — duplicate or descending key.
    NonCanonicalMap,
    /// Decode succeeded but input bytes remained.
    TrailingBytes,
}

// ---------------------------------------------------------------------------
// Decoder state
// ---------------------------------------------------------------------------

/// Stateful decoder cursor with a remaining-cap budget.
///
/// The cap is per-decode-invocation: each top-level call constructs a
/// fresh `Decoder` with a fresh budget. Length-prefix headers are
/// validated against the *remaining* budget so nested decoders share
/// the parent cap rather than reset it.
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    cap_remaining: usize,
}

impl<'a> Decoder<'a> {
    /// Construct a decoder with the [`DEFAULT_DECODE_CAP`] budget.
    #[must_use]
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_cap(input, DEFAULT_DECODE_CAP)
    }

    /// Construct a decoder with a caller-supplied byte cap.
    #[must_use]
    pub fn with_cap(input: &'a [u8], cap: usize) -> Self {
        Self {
            input,
            pos: 0,
            cap_remaining: cap,
        }
    }

    /// Read exactly `n` bytes from the cursor, advancing it.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEof)?;
        if end > self.input.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.input[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read a `u32 LE` length header and charge it against the cap.
    /// Returns `CapExceeded` *before* any allocation is attempted.
    pub fn read_len_prefix(&mut self) -> Result<usize, DecodeError> {
        let raw = u32::decode(self)?;
        let n = raw as usize;
        if n > self.cap_remaining {
            return Err(DecodeError::CapExceeded);
        }
        self.cap_remaining -= n;
        Ok(n)
    }

    /// Bytes consumed so far.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Total input length.
    #[must_use]
    pub fn input_len(&self) -> usize {
        self.input.len()
    }

    /// True when no input remains.
    #[must_use]
    pub fn is_at_end(&self) -> bool {
        self.pos >= self.input.len()
    }
}

// ---------------------------------------------------------------------------
// Encode / Decode trait surface
// ---------------------------------------------------------------------------

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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError>;
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
pub fn from_bytes<T: Decode>(input: &[u8]) -> Result<T, DecodeError> {
    from_bytes_with_cap(input, DEFAULT_DECODE_CAP)
}

/// Decode a value from `input` with an explicit cap, enforcing strict
/// (no trailing bytes) consumption per GEN-0035:R6.
pub fn from_bytes_with_cap<T: Decode>(input: &[u8], cap: usize) -> Result<T, DecodeError> {
    let mut d = Decoder::with_cap(input, cap);
    let value = T::decode(&mut d)?;
    if !d.is_at_end() {
        return Err(DecodeError::TrailingBytes);
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

// u8 / i8 — single byte.
impl Encode for u8 {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }
}
impl Decode for u8 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(d.read_bytes(1)?[0])
    }
}
impl Encode for i8 {
    fn encode(&self, out: &mut Vec<u8>) {
        // Bit-pattern preserved; cast is the wire-level operation.
        #[allow(clippy::cast_sign_loss)]
        out.push(*self as u8);
    }
}
impl Decode for i8 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        #[allow(clippy::cast_possible_wrap)]
        Ok(d.read_bytes(1)?[0] as i8)
    }
}

// Macro for fixed-width LE primitives.
macro_rules! impl_le_primitive {
    ($($ty:ty => $n:expr),+ $(,)?) => {
        $(
            impl Encode for $ty {
                fn encode(&self, out: &mut Vec<u8>) {
                    out.extend_from_slice(&self.to_le_bytes());
                }
            }
            impl Decode for $ty {
                fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
                    let bytes = d.read_bytes($n)?;
                    let mut arr = [0u8; $n];
                    arr.copy_from_slice(bytes);
                    Ok(<$ty>::from_le_bytes(arr))
                }
            }
        )+
    };
}

impl_le_primitive!(
    u16 => 2,
    u32 => 4,
    u64 => 8,
    u128 => 16,
    i16 => 2,
    i32 => 4,
    i64 => 8,
    i128 => 16,
    f32 => 4,
    f64 => 8,
);

// bool — 1 byte: 0u8 / 1u8 strict.
impl Encode for bool {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}
impl Decode for bool {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match d.read_bytes(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::InvalidBool),
        }
    }
}

// Unit — zero bytes.
impl Encode for () {
    fn encode(&self, _out: &mut Vec<u8>) {}
}
impl Decode for () {
    fn decode(_d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Option<T> — 0u8 None / 1u8 Some + payload
// ---------------------------------------------------------------------------

impl<T: Encode> Encode for Option<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0u8),
            Some(v) => {
                out.push(1u8);
                v.encode(out);
            }
        }
    }
}
impl<T: Decode> Decode for Option<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        match d.read_bytes(1)?[0] {
            0 => Ok(None),
            1 => Ok(Some(T::decode(d)?)),
            _ => Err(DecodeError::InvalidOptionTag),
        }
    }
}

// ---------------------------------------------------------------------------
// Length-prefixed: Vec<T>, String, &[u8] (encode only for borrowed)
// ---------------------------------------------------------------------------

#[allow(clippy::cast_possible_truncation)]
fn encode_len_prefix(len: usize, out: &mut Vec<u8>) {
    // GEN-0035:R3 — length prefix is `u32 LE`. Lengths beyond u32::MAX
    // are not representable on the wire; the decoder is capped at 1 MiB
    // by default so encoder-side overflow is a programming error.
    debug_assert!(len <= u32::MAX as usize, "length exceeds u32::MAX");
    (len as u32).encode(out);
}

impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        for v in self {
            v.encode(out);
        }
    }
}
impl<T: Decode> Decode for Vec<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        // Bounded doubling: cap was already charged in read_len_prefix,
        // so reserving n is safe. We do not reserve eagerly beyond n,
        // matching GEN-0035 §"Decoder cap" peak-allocation bound.
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(T::decode(d)?);
        }
        Ok(v)
    }
}

// Specialised Vec<u8> path: payload is opaque bytes; skip per-element dispatch.
impl Encode for [u8] {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        out.extend_from_slice(self);
    }
}

impl Encode for String {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        out.extend_from_slice(self.as_bytes());
    }
}
impl Decode for String {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        let bytes = d.read_bytes(n)?;
        core::str::from_utf8(bytes)
            .map(alloc::string::ToString::to_string)
            .map_err(|_| DecodeError::InvalidUtf8)
    }
}

impl Encode for str {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        out.extend_from_slice(self.as_bytes());
    }
}

// ---------------------------------------------------------------------------
// Fixed-size arrays — back-to-back, no length prefix (GEN-0035 tuples / arrays).
// ---------------------------------------------------------------------------

impl<T: Encode, const N: usize> Encode for [T; N] {
    fn encode(&self, out: &mut Vec<u8>) {
        for v in self {
            v.encode(out);
        }
    }
}
impl<T: Decode, const N: usize> Decode for [T; N] {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        // Build via an intermediate Vec to avoid MaybeUninit gymnastics
        // under #![forbid(unsafe_code)].
        let mut v: Vec<T> = Vec::with_capacity(N);
        for _ in 0..N {
            v.push(T::decode(d)?);
        }
        // SAFETY (logical, no unsafe): `try_into` on a Vec of exact length
        // succeeds; we built it with N elements above.
        v.try_into()
            .map_err(|_| DecodeError::UnexpectedEof /* unreachable */)
    }
}

// ---------------------------------------------------------------------------
// BTreeMap<K, V> — u32 LE count + entries in ascending encoded-K-bytes order.
// ---------------------------------------------------------------------------

impl<K: Encode + Ord, V: Encode> Encode for BTreeMap<K, V> {
    fn encode(&self, out: &mut Vec<u8>) {
        // BTreeMap iterates keys in ascending K::Ord order, which is the
        // workspace's canonical-bytes-equivalent for the primitive types
        // currently in use. GEN-0035:R5 mandates ascending *encoded
        // bytes* of K; for the primitive-key cases this coincides with
        // K::Ord. Composite keys whose Ord disagrees with encoded-bytes
        // ordering are not yet supported and will be addressed in
        // sub-mission B if any surface.
        encode_len_prefix(self.len(), out);
        for (k, v) in self {
            k.encode(out);
            v.encode(out);
        }
    }
}

impl<K: Decode + Encode + Ord, V: Decode> Decode for BTreeMap<K, V> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        let mut map = BTreeMap::new();
        let mut prev_key_bytes: Option<Vec<u8>> = None;
        for _ in 0..n {
            // We need to validate canonical order by encoded-key-bytes.
            // Decode the key, re-encode it to obtain its canonical bytes,
            // and compare to the previous entry. This double-pass is the
            // simplest correct implementation; B may optimise to a
            // single-pass read.
            let k = K::decode(d)?;
            let mut k_bytes = Vec::new();
            k.encode(&mut k_bytes);
            if let Some(prev) = &prev_key_bytes
                && k_bytes.as_slice() <= prev.as_slice()
            {
                return Err(DecodeError::NonCanonicalMap);
            }
            let v = V::decode(d)?;
            prev_key_bytes = Some(k_bytes);
            map.insert(k, v);
        }
        Ok(map)
    }
}

// ---------------------------------------------------------------------------
// BTreeSet<T> — u32 LE count + elements in ascending encoded-bytes order.
// ---------------------------------------------------------------------------
//
// Same canonical-ordering contract as BTreeMap (GEN-0035:R5). T: Ord is
// the std std-bound; encoded-bytes order coincides with T::Ord for the
// primitive element types currently in use.

impl<T: Encode + Ord> Encode for BTreeSet<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        for v in self {
            v.encode(out);
        }
    }
}

// ---------------------------------------------------------------------------
// Smart pointers & wrappers — encode-transparent
// ---------------------------------------------------------------------------
//
// Box<T>, Arc<T>, Cow<'_, T> encode as the underlying T — matching the
// schema-hash transparency in pardosa-genome (GEN-0036 §"Smart pointers
// and wrappers"). PhantomData<T> encodes as zero bytes — like `()`.

impl<T: Encode + ?Sized> Encode for Box<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        (**self).encode(out);
    }
}

impl<T: Encode + ?Sized> Encode for Arc<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        (**self).encode(out);
    }
}

impl<T: Encode + ToOwned + ?Sized> Encode for Cow<'_, T> {
    fn encode(&self, out: &mut Vec<u8>) {
        (**self).encode(out);
    }
}

impl<T: ?Sized> Encode for PhantomData<T> {
    fn encode(&self, _out: &mut Vec<u8>) {}
}

// ---------------------------------------------------------------------------
// char — 4-byte LE u32 codepoint
// ---------------------------------------------------------------------------
//
// `char` is a Unicode scalar value (21 bits used). Encoded as the u32
// codepoint; surrogate values are not representable so decode-side validation
// returns InvalidUtf8 if a non-scalar u32 is observed.

impl Encode for char {
    fn encode(&self, out: &mut Vec<u8>) {
        u32::from(*self).encode(out);
    }
}
impl Decode for char {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let cp = u32::decode(d)?;
        char::from_u32(cp).ok_or(DecodeError::InvalidUtf8)
    }
}

// ---------------------------------------------------------------------------
// Borrowed slice forms — &str, &[u8]
// ---------------------------------------------------------------------------
//
// `&str` and `&[u8]` share wire form with `str` / `[u8]` (length-prefixed
// payload). Provided explicitly so generic code over `T: Encode` with
// `T = &str` / `T = &[u8]` resolves without auto-ref dance.

impl Encode for &str {
    fn encode(&self, out: &mut Vec<u8>) {
        (*self).encode(out);
    }
}

impl Encode for &[u8] {
    fn encode(&self, out: &mut Vec<u8>) {
        (*self).encode(out);
    }
}

// ---------------------------------------------------------------------------
// Tuples — back-to-back, no length prefix (up to 4 for now; B extends to 16).
// ---------------------------------------------------------------------------

macro_rules! impl_tuple {
    ($($T:ident: $idx:tt),+) => {
        impl<$($T: Encode),+> Encode for ($($T,)+) {
            fn encode(&self, out: &mut Vec<u8>) {
                $( self.$idx.encode(out); )+
            }
        }
        impl<$($T: Decode),+> Decode for ($($T,)+) {
            fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
                Ok(( $( $T::decode(d)?, )+ ))
            }
        }
    };
}

impl_tuple!(T0: 0);
impl_tuple!(T0: 0, T1: 1);
impl_tuple!(T0: 0, T1: 1, T2: 2);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9, T10: 10);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9, T10: 10, T11: 11);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9, T10: 10, T11: 11, T12: 12);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9, T10: 10, T11: 11, T12: 12, T13: 13);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9, T10: 10, T11: 11, T12: 12, T13: 13, T14: 14);
impl_tuple!(T0: 0, T1: 1, T2: 2, T3: 3, T4: 4, T5: 5, T6: 6, T7: 7, T8: 8, T9: 9, T10: 10, T11: 11, T12: 12, T13: 13, T14: 14, T15: 15);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn rt<T: Encode + Decode + PartialEq + core::fmt::Debug>(v: T) {
        let bytes = to_vec(&v);
        let back: T = from_bytes(&bytes).expect("decode");
        assert_eq!(v, back);
    }

    #[test]
    fn primitive_widths() {
        // GEN-0035 §"Primitive encoding"
        assert_eq!(to_vec(&0u8), vec![0]);
        assert_eq!(to_vec(&1u8), vec![1]);
        assert_eq!(to_vec(&0x0102u16), vec![0x02, 0x01]);
        assert_eq!(to_vec(&0x01020304u32), vec![0x04, 0x03, 0x02, 0x01]);
        assert_eq!(to_vec(&true), vec![1]);
        assert_eq!(to_vec(&false), vec![0]);
    }

    #[test]
    fn primitive_roundtrip() {
        rt(0u8);
        rt(255u8);
        rt(-1i8);
        rt(u16::MAX);
        rt(i16::MIN);
        rt(u32::MAX);
        rt(u64::MAX);
        rt(u128::MAX);
        rt(i128::MIN);
        rt(1.5f32);
        rt(f64::INFINITY.to_bits()); // ensure bit-level f64 roundtrip via u64
        rt(true);
        rt(false);
    }

    #[test]
    fn option_layout() {
        // GEN-0035 §"Composite encoding" — Option<u32>: 1+4 bytes Some, 1 byte None.
        let some = to_vec(&Some(0x01020304u32));
        assert_eq!(some, vec![1, 0x04, 0x03, 0x02, 0x01]);
        let none: Vec<u8> = to_vec(&Option::<u32>::None);
        assert_eq!(none, vec![0]);
        rt(Some(42u64));
        rt(Option::<String>::None);
    }

    #[test]
    fn invalid_option_tag_rejected() {
        let err = from_bytes::<Option<u32>>(&[2u8, 0, 0, 0, 0]).unwrap_err();
        assert_eq!(err, DecodeError::InvalidOptionTag);
    }

    #[test]
    fn invalid_bool_rejected() {
        let err = from_bytes::<bool>(&[2u8]).unwrap_err();
        assert_eq!(err, DecodeError::InvalidBool);
    }

    #[test]
    fn vec_u8_layout() {
        // Vec<u8> length 3: 4 LE length bytes + 3 payload bytes.
        let bytes = to_vec(&vec![0xAAu8, 0xBB, 0xCC]);
        assert_eq!(bytes, vec![3, 0, 0, 0, 0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn string_roundtrip() {
        rt(String::from(""));
        rt(String::from("hello, world"));
        rt(String::from("⛵🦀"));
    }

    #[test]
    fn invalid_utf8_rejected() {
        // len=2, payload = 0xFF 0xFE — invalid UTF-8.
        let err = from_bytes::<String>(&[2, 0, 0, 0, 0xFF, 0xFE]).unwrap_err();
        assert_eq!(err, DecodeError::InvalidUtf8);
    }

    #[test]
    fn trailing_bytes_rejected() {
        // GEN-0035:R6 — one extra byte after a u32.
        let err = from_bytes::<u32>(&[1, 0, 0, 0, 0xFF]).unwrap_err();
        assert_eq!(err, DecodeError::TrailingBytes);
    }

    #[test]
    fn cap_exceeded_before_alloc() {
        // u32 length header = 2 MiB; default cap is 1 MiB. Must reject
        // before allocation (we cannot directly observe allocation, but
        // the error variant must be CapExceeded — not UnexpectedEof from
        // a successful huge allocation followed by short input).
        let bogus_len: u32 = 2 * 1024 * 1024;
        let mut input = Vec::new();
        input.extend_from_slice(&bogus_len.to_le_bytes());
        let err = from_bytes::<Vec<u8>>(&input).unwrap_err();
        assert_eq!(err, DecodeError::CapExceeded);
    }

    #[test]
    fn cap_configurable() {
        // 4-byte length header advertising 8 bytes is fine under cap=16,
        // rejected under cap=4.
        let mut input = Vec::new();
        input.extend_from_slice(&8u32.to_le_bytes());
        input.extend_from_slice(&[0u8; 8]);
        let ok: Vec<u8> = from_bytes_with_cap(&input, 16).unwrap();
        assert_eq!(ok.len(), 8);
        let err = from_bytes_with_cap::<Vec<u8>>(&input, 4).unwrap_err();
        assert_eq!(err, DecodeError::CapExceeded);
    }

    #[test]
    fn cap_charges_nested_length_prefixes() {
        // Vec<Vec<u8>> with two inner vecs of 4 bytes each: outer header
        // charges 2 (count), inner headers charge 4 each, payloads charge
        // 4 each. Total cap usage from len-prefixes = 2 + 4 + 4 = 10 bytes
        // of "budget"; we set cap to exactly fit.
        let v: Vec<Vec<u8>> = vec![vec![1, 2, 3, 4], vec![5, 6, 7, 8]];
        let bytes = to_vec(&v);
        // Generous cap succeeds.
        let back: Vec<Vec<u8>> = from_bytes_with_cap(&bytes, 1024).unwrap();
        assert_eq!(back, v);
        // Cap=2 (= outer count) succeeds for the outer header but the
        // inner length=4 exceeds remaining cap=0, so CapExceeded.
        let err = from_bytes_with_cap::<Vec<Vec<u8>>>(&bytes, 2).unwrap_err();
        assert_eq!(err, DecodeError::CapExceeded);
    }

    #[test]
    fn vec_roundtrip() {
        rt(Vec::<u32>::new());
        rt(vec![1u32, 2, 3, 4, 5]);
        rt(vec![Some(1u8), None, Some(2)]);
    }

    #[test]
    fn array_back_to_back_no_prefix() {
        let bytes = to_vec(&[1u8, 2, 3]);
        assert_eq!(bytes, vec![1, 2, 3]);
        let back: [u8; 3] = from_bytes(&bytes).unwrap();
        assert_eq!(back, [1, 2, 3]);
    }

    #[test]
    fn tuple_back_to_back_no_prefix() {
        let bytes = to_vec(&(1u8, 0x0203u16));
        assert_eq!(bytes, vec![1, 0x03, 0x02]);
        rt((1u32, 2u64, 3u8));
        rt((true, false, 0u8, u32::MAX));
    }

    #[test]
    fn btreemap_roundtrip_and_canonical_order() {
        let mut m: BTreeMap<u32, u8> = BTreeMap::new();
        m.insert(1, 10);
        m.insert(2, 20);
        m.insert(3, 30);
        rt(m.clone());

        // Tamper: re-emit with descending keys; decoder must reject.
        let mut bad = Vec::new();
        3u32.encode(&mut bad);
        3u32.encode(&mut bad);
        30u8.encode(&mut bad);
        2u32.encode(&mut bad);
        20u8.encode(&mut bad);
        1u32.encode(&mut bad);
        10u8.encode(&mut bad);
        let err = from_bytes::<BTreeMap<u32, u8>>(&bad).unwrap_err();
        assert_eq!(err, DecodeError::NonCanonicalMap);
    }

    #[test]
    fn unexpected_eof() {
        let err = from_bytes::<u32>(&[1, 2]).unwrap_err();
        assert_eq!(err, DecodeError::UnexpectedEof);
    }

    #[test]
    fn f1_invariant_anticipation() {
        // GEN-0035 §"Composite encoding" — unit variants of a `repr(u8)`
        // enum encode as one byte = the explicit discriminant. Sub-mission
        // C will land EventError with Internal = 7 (F4) and assert
        // buf[0] == 7u8. Here we anticipate the byte-level expectation
        // for a hand-rolled enum impl, to surface any encoding-spec
        // defect now rather than at C.
        #[repr(u8)]
        enum Tag {
            #[allow(dead_code)]
            Zero = 0,
            Seven = 7,
        }
        impl Encode for Tag {
            fn encode(&self, out: &mut Vec<u8>) {
                let d: u8 = match self {
                    Tag::Zero => 0,
                    Tag::Seven => 7,
                };
                out.push(d);
            }
        }
        let mut buf = Vec::new();
        Tag::Seven.encode(&mut buf);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0], 7u8);
    }
}
