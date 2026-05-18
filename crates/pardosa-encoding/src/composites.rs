//! Composite-type `Encode`/`Decode` impls: `Option<T>`, `Vec<T>`, `String`,
//! borrowed slice forms (`&str`, `&[u8]`, `str`, `[u8]`), fixed-size arrays,
//! `BTreeMap` / `BTreeSet`, smart pointers (`Box`, `Arc`, `Cow`, `PhantomData`),
//! `char`, tuples up to T15, and `NonZeroU64`.
//!
//! GEN-0035 §"Composite encoding". The `encode_len_prefix` helper and the
//! `impl_tuple!` macro live here as their primary consumers; `foreign.rs`
//! re-uses `encode_len_prefix` via the `pub(crate)` re-export.

use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::num::NonZeroU64;

use crate::{Decode, Decoder, Encode, EventError};

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
pub(crate) fn encode_len_prefix(len: usize, out: &mut Vec<u8>) {
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
