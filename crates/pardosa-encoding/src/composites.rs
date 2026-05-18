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

impl<T: Decode + Encode + Ord> Decode for BTreeSet<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, EventError> {
        // Mirrors BTreeMap<K, V>::decode (this file, above). For each
        // element we decode T, re-encode to obtain canonical bytes, and
        // assert strictly ascending byte order against the previous
        // element. Strict (not ≤) because BTreeSet does not admit
        // duplicates; a repeat is a wire violation, not just
        // non-canonical. The re-encode is the same double-pass shape
        // as BTreeMap and a future SM may collapse to single-pass.
        let n = d.read_len_prefix()?;
        let mut set = BTreeSet::new();
        let mut prev_bytes: Option<Vec<u8>> = None;
        for _ in 0..n {
            let v = T::decode(d)?;
            let mut v_bytes = Vec::new();
            v.encode(&mut v_bytes);
            if let Some(prev) = &prev_bytes
                && v_bytes.as_slice() <= prev.as_slice()
            {
                return Err(EventError::InvalidInput);
            }
            prev_bytes = Some(v_bytes);
            set.insert(v);
        }
        Ok(set)
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

#[cfg(test)]
mod tests {
    use crate::composites::encode_len_prefix;
    use crate::{Decode, Encode, EventError, from_bytes, to_vec};
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper takes T by value to keep call sites ergonomic; `assert_eq!` then borrows internally"
    )]
    fn rt<T: Encode + Decode + PartialEq + core::fmt::Debug>(v: T) {
        let bytes = to_vec(&v);
        let back: T = from_bytes(&bytes).expect("decode");
        assert_eq!(v, back);
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

    // -----------------------------------------------------------------
    // BTreeSet<T> — F3 (`adr-fmt-mync`, Encode/Decode parity)
    // -----------------------------------------------------------------
    //
    // Encode existed pre-F3; Decode was missing. Wire shape: u32 LE
    // count prefix followed by elements. Decoder enforces strictly
    // ascending encoded-bytes order (BTreeSet rejects duplicates, so
    // equal-bytes is also a wire violation, not just non-canonical).
    //
    // KNOWN GAP filed as discovered-from at the bottom of this block:
    // the Encode side iterates in `T::Ord`, which only coincides with
    // encoded-bytes order for fixed-width primitive `T`. Variable-
    // length `T` (e.g. `String`, `Vec<u8>`) needs the sort-by-encoded-
    // bytes shape used by BTreeMap. F3's scope is decode parity; the
    // wider encode-order fix is a separate bead.

    #[test]
    fn btreeset_roundtrip_primitive() {
        use alloc::collections::BTreeSet;
        let mut s: BTreeSet<u32> = BTreeSet::new();
        s.insert(1);
        s.insert(2);
        s.insert(3);
        rt(s);

        rt(BTreeSet::<u64>::new());
    }

    #[test]
    fn btreeset_decode_rejects_misordered() {
        use alloc::collections::BTreeSet;
        // Hand-construct an encoded set with descending u32 elements;
        // decoder must reject as the ascending-bytes invariant fails.
        let mut bad = Vec::new();
        encode_len_prefix(3, &mut bad);
        3u32.encode(&mut bad);
        2u32.encode(&mut bad);
        1u32.encode(&mut bad);
        let err = from_bytes::<BTreeSet<u32>>(&bad).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }

    #[test]
    fn btreeset_decode_rejects_duplicate() {
        use alloc::collections::BTreeSet;
        // Strictly-ascending check catches equal adjacent bytes too;
        // a duplicate element is a wire violation, not just non-canonical.
        let mut bad = Vec::new();
        encode_len_prefix(2, &mut bad);
        7u32.encode(&mut bad);
        7u32.encode(&mut bad);
        let err = from_bytes::<BTreeSet<u32>>(&bad).unwrap_err();
        assert_eq!(err, EventError::InvalidInput);
    }
}
