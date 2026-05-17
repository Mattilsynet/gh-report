//! In-house canonical encoding for pardosa events (GEN-0035).
//!
//! The wire format is a deterministic sequential canonical encoding
//! (LE primitives, length-prefixed variable-width data, `repr(u8)`
//! enum discriminants) owned by the workspace so we control the spec,
//! the sealing, and the decoder cap semantics. This crate provides the
//! substrate ([`Encode`], [`Decode`], [`EventError`], primitive impls);
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
use core::num::NonZeroU64;

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

/// Canonical event-level error surface for pardosa (GEN-0039).
///
/// `repr(u8)` with literal discriminants pinned 0..=10. The in-house
/// canonical encoding emits a single byte equal to the discriminant for
/// each variant (F4 wire contract: byte-1 of an encoded `EventError`
/// equals the discriminant value).
///
/// Variant ordering and discriminant values are part of the wire
/// contract — see GEN-0039. Renumbering is a breaking change.
///
/// `EventError` is also the return type of [`Decode::decode`] following
/// the C2 migration (sub-mission `adr-fmt-vggv`): all decoder-local
/// failure modes (truncated input, cap exceeded, invalid discriminant,
/// invalid UTF-8, non-canonical map, trailing bytes) collapse to
/// `EventError::InvalidInput`, matching the pre-migration bridge
/// semantics from `pardosa_traits`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum EventError {
    /// Caller-supplied data violated a documented input invariant.
    /// All decoder-local failures (truncated input, malformed tag,
    /// non-canonical ordering, cap exceeded, trailing bytes, invalid
    /// UTF-8, unknown discriminant) surface as this variant.
    InvalidInput = 0,
    /// The addressed entity does not exist.
    NotFound = 1,
    /// The operation conflicts with the current state (e.g. version mismatch,
    /// duplicate key, concurrent write race).
    Conflict = 2,
    /// Caller is not authenticated.
    Unauthorized = 3,
    /// Caller is authenticated but lacks permission for the operation.
    PermissionDenied = 4,
    /// A required dependency is temporarily unavailable; retry may succeed.
    Unavailable = 5,
    /// The operation did not complete within its deadline.
    Timeout = 6,
    /// An internal invariant was violated. Carries no caller-actionable
    /// detail; surface only as an opaque failure.
    Internal = 7,
    /// A resource quota or limit was exceeded (memory, message size, rate).
    ResourceExhausted = 8,
    /// The operation was explicitly cancelled before completion.
    Cancelled = 9,
    /// Underlying storage reported irrecoverable data loss for the
    /// affected entity.
    DataLoss = 10,
}

impl EventError {
    /// Return the wire discriminant byte for this variant.
    ///
    /// Equivalent to the first (and only) byte of `EventError::encode`.
    /// Pinned by GEN-0039; renumbering is a breaking change.
    #[must_use]
    pub const fn discriminant(self) -> u8 {
        // `repr(u8)` makes the cast a no-op at the bit level.
        self as u8
    }
}

impl Encode for EventError {
    fn encode(&self, out: &mut Vec<u8>) {
        // Single byte per GEN-0039 F4 wire contract. `repr(u8)` makes
        // `self.discriminant()` bit-identical to the variant's pinned
        // discriminant.
        out.push(self.discriminant());
    }
}

