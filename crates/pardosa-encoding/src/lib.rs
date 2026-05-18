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

mod decoder;
pub use decoder::{DEFAULT_DECODE_CAP, Decoder};

mod error;
pub use error::EventError;

mod traits;
pub use traits::{Decode, Encode, from_bytes, from_bytes_with_cap, to_vec};

mod primitives;

mod composites;

mod foreign;

mod precursor;
#[cfg(feature = "blake3")]
pub use precursor::precursor_hash_of;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::vec;
    use alloc::vec::Vec;

    use crate::composites::encode_len_prefix;

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
