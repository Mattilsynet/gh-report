use crate::{Decode, DecodeError, Decoder, Encode};
use crate::{EncodeOverflow, TryEncode, try_encode_len_prefix};
use alloc::borrow::{Cow, ToOwned};
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::num::NonZeroU64;
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
            tag => Err(DecodeError::TagOutOfRange {
                tag: u32::from(tag),
            }),
        }
    }
}
/// Encode a `usize` length as the LE `u32` wire prefix
/// (GEN-0035:R3).
///
/// W1 (2026-05-24): prior `len as u32` silently truncated
/// `>= 1 << 32` in release, emitting full payload — wrong
/// bytes, undetectable downstream. Surface: every `Vec<T>`,
/// `String`, `&[u8]`, `BTreeMap`, `BTreeSet` encoder.
///
/// Fix: `u32::try_from(len).expect(..)` panics in debug +
/// release; representable values round-trip byte-identically.
/// ADR-0009 stability: `encode` keeps `-> ()`.
///
/// # Panics
/// `len > u32::MAX`. Honest callers stay under
/// `DEFAULT_DECODE_CAP` (1 MiB).
pub(crate) fn encode_len_prefix(len: usize, out: &mut Vec<u8>) {
    let prefix = u32::try_from(len).expect("length exceeds u32::MAX");
    prefix.encode(out);
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
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(T::decode(d)?);
        }
        Ok(v)
    }
}
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
            .map_err(|_| DecodeError::InvalidValue)
    }
}
impl Encode for str {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        out.extend_from_slice(self.as_bytes());
    }
}
impl<T: Encode, const N: usize> Encode for [T; N] {
    fn encode(&self, out: &mut Vec<u8>) {
        for v in self {
            v.encode(out);
        }
    }
}
impl<T: Decode, const N: usize> Decode for [T; N] {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let mut v: Vec<T> = Vec::with_capacity(N);
        for _ in 0..N {
            v.push(T::decode(d)?);
        }
        debug_assert_eq!(v.len(), N, "loop invariant: v populated with N elements");
        Ok(v.try_into()
            .unwrap_or_else(|_| unreachable!("v.len() == N by loop invariant")))
    }
}
impl<K: Encode + Ord, V: Encode> Encode for BTreeMap<K, V> {
    fn encode(&self, out: &mut Vec<u8>) {
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
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let n = d.read_len_prefix()?;
        let mut map = BTreeMap::new();
        let mut prev_key_bytes: Option<Vec<u8>> = None;
        for _ in 0..n {
            let k = K::decode(d)?;
            let mut k_bytes = Vec::new();
            k.encode(&mut k_bytes);
            if let Some(prev) = &prev_key_bytes
                && k_bytes.as_slice() <= prev.as_slice()
            {
                return Err(DecodeError::InvalidValue);
            }
            let v = V::decode(d)?;
            prev_key_bytes = Some(k_bytes);
            map.insert(k, v);
        }
        Ok(map)
    }
}
impl<T: Encode + Ord> Encode for BTreeSet<T> {
    fn encode(&self, out: &mut Vec<u8>) {
        encode_len_prefix(self.len(), out);
        let mut scratches: Vec<Vec<u8>> = Vec::with_capacity(self.len());
        for v in self {
            let mut buf = Vec::new();
            v.encode(&mut buf);
            scratches.push(buf);
        }
        scratches.sort();
        for buf in &scratches {
            out.extend_from_slice(buf);
        }
    }
}
impl<T: Decode + Encode + Ord> Decode for BTreeSet<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
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
                return Err(DecodeError::InvalidValue);
            }
            prev_bytes = Some(v_bytes);
            set.insert(v);
        }
        Ok(set)
    }
}
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
impl<T: Decode> Decode for Box<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Box::new(T::decode(d)?))
    }
}
impl<T: Decode> Decode for Arc<T> {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Arc::new(T::decode(d)?))
    }
}
impl<T: ?Sized> Encode for PhantomData<T> {
    fn encode(&self, _out: &mut Vec<u8>) {}
}
impl Encode for char {
    fn encode(&self, out: &mut Vec<u8>) {
        u32::from(*self).encode(out);
    }
}
impl Decode for char {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let cp = u32::decode(d)?;
        char::from_u32(cp).ok_or(DecodeError::InvalidValue)
    }
}
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
macro_rules! impl_tuple {
    ($($T:ident : $idx:tt),+) => {
        impl <$($T : Encode),+> Encode for ($($T,)+) { fn encode(& self, out : & mut Vec
        < u8 >) { $(self.$idx .encode(out);)+ } } impl <$($T : Decode),+> Decode for
        ($($T,)+) { fn decode(d : & mut Decoder <'_ >) -> Result < Self, DecodeError > {
        Ok(($($T ::decode(d) ?,)+)) } }
    };
}
impl_tuple!(T0 : 0);
impl_tuple!(T0 : 0, T1 : 1);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2, T3 : 3);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7);
impl_tuple!(T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9
);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9, T10 :
    10
);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9, T10 :
    10, T11 : 11
);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9, T10 :
    10, T11 : 11, T12 : 12
);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9, T10 :
    10, T11 : 11, T12 : 12, T13 : 13
);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9, T10 :
    10, T11 : 11, T12 : 12, T13 : 13, T14 : 14
);
impl_tuple!(
    T0 : 0, T1 : 1, T2 : 2, T3 : 3, T4 : 4, T5 : 5, T6 : 6, T7 : 7, T8 : 8, T9 : 9, T10 :
    10, T11 : 11, T12 : 12, T13 : 13, T14 : 14, T15 : 15
);
impl Encode for NonZeroU64 {
    fn encode(&self, out: &mut Vec<u8>) {
        self.get().encode(out);
    }
}
impl Decode for NonZeroU64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let raw = u64::decode(d)?;
        NonZeroU64::new(raw).ok_or(DecodeError::InvalidValue)
    }
}
impl<T: Encode> TryEncode for Vec<T> {
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
        try_encode_len_prefix(self.len(), out)?;
        for v in self {
            v.encode(out);
        }
        Ok(())
    }
}
impl TryEncode for [u8] {
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
        try_encode_len_prefix(self.len(), out)?;
        out.extend_from_slice(self);
        Ok(())
    }
}
impl TryEncode for String {
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
        try_encode_len_prefix(self.len(), out)?;
        out.extend_from_slice(self.as_bytes());
        Ok(())
    }
}
impl TryEncode for str {
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
        try_encode_len_prefix(self.len(), out)?;
        out.extend_from_slice(self.as_bytes());
        Ok(())
    }
}
impl<K: Encode + Ord, V: Encode> TryEncode for BTreeMap<K, V> {
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
        try_encode_len_prefix(self.len(), out)?;
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
        Ok(())
    }
}
impl<T: Encode + Ord> TryEncode for BTreeSet<T> {
    fn try_encode(&self, out: &mut Vec<u8>) -> Result<(), EncodeOverflow> {
        try_encode_len_prefix(self.len(), out)?;
        let mut scratches: Vec<Vec<u8>> = Vec::with_capacity(self.len());
        for v in self {
            let mut buf = Vec::new();
            v.encode(&mut buf);
            scratches.push(buf);
        }
        scratches.sort();
        for buf in &scratches {
            out.extend_from_slice(buf);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use crate::composites::encode_len_prefix;
    use crate::{Decode, DecodeError, Encode, from_bytes, to_vec};
    use alloc::boxed::Box;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::sync::Arc;
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
        assert_eq!(err, DecodeError::TagOutOfRange { tag: 2 });
    }
    #[test]
    fn vec_u8_layout() {
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
        let err = from_bytes::<String>(&[2, 0, 0, 0, 0xFF, 0xFE]).unwrap_err();
        assert_eq!(err, DecodeError::InvalidValue);
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
        let mut bad = Vec::new();
        3u32.encode(&mut bad);
        3u32.encode(&mut bad);
        30u8.encode(&mut bad);
        2u32.encode(&mut bad);
        20u8.encode(&mut bad);
        1u32.encode(&mut bad);
        10u8.encode(&mut bad);
        let err = from_bytes::<BTreeMap<u32, u8>>(&bad).unwrap_err();
        assert_eq!(err, DecodeError::InvalidValue);
    }
    #[test]
    fn roundtrip_btreemap_string_u32_mixed_length() {
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        m.insert(String::from("alpha"), 1);
        m.insert(String::from("beta"), 2);
        m.insert(String::from("gamma"), 3);
        rt(m);
    }
    #[test]
    fn canonical_bytes_btreemap_string_u32_mixed_length() {
        let mut m: BTreeMap<String, u32> = BTreeMap::new();
        m.insert(String::from("alpha"), 1);
        m.insert(String::from("beta"), 2);
        m.insert(String::from("gamma"), 3);
        let got = to_vec(&m);
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
        let mut m: BTreeMap<Vec<u8>, u32> = BTreeMap::new();
        m.insert(vec![1], 10);
        m.insert(vec![1, 1], 20);
        m.insert(vec![2], 30);
        rt(m);
    }
    #[test]
    fn decode_btreemap_rejects_misordered() {
        let mut bad = Vec::new();
        encode_len_prefix(3, &mut bad);
        for (k, v) in [("alpha", 1u32), ("beta", 2), ("gamma", 3)] {
            String::from(k).encode(&mut bad);
            v.encode(&mut bad);
        }
        let err = from_bytes::<BTreeMap<String, u32>>(&bad).unwrap_err();
        assert_eq!(err, DecodeError::InvalidValue);
    }
    #[test]
    fn roundtrip_tuple_u8_u16_u32() {
        rt((7u8, 0x1234u16, 0xdead_beefu32));
    }
    #[test]
    fn non_zero_u64_layout_and_roundtrip() {
        use core::num::NonZeroU64;
        let nz = NonZeroU64::new(0x0102_0304_0506_0708).expect("nonzero literal");
        let bytes = to_vec(&nz);
        assert_eq!(bytes, vec![0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01]);
        let back: NonZeroU64 = from_bytes(&bytes).expect("decode");
        assert_eq!(back, nz);
    }
    #[test]
    fn non_zero_u64_rejects_zero_on_decode() {
        let err = from_bytes::<core::num::NonZeroU64>(&[0u8; 8]).unwrap_err();
        assert_eq!(err, DecodeError::InvalidValue);
    }
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
        let mut bad = Vec::new();
        encode_len_prefix(3, &mut bad);
        3u32.encode(&mut bad);
        2u32.encode(&mut bad);
        1u32.encode(&mut bad);
        let err = from_bytes::<BTreeSet<u32>>(&bad).unwrap_err();
        assert_eq!(err, DecodeError::InvalidValue);
    }
    #[test]
    fn btreeset_decode_rejects_duplicate() {
        use alloc::collections::BTreeSet;
        let mut bad = Vec::new();
        encode_len_prefix(2, &mut bad);
        7u32.encode(&mut bad);
        7u32.encode(&mut bad);
        let err = from_bytes::<BTreeSet<u32>>(&bad).unwrap_err();
        assert_eq!(err, DecodeError::InvalidValue);
    }
    #[test]
    fn roundtrip_btreeset_string_mixed_length() {
        use alloc::collections::BTreeSet;
        let mut s: BTreeSet<String> = BTreeSet::new();
        s.insert(String::from("alpha"));
        s.insert(String::from("beta"));
        s.insert(String::from("gamma"));
        rt(s);
    }
    #[test]
    fn canonical_bytes_btreeset_string_mixed_length() {
        use alloc::collections::BTreeSet;
        let mut s: BTreeSet<String> = BTreeSet::new();
        s.insert(String::from("alpha"));
        s.insert(String::from("beta"));
        s.insert(String::from("gamma"));
        let got = to_vec(&s);
        let mut elems: Vec<Vec<u8>> = Vec::new();
        for e in ["alpha", "beta", "gamma"] {
            let mut eb = Vec::new();
            String::from(e).encode(&mut eb);
            elems.push(eb);
        }
        elems.sort();
        let mut expected = Vec::new();
        expected.extend_from_slice(
            &u32::try_from(elems.len())
                .expect("test fixture under u32::MAX")
                .to_le_bytes(),
        );
        for eb in &elems {
            expected.extend_from_slice(eb);
        }
        assert_eq!(got, expected);
    }
    #[test]
    fn roundtrip_btreeset_vec_u8_mixed_length() {
        use alloc::collections::BTreeSet;
        let mut s: BTreeSet<Vec<u8>> = BTreeSet::new();
        s.insert(vec![1]);
        s.insert(vec![1, 1]);
        s.insert(vec![2]);
        rt(s);
    }
    #[test]
    fn roundtrip_box_u64() {
        rt(Box::new(0xDEAD_BEEF_CAFE_F00D_u64));
    }
    #[test]
    fn roundtrip_arc_u32() {
        rt(Arc::new(0xCAFE_BABE_u32));
    }
    /// W1 regression: `encode_len_prefix` panics on lengths that do
    /// not fit in a `u32` LE prefix. Pre-W1, release builds silently
    /// truncated `len as u32`, producing canonically wrong wire bytes
    /// (short prefix + full payload). Gated to 64-bit targets because
    /// `u32::MAX < usize::MAX` only holds for `pointer_width = "64"`.
    #[test]
    #[cfg(target_pointer_width = "64")]
    #[should_panic(expected = "length exceeds u32::MAX")]
    fn encode_len_prefix_panics_when_len_exceeds_u32_max() {
        let mut out = Vec::new();
        encode_len_prefix((u32::MAX as usize) + 1, &mut out);
    }
    /// W1 regression: `encode_len_prefix` accepts `u32::MAX` exactly
    /// and emits the canonical 4-byte LE prefix. Verifies the
    /// representable boundary is unchanged.
    #[test]
    fn encode_len_prefix_accepts_u32_max() {
        let mut out = Vec::new();
        encode_len_prefix(u32::MAX as usize, &mut out);
        assert_eq!(out, u32::MAX.to_le_bytes());
    }
}
