use pardosa_wire::{Decode, DecodeError, Encode, from_bytes_with_cap, to_vec};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use std::collections::BTreeMap;
const GENEROUS_CAP: usize = 1 << 20;
fn round_trip<T>(value: &T)
where
    T: Decode + Encode + PartialEq + core::fmt::Debug,
{
    let bytes = to_vec(value);
    let decoded: T =
        from_bytes_with_cap(&bytes, GENEROUS_CAP).expect("generous cap should suffice");
    assert_eq!(&decoded, value, "round-trip mismatch");
}
fn decode_terminates<T: Decode>(bytes: &[u8], cap: usize) {
    let _ = from_bytes_with_cap::<T>(bytes, cap);
}
fn strat_vec_u8() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..256)
}
fn strat_vec_vec_u32() -> impl Strategy<Value = Vec<Vec<u32>>> {
    vec(vec(any::<u32>(), 0..64), 0..16)
}
fn strat_btree_map_u32_vec_u8() -> impl Strategy<Value = BTreeMap<u32, Vec<u8>>> {
    btree_map(any::<u32>(), vec(any::<u8>(), 0..64), 0..32)
}
fn strat_array_u8_32() -> impl Strategy<Value = [u8; 32]> {
    any::<[u8; 32]>()
}
fn strat_small_cap() -> impl Strategy<Value = usize> {
    prop_oneof![
        Just(0_usize),
        Just(4),
        Just(16),
        Just(64),
        Just(256),
        0_usize..1024
    ]
}
fn strat_junk_bytes() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..2048)
}
proptest! {
    #![proptest_config(ProptestConfig { cases : 256, ..ProptestConfig::default() })]
    #[test] fn round_trip_vec_u8(v in strat_vec_u8()) { round_trip(& v); } #[test] fn
    round_trip_vec_vec_u32(v in strat_vec_vec_u32()) { round_trip(& v); } #[test] fn
    round_trip_btree_map_u32_vec_u8(v in strat_btree_map_u32_vec_u8()) { round_trip(& v);
    } #[test] fn round_trip_array_u8_32(v in strat_array_u8_32()) { round_trip(& v); }
    #[test] fn cap_clamp_vec_u8(v in strat_vec_u8(), cap in strat_small_cap()) { let
    bytes = to_vec(& v); if let Ok(decoded) = from_bytes_with_cap::< Vec < u8 >> (&
    bytes, cap) { prop_assert_eq!(decoded, v); } } #[test] fn cap_clamp_vec_vec_u32(v in
    strat_vec_vec_u32(), cap in strat_small_cap()) { let bytes = to_vec(& v); if let
    Ok(decoded) = from_bytes_with_cap::< Vec < Vec < u32 >>> (& bytes, cap) {
    prop_assert_eq!(decoded, v); } } #[test] fn cap_clamp_btree_map_u32_vec_u8(v in
    strat_btree_map_u32_vec_u8(), cap in strat_small_cap()) { let bytes = to_vec(& v); if
    let Ok(decoded) = from_bytes_with_cap::< BTreeMap < u32, Vec < u8 >>> (& bytes, cap)
    { prop_assert_eq!(decoded, v); } } #[test] fn junk_bytes_terminates_vec_u8(bytes in
    strat_junk_bytes(), cap in strat_small_cap()) { decode_terminates::< Vec < u8 >> (&
    bytes, cap); } #[test] fn junk_bytes_terminates_vec_vec_u32(bytes in
    strat_junk_bytes(), cap in strat_small_cap()) { decode_terminates::< Vec < Vec < u32
    >>> (& bytes, cap); } #[test] fn junk_bytes_terminates_btree_map(bytes in
    strat_junk_bytes(), cap in strat_small_cap()) { decode_terminates::< BTreeMap < u32,
    Vec < u8 >>> (& bytes, cap); } #[test] fn junk_bytes_terminates_array_u8_32(bytes in
    strat_junk_bytes(), cap in strat_small_cap()) { decode_terminates::< [u8; 32] > (&
    bytes, cap); }
}
#[test]
fn cap_zero_rejects_nonempty_payload() {
    let bytes = to_vec(&vec![1u8, 2, 3]);
    let err =
        from_bytes_with_cap::<Vec<u8>>(&bytes, 0).expect_err("cap=0 must reject 3-byte payload");
    let _: DecodeError = err;
}