impl Decode for EventError {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        // Exhaustive match on the pinned 0..=10 wire bytes. Unknown
        // discriminants surface as `InvalidInput`, matching the
        // post-C2 convention for all decoder-local failures.
        let byte = u8::decode(d)?;
        match byte {
            0 => Ok(EventError::InvalidInput),
            1 => Ok(EventError::NotFound),
            2 => Ok(EventError::Conflict),
            3 => Ok(EventError::Unauthorized),
            4 => Ok(EventError::PermissionDenied),
            5 => Ok(EventError::Unavailable),
            6 => Ok(EventError::Timeout),
            7 => Ok(EventError::Internal),
            8 => Ok(EventError::ResourceExhausted),
            9 => Ok(EventError::Cancelled),
            10 => Ok(EventError::DataLoss),
            _ => Err(EventError::InvalidInput),
        }
    }
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
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidInput`] when fewer than `n` bytes remain
    /// in the input, or when the resulting cursor position would overflow.
    pub fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], EventError> {
        let end = self.pos.checked_add(n).ok_or(EventError::InvalidInput)?;
        if end > self.input.len() {
            return Err(EventError::InvalidInput);
        }
        let slice = &self.input[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read a `u32 LE` length header and charge it against the cap.
    /// Returns `CapExceeded` *before* any allocation is attempted.
    ///
    /// # Errors
    ///
    /// Returns [`EventError::InvalidInput`] if the prefix cannot be read or
    /// the length exceeds the remaining decode cap.
    pub fn read_len_prefix(&mut self) -> Result<usize, EventError> {
        let raw = u32::decode(self)?;
        let n = raw as usize;
        if n > self.cap_remaining {
            return Err(EventError::InvalidInput);
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        Ok(d.read_bytes(1)?[0])
    }
}
impl Encode for i8 {
    fn encode(&self, out: &mut Vec<u8>) {
        // Bit-pattern preserved; cast is the wire-level operation.
        #[allow(
            clippy::cast_sign_loss,
            reason = "non-idiomatic Rust required: wire-protocol i8↔u8 reinterpretation per GEN-0035 is by-design bit reuse; `try_from` would reject negative values that are valid on the wire"
        )]
        out.push(*self as u8);
    }
}
impl Decode for i8 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        #[allow(
            clippy::cast_possible_wrap,
            reason = "non-idiomatic Rust required: wire-protocol u8↔i8 reinterpretation per GEN-0035 is by-design bit reuse; `try_from` would reject high-bit-set bytes that are valid on the wire"
        )]
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
                fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        match d.read_bytes(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(EventError::InvalidInput),
        }
    }
}

// Unit — zero bytes.
impl Encode for () {
    fn encode(&self, _out: &mut Vec<u8>) {}
}
impl Decode for () {
    fn decode(_d: &mut Decoder<'_>) -> Result<Self, EventError> {
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        match d.read_bytes(1)?[0] {
            0 => Ok(None),
            1 => Ok(Some(T::decode(d)?)),
            _ => Err(EventError::InvalidInput),
        }
    }
}

// ---------------------------------------------------------------------------
// Length-prefixed: Vec<T>, String, &[u8] (encode only for borrowed)
// ---------------------------------------------------------------------------

#[allow(
    clippy::cast_possible_truncation,
    reason = "non-idiomatic Rust required: wire-protocol u32 LE length prefix per GEN-0035:R3; the debug_assert above forbids the truncation branch in debug builds, release relies on the upstream 1 MiB cap to keep `len` well below `u32::MAX`"
)]
fn encode_len_prefix(len: usize, out: &mut Vec<u8>) {
    // GEN-0035:R3 — length prefix is `u32 LE`. Lengths beyond u32::MAX
    // are not representable on the wire; the decoder is capped at 1 MiB
    // by default so encoder-side overflow is a programming error.
    debug_assert!(u32::try_from(len).is_ok(), "length exceeds u32::MAX");
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        let bytes = d.read_bytes(n)?;
        core::str::from_utf8(bytes)
            .map(alloc::string::ToString::to_string)
            .map_err(|_| EventError::InvalidInput)
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        // Build via an intermediate Vec to avoid MaybeUninit gymnastics
        // under #![forbid(unsafe_code)].
        let mut v: Vec<T> = Vec::with_capacity(N);
        for _ in 0..N {
            v.push(T::decode(d)?);
        }
        // Provably-Ok by construction: `v` has exactly N elements (the
        // loop pushed N times, no fallible interleaving could shrink it
        // after a successful T::decode). `try_into::<[T; N]>` on a Vec
        // of length N cannot fail. The debug_assert pins the invariant
        // for debug builds; the expect carries the same justification
        // into release builds without runtime cost on the happy path.
        debug_assert_eq!(v.len(), N, "loop invariant: v populated with N elements");
        Ok(v.try_into()
            .unwrap_or_else(|_| unreachable!("v.len() == N by loop invariant")))
    }
}

// ---------------------------------------------------------------------------
// BTreeMap<K, V> — u32 LE count + entries in ascending encoded-K-bytes order.
// ---------------------------------------------------------------------------

impl<K: Encode + Ord, V: Encode> Encode for BTreeMap<K, V> {
    fn encode(&self, out: &mut Vec<u8>) {
        // GEN-0035:R5 mandates ascending *encoded bytes* of K. BTreeMap
        // iterates in K::Ord order, which coincides with encoded-bytes
        // order for fixed-width primitive keys but disagrees for any
        // variable-length-encoded K (String, Vec<u8>, Vec<T>, etc.) where
        // the u32 length prefix dominates lex order. We therefore encode
        // each (k, v) into a scratch pair and sort pairs by canonical
        // K-bytes before emission. Sort-key bytes ARE the wire bytes for
        // K (`Encode::encode` is deterministic), so the decoder's
        // re-encode-and-compare invariant at lib.rs:413–438 holds.
        encode_len_prefix(self.len(), out);
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(self.len());
        for (k, v) in self {
            let mut k_bytes = Vec::new();
            k.encode(&mut k_bytes);
            let mut v_bytes = Vec::new();
            v.encode(&mut v_bytes);
            pairs.push((k_bytes, v_bytes));
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        for (k_bytes, v_bytes) in &pairs {
            out.extend_from_slice(k_bytes);
            out.extend_from_slice(v_bytes);
        }
    }
}

impl<K: Decode + Encode + Ord, V: Decode> Decode for BTreeMap<K, V> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
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
                return Err(EventError::InvalidInput);
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let cp = u32::decode(d)?;
        char::from_u32(cp).ok_or(EventError::InvalidInput)
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
            fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
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
// GEN-0041 foreign-crate v0 floor
// ---------------------------------------------------------------------------
//
// Encode + Decode impls for `uuid::Uuid`, `bytes::Bytes`, and
// `arrayvec::ArrayVec<T, N>` behind feature gates `uuid`, `bytes`,
// `arrayvec`. The sealing chain (`sealed::Sealed` + `EventSafe`) for these
// types lives in `pardosa-traits` behind matching feature gates; the orphan
// rule mandates the split (Encode is defined here, EventSafe there).
//
// S1 (byte-shape conformance) — each impl conforms to GEN-0035 length-prefix
// rules: fixed-width types emit verbatim bytes back-to-back; variable-length
// payloads emit `[len:u32 LE][bytes…]`. Tests below assert wire layout.
//
// S2 (post-decode capacity/length validity) — capacity-bounded types
// (`ArrayVec<T, N>`) reject a decoded length > N before any allocation,
// surfacing as `EventError::InvalidInput` (the frozen post-C2 variant for
// caller-input violations; no new variant introduced).

// ---- uuid::Uuid — 16 bytes verbatim, no length prefix ----------------------
#[cfg(feature = "uuid")]
impl Encode for uuid::Uuid {
    fn encode(&self, out: &mut Vec<u8>) {
        // `Uuid::as_bytes()` returns `&[u8; 16]` with a stable layout
        // (uuid crate documents these as the bytes "in network order");
        // we emit them verbatim. Fixed width = no length prefix, matching
        // GEN-0035 §"Fixed-size arrays".
        out.extend_from_slice(uuid::Uuid::as_bytes(self));
    }
}

#[cfg(feature = "uuid")]
impl Decode for uuid::Uuid {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let bytes = d.read_bytes(16)?;
        let mut arr = [0u8; 16];
        arr.copy_from_slice(bytes);
        Ok(uuid::Uuid::from_bytes(arr))
    }
}

// ---- bytes::Bytes — length-prefixed opaque payload -------------------------
#[cfg(feature = "bytes")]
impl Encode for bytes::Bytes {
    fn encode(&self, out: &mut Vec<u8>) {
        // Wire-identical to `Vec<u8>` / `&[u8]` — GEN-0035 length-prefix
        // rule applies to any variable-length byte payload regardless of
        // ownership flavour. Round-trip via `Bytes::copy_from_slice` on
        // the decode side.
        encode_len_prefix(self.len(), out);
        out.extend_from_slice(self);
    }
}

#[cfg(feature = "bytes")]
impl Decode for bytes::Bytes {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        let slice = d.read_bytes(n)?;
        Ok(bytes::Bytes::copy_from_slice(slice))
    }
}

// ---- arrayvec::ArrayVec<T, N> — length-prefixed bounded vec ---------------
#[cfg(feature = "arrayvec")]
impl<T: Encode, const N: usize> Encode for arrayvec::ArrayVec<T, N> {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        for v in self {
            v.encode(out);
        }
    }
}

#[cfg(feature = "arrayvec")]
impl<T: Decode, const N: usize> Decode for arrayvec::ArrayVec<T, N> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let n = d.read_len_prefix()?;
        // S2 guard: capacity-bounded types must reject `len > N` before any
        // per-element decode so a malformed header cannot consume budget
        // it will never deposit into a value.
        if n > N {
            return Err(EventError::InvalidInput);
        }
        let mut v: arrayvec::ArrayVec<T, N> = arrayvec::ArrayVec::new();
        for _ in 0..n {
            // `try_push` cannot fail given the S2 check above — n ≤ N and
            // we push exactly n times. Mapping the (unreachable) error to
            // InvalidInput keeps the surface frozen.
            v.try_push(T::decode(d)?)
                .map_err(|_| EventError::InvalidInput)?;
        }
        Ok(v)
    }
}

// ---- jiff::Timestamp — 8 bytes LE of as_microsecond() ---------------------
//
// GEN-0043:R1 — canonical wire shape is the 8-byte little-endian encoding
// of `Timestamp::as_microsecond()` (i64 microseconds since the Unix epoch).
// Decode reads 8 bytes LE and reconstructs via `Timestamp::from_microsecond`;
// the round-trip is total over `i64` (no in-range rejection — `from_microsecond`
// accepts the full `i64` domain per GEN-0043:R1). Sub-microsecond precision
// is truncated at encode and the truncated value is the canonical wall-clock
// identity (GEN-0043:R2). Fixed-width 8 bytes — no length prefix; the
// GEN-0035:R8 decoder cap does not apply (GEN-0043:R4).
#[cfg(feature = "jiff")]
impl Encode for jiff::Timestamp {
    fn encode(&self, out: &mut Vec<u8>) {
        self.as_microsecond().encode(out);
    }
}

#[cfg(feature = "jiff")]
impl Decode for jiff::Timestamp {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let micros = i64::decode(d)?;
        // `from_microsecond` is total over i64 per GEN-0043:R1 — no in-range
        // check at decode. Map any future error (e.g. if jiff narrows its
        // accepted range) onto the frozen InvalidInput variant.
        jiff::Timestamp::from_microsecond(micros).map_err(|_| EventError::InvalidInput)
    }
}

// ---------------------------------------------------------------------------
// NonZeroU64 — F2c-pre Inc-pre.1 (PAR-0021:R1 hash-chain prereq)
// ---------------------------------------------------------------------------
//
// Concrete-type impl, NOT a blanket over `NonZero<T>`. The blanket form
// would foreclose a future, wider sealing scheme that wants to enumerate
// each `NonZero<*>` independently; pinning to `NonZeroU64` here is the
// only consumer surface today (cherry-pit-core `AggregateId`, `sequence`)
// and leaves the blanket path open. Wire shape: 8 bytes LE of the inner
// u64 — identical to `u64`. Decode rejects `0u64` (niche violation) as
// `EventError::InvalidInput`.

impl Encode for NonZeroU64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.get().encode(out);
    }
}

impl Decode for NonZeroU64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        let raw = u64::decode(d)?;
        NonZeroU64::new(raw).ok_or(EventError::InvalidInput)
    }
}

// ---------------------------------------------------------------------------
// PAR-0021 R1 precursor-hash helper
// ---------------------------------------------------------------------------
//
// BLAKE3 is the pardosa precursor-identity hash. The helper is feature-gated
// so the default no-feature build of `pardosa-encoding` stays dep-free per
// GEN-0041. The hash domain (which event bytes feed in) is the caller's
// responsibility: this helper treats input as opaque bytes and never
// inspects encoding structure. F2c will define the encoding-excluding-hash
// canonicalisation and wire callers accordingly.

/// BLAKE3 hash of canonical event bytes — the precursor identity per PAR-0021 R1.
///
/// Input is the canonical-encoded event bytes EXCLUDING the `precursor_hash`
/// field itself (F2c defines that encoding). This helper treats input as
/// opaque bytes; domain separation is the caller's responsibility.
///
/// Feature-gated under `blake3`; default no-feature build preserves the
/// `#![no_std]` substrate dependency-free per GEN-0041.
#[cfg(feature = "blake3")]
#[must_use]
pub fn precursor_hash_of(event_bytes: &[u8]) -> [u8; 32] {
    blake3::hash(event_bytes).into()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper takes T by value to keep 27 call sites ergonomic (`rt(String::from(\"x\"))` rather than `rt(&String::from(\"x\"))`); `assert_eq!` then borrows internally"
    )]
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
        assert_eq!(to_vec(&0x0102_0304_u32), vec![0x04, 0x03, 0x02, 0x01]);
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
        let some = to_vec(&Some(0x0102_0304_u32));
        assert_eq!(some, vec![1, 0x04, 0x03, 0x02, 0x01]);
        let none: Vec<u8> = to_vec(&Option::<u32>::None);
        assert_eq!(none, vec![0]);
        rt(Some(42u64));
        rt(Option::<String>::None);
    }

    #[test]
    fn invalid_option_tag_rejected() {
        let err = from_bytes::<Option<u32>>(&[2u8, 0, 0, 0, 0]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn invalid_bool_rejected() {
        let err = from_bytes::<bool>(&[2u8]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn vec_u8_layout() {
        // Vec<u8> length 3: 4 LE length bytes + 3 payload bytes.
        let bytes = to_vec(&vec![0xAAu8, 0xBB, 0xCC]);
        assert_eq!(bytes, vec![3, 0, 0, 0, 0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn string_roundtrip() {
        rt(String::new());
        rt(String::from("hello, world"));
        rt(String::from("⛵🦀"));
    }

    #[test]
    fn invalid_utf8_rejected() {
        // len=2, payload = 0xFF 0xFE — invalid UTF-8.
        let err = from_bytes::<String>(&[2, 0, 0, 0, 0xFF, 0xFE]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn trailing_bytes_rejected() {
        // GEN-0035:R6 — one extra byte after a u32.
        let err = from_bytes::<u32>(&[1, 0, 0, 0, 0xFF]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
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
        assert_eq!(err, EventError::InvalidInput);
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
        assert_eq!(err, EventError::InvalidInput);
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
        assert_eq!(err, EventError::InvalidInput);
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
        assert_eq!(err, EventError::InvalidInput);
    }

    // ---- B0 (sub-mission B folded in): canonical map ordering for
    // variable-length encoded keys. GEN-0035:R5 — entries must be emitted
    // in ascending order of canonical encoded bytes of K, not K::Ord.
    // The two roundtrip tests below were red against the pre-fix encoder
    // because for mixed-length keys (e.g. "alpha"/"beta"/"gamma") the
    // u32 length prefix dominates lex order and disagrees with K::Ord;
    // the decoder rejected the encoder's own output with NonCanonicalMap.
    //
    // Subsumption notes (sub-mission B's named matrix):
    //   - `string_roundtrip` covers the String matrix entry.
    //   - `vec_u8_layout` + `cap_charges_nested_length_prefixes` cover
    //     Vec<u8> round-trip; `roundtrip_btreemap_vec_u8_u32_mixed_length`
    //     below additionally exercises Vec<u8>-keyed BTreeMaps.

    #[test]
    fn roundtrip_btreemap_string_u32_mixed_length() {
        // B0 load-bearing: original reproducer. Mixed-length String keys
        // — K::Ord ("alpha" < "beta" < "gamma") disagrees with encoded-
        // bytes order because the u32 length prefix of "gamma" (5) ties
        // with "alpha" but "beta" is length 4 ... actually all three are
        // not equal length, so encoded-bytes order = ascending length
        // tie-break by content. Encoder must sort by encoded-K-bytes.
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        m.insert(String::from("alpha"), 1);
        m.insert(String::from("beta"), 2);
        m.insert(String::from("gamma"), 3);
        rt(m);
    }

    #[test]
    fn canonical_bytes_btreemap_string_u32_mixed_length() {
        // B0 load-bearing: assert the wire bytes are the
        // sort-by-encoded-K-bytes order, NOT K::Ord order.
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        m.insert(String::from("alpha"), 1);
        m.insert(String::from("beta"), 2);
        m.insert(String::from("gamma"), 3);
        let got = to_vec(&m);

        // Build expected by encoding each (k, v) into its own buffer,
        // sorting pairs by encoded-K-bytes, then concatenating with the
        // u32 LE count prefix.
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for (k, v) in [("alpha", 1u32), ("beta", 2), ("gamma", 3)] {
            let mut kb = Vec::new();
            String::from(k).encode(&mut kb);
            let mut vb = Vec::new();
            v.encode(&mut vb);
            pairs.push((kb, vb));
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let mut expected = Vec::new();
        expected.extend_from_slice(
            &u32::try_from(pairs.len())
                .expect("test fixture under u32::MAX")
                .to_le_bytes(),
        );
        for (kb, vb) in &pairs {
            expected.extend_from_slice(kb);
            expected.extend_from_slice(vb);
        }
        assert_eq!(got, expected);
    }

    #[test]
    fn roundtrip_btreemap_vec_u8_u32_mixed_length() {
        // B0 load-bearing: generalises beyond String. Vec<u8> keys with
        // distinct lengths — vec![1] (len 1), vec![1,1] (len 2), vec![2]
        // (len 1). K::Ord on Vec<u8> is lex on bytes ignoring length, so
        // vec![1] < vec![1,1] < vec![2]; encoded bytes prepend u32 len,
        // so encoded order = ascending length tie-break by content.
        let mut m: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        m.insert(vec![1], 10);
        m.insert(vec![1, 1], 20);
        m.insert(vec![2], 30);
        rt(m);
    }

    #[test]
    fn decode_btreemap_rejects_misordered() {
        // B0 load-bearing: negative case. Hand-construct an encoded map
        // whose entries are in K::Ord order with mixed-length String keys
        // (which is the *wrong* order under GEN-0035:R5). Decoder must
        // reject with NonCanonicalMap. Guards the decoder's invariant
        // against any future "optimisation" that drops the check.
        let mut bad = Vec::new();
        // count = 3
        encode_len_prefix(3, &mut bad);
        // K::Ord order: "alpha", "beta", "gamma". For variable-length
        // keys this does NOT match encoded-bytes order (length prefix
        // dominates), so the decoder must reject.
        for (k, v) in [("alpha", 1u32), ("beta", 2), ("gamma", 3)] {
            String::from(k).encode(&mut bad);
            v.encode(&mut bad);
        }
        let err = from_bytes::<BTreeMap<String, u32>>(&bad).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn roundtrip_tuple_u8_u16_u32() {
        // B's named matrix entry (was rolled back at B-attempt). Verifies
        // tuple round-trip at a specific arity beyond the existing
        // `tuple_back_to_back_no_prefix` coverage.
        rt((7u8, 0x1234u16, 0xdead_beefu32));
    }

    #[test]
    fn unexpected_eof() {
        let err = from_bytes::<u32>(&[1, 2]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
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
            #[expect(
                dead_code,
                reason = "test enum: `Zero` is the documentary tag-0 discriminant; only `Seven` is constructed in this test body"
            )]
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

    // ----- GEN-0041 foreign-crate v0 floor -----------------------------------

    #[cfg(feature = "uuid")]
    #[test]
    fn uuid_roundtrip_and_layout() {
        // S1: Uuid encodes as 16 verbatim bytes, no length prefix. Pick a
        // pattern whose every byte is distinct so any byte-order surprise
        // would show up as a permuted assert.
        let raw: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
            0xEE, 0xFF,
        ];
        let u = uuid::Uuid::from_bytes(raw);
        let bytes = to_vec(&u);
        assert_eq!(bytes.as_slice(), &raw[..]);
        let back: uuid::Uuid = from_bytes(&bytes).unwrap();
        assert_eq!(back, u);
    }

    #[cfg(feature = "uuid")]
    #[test]
    fn uuid_truncated_input_rejected() {
        // 15 bytes is one short of the 16 the fixed-width decode requires;
        // surfaces via the standard truncated-read path.
        let err = from_bytes::<uuid::Uuid>(&[0u8; 15]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[cfg(feature = "bytes")]
    #[test]
    fn bytes_roundtrip_and_layout() {
        // S1: length-prefixed payload identical to Vec<u8>/&[u8] wire form.
        let payload = bytes::Bytes::from_static(&[0xAA, 0xBB, 0xCC]);
        let wire = to_vec(&payload);
        assert_eq!(wire, vec![3, 0, 0, 0, 0xAA, 0xBB, 0xCC]);
        let back: bytes::Bytes = from_bytes(&wire).unwrap();
        assert_eq!(back, payload);

        // Empty payload still carries the 4-byte length header.
        let empty = bytes::Bytes::new();
        let wire_empty = to_vec(&empty);
        assert_eq!(wire_empty, vec![0, 0, 0, 0]);
        let back_empty: bytes::Bytes = from_bytes(&wire_empty).unwrap();
        assert_eq!(back_empty, empty);
    }

    #[cfg(feature = "bytes")]
    #[test]
    fn bytes_wire_matches_vec_u8() {
        // S1 sanity: a `bytes::Bytes` payload and a `Vec<u8>` with identical
        // contents must produce byte-identical wire output. Locks the
        // "same length-prefix rule for any opaque byte payload" invariant.
        let payload = [0xDE, 0xAD, 0xBE, 0xEF, 0x01];
        let from_bytes_form = to_vec(&bytes::Bytes::copy_from_slice(&payload));
        let from_vec_form = to_vec(&payload.to_vec());
        assert_eq!(from_bytes_form, from_vec_form);
    }

    #[cfg(feature = "arrayvec")]
    #[test]
    fn arrayvec_roundtrip() {
        // Variable-length capacity-bounded vec encodes like Vec<T>: u32 LE
        // count + per-element encode. Round-trip at len < N and len == N.
        let mut av: arrayvec::ArrayVec<u32, 4> = arrayvec::ArrayVec::new();
        av.try_push(1).unwrap();
        av.try_push(2).unwrap();
        av.try_push(3).unwrap();
        let wire = to_vec(&av);
        // 4-byte LE count + 3 * 4-byte u32 LE payload = 16 bytes total.
        assert_eq!(wire[..4], [3, 0, 0, 0]);
        let back: arrayvec::ArrayVec<u32, 4> = from_bytes(&wire).unwrap();
        assert_eq!(back.as_slice(), av.as_slice());

        // At capacity.
        let mut full: arrayvec::ArrayVec<u8, 3> = arrayvec::ArrayVec::new();
        full.try_push(7).unwrap();
        full.try_push(8).unwrap();
        full.try_push(9).unwrap();
        let wire = to_vec(&full);
        let back: arrayvec::ArrayVec<u8, 3> = from_bytes(&wire).unwrap();
        assert_eq!(back.as_slice(), full.as_slice());
    }

    #[cfg(feature = "arrayvec")]
    #[test]
    fn arrayvec_rejects_len_over_capacity() {
        // S2: a decoded length-prefix exceeding the target capacity must
        // surface as EventError::InvalidInput *before* any per-element
        // decode runs. Construct the smallest such wire: count=4 against
        // a 3-capacity ArrayVec.
        //
        // Bonus: only 4 payload bytes follow (one fewer than required for
        // count=4 even at u8), so a missed S2 guard would also fail via
        // the truncated-read path; the test asserts the *S2* code path is
        // reached by making the input long enough that absent the guard
        // the decode would otherwise succeed.
        let mut wire = Vec::new();
        4u32.encode(&mut wire);
        wire.extend_from_slice(&[1u8, 2, 3, 4]);
        let err = from_bytes::<arrayvec::ArrayVec<u8, 3>>(&wire).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    // ----- jiff::Timestamp foreign-floor (GEN-0043) --------------------------

    #[cfg(feature = "jiff")]
    #[test]
    fn jiff_timestamp_layout_and_roundtrip() {
        // GEN-0043:R1 — wire shape is 8-byte LE of as_microsecond() (i64).
        // Pick a distinct-byte pattern in the positive half so any byte-
        // order surprise would show up as a permuted assert.
        let micros: i64 = 0x0102_0304_0506_0708;
        let ts = jiff::Timestamp::from_microsecond(micros).unwrap();
        let bytes = to_vec(&ts);
        assert_eq!(bytes, vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        let back: jiff::Timestamp = from_bytes(&bytes).unwrap();
        assert_eq!(back, ts);
        assert_eq!(back.as_microsecond(), micros);
    }

    #[cfg(feature = "jiff")]
    #[test]
    fn jiff_timestamp_zero_micros_roundtrip() {
        // The Unix epoch — micros == 0 — must encode as 8 zero bytes and
        // round-trip cleanly. Anchors the floor of the GEN-0043:R1 domain.
        let ts = jiff::Timestamp::from_microsecond(0).unwrap();
        let bytes = to_vec(&ts);
        assert_eq!(bytes, vec![0u8; 8]);
        let back: jiff::Timestamp = from_bytes(&bytes).unwrap();
        assert_eq!(back, ts);
    }

    #[cfg(feature = "jiff")]
    #[test]
    fn jiff_timestamp_truncated_input_rejected() {
        // 7 bytes is one short of the fixed-width 8 the decode requires;
        // surfaces via the standard truncated-read path.
        let err = from_bytes::<jiff::Timestamp>(&[0u8; 7]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    // -----------------------------------------------------------------
    // EventError wire-contract pins (GEN-0039 / footgun FH6 = F11+F12)
    // -----------------------------------------------------------------
    //
    // These tests freeze the wire byte for every `EventError` variant.
    // If a future edit reorders or renumbers the enum, the assertions
    // below break loudly instead of the wire silently shifting.

    #[test]
    fn event_error_discriminants_pinned() {
        // One assert per variant — GEN-0039 wire contract, byte 1.
        assert_eq!(EventError::InvalidInput.discriminant(), 0);
        assert_eq!(EventError::NotFound.discriminant(), 1);
        assert_eq!(EventError::Conflict.discriminant(), 2);
        assert_eq!(EventError::Unauthorized.discriminant(), 3);
        assert_eq!(EventError::PermissionDenied.discriminant(), 4);
        assert_eq!(EventError::Unavailable.discriminant(), 5);
        assert_eq!(EventError::Timeout.discriminant(), 6);
        assert_eq!(EventError::Internal.discriminant(), 7);
        assert_eq!(EventError::ResourceExhausted.discriminant(), 8);
        assert_eq!(EventError::Cancelled.discriminant(), 9);
        assert_eq!(EventError::DataLoss.discriminant(), 10);
    }

    #[test]
    fn event_error_roundtrip_every_variant() {
        // Symmetric Encode/Decode for every variant 0..=10.
        for v in [
            EventError::InvalidInput,
            EventError::NotFound,
            EventError::Conflict,
            EventError::Unauthorized,
            EventError::PermissionDenied,
            EventError::Unavailable,
            EventError::Timeout,
            EventError::Internal,
            EventError::ResourceExhausted,
            EventError::Cancelled,
            EventError::DataLoss,
        ] {
            let bytes = to_vec(&v);
            assert_eq!(bytes.len(), 1, "EventError encodes to one byte");
            assert_eq!(bytes[0], v.discriminant());
            let back: EventError = from_bytes(&bytes).expect("decode");
            assert_eq!(v, back);
        }
    }

    #[test]
    fn event_error_unknown_discriminant_rejected() {
        // Discriminants 11..=255 are not assigned; decode must reject.
        for b in 11u8..=255 {
            let err = from_bytes::<EventError>(&[b]).unwrap_err();
            assert_eq!(err, EventError::InvalidInput);
        }
    }

    // -----------------------------------------------------------------
    // NonZeroU64 — Inc-pre.1 (F2c-pre, PAR-0021:R1 hash-chain prereq)
    // -----------------------------------------------------------------
    //
    // Concrete-type impl, NOT a blanket over `NonZero<T>`. Preserves
    // the future blanket-impl path so a wider sealing scheme can
    // subsume this point impl without removing it. Wire shape: 8-byte
    // LE of the inner u64, identical to `u64` (NonZeroU64::get()).

    #[test]
    fn non_zero_u64_layout_and_roundtrip() {
        use core::num::NonZeroU64;
        let nz = NonZeroU64::new(0x0102_0304_0506_0708).expect("nonzero literal");
        let bytes = to_vec(&nz);
        // 8-byte LE — same wire as u64::get().
        assert_eq!(bytes, vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        let back: NonZeroU64 = from_bytes(&bytes).expect("decode");
        assert_eq!(back, nz);
    }

    #[test]
    fn non_zero_u64_rejects_zero_on_decode() {
        // Wire `0u64` violates the niche; surface as InvalidInput.
        let err = from_bytes::<core::num::NonZeroU64>(&[0u8; 8]).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }
}
