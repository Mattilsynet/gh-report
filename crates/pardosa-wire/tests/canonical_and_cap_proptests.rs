//! Proptests for canonical-ordering invariants and decode-cap
//! adversarial robustness beyond `decode_cap_respected.rs`.
//!
//! Anchors: ADR-0005 (canonical deterministic codec); sorted
//! key-bytes observable in serialised form; `composites.rs`
//! `BTreeMap`/`BTreeSet` encoders sort, decoders reject
//! non-strictly-increasing input.
//!
//! Adds: (1) re-encode proptests pinning sorted-key-bytes across
//! random inputs; (2) `BTreeMap<String, _>` / `BTreeSet<String>`
//! round-trip / cap / junk over mixed-length keys; (3) huge
//! declared `n` vs small cap must reject (no allocation);
//! (4) `Option<T>` and tuple round-trips.
use pardosa_wire::{Decode, Encode, from_bytes_with_cap, to_vec};
use proptest::collection::{btree_map, btree_set, vec};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
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
fn strat_string_short() -> impl Strategy<Value = String> {
    "[a-z0-9]{0,16}".prop_map(String::from)
}
fn strat_btreemap_string_u32() -> impl Strategy<Value = BTreeMap<String, u32>> {
    btree_map(strat_string_short(), any::<u32>(), 0..32)
}
fn strat_btreeset_string() -> impl Strategy<Value = BTreeSet<String>> {
    btree_set(strat_string_short(), 0..32)
}
fn strat_btreeset_vec_u8() -> impl Strategy<Value = BTreeSet<Vec<u8>>> {
    btree_set(vec(any::<u8>(), 0..16), 0..32)
}
fn strat_option_u64() -> impl Strategy<Value = Option<u64>> {
    proptest::option::of(any::<u64>())
}
fn strat_tuple_mixed() -> impl Strategy<Value = (u8, u16, u32, Vec<u8>)> {
    (
        any::<u8>(),
        any::<u16>(),
        any::<u32>(),
        vec(any::<u8>(), 0..32),
    )
}
fn strat_junk_bytes() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..2048)
}
/// A claimed length prefix of `n`, optionally followed by a few payload bytes.
/// Used to construct adversarial Vec/String headers without committing memory.
fn strat_huge_len_prefix() -> impl Strategy<Value = Vec<u8>> {
    (any::<u32>(), vec(any::<u8>(), 0..32)).prop_map(|(n, tail)| {
        let mut v = Vec::with_capacity(4 + tail.len());
        v.extend_from_slice(&n.to_le_bytes());
        v.extend_from_slice(&tail);
        v
    })
}
/// Re-encode the value, then walk the serialized stream and witness that
/// `BTreeMap` key chunks come out in non-decreasing byte order.
///
/// We don't have a public decoder API to split the stream into entries
/// without re-decoding, so instead we exploit the fact that `Decode for
/// BTreeMap` already enforces strict-increase: a successful decode of
/// `bytes` proves the encoder produced canonical bytes. We additionally
/// re-encode the decoded value and assert determinism.
fn assert_canonical_map<K, V>(m: &BTreeMap<K, V>)
where
    K: Encode + Decode + Ord + Clone + core::fmt::Debug,
    V: Encode + Decode + PartialEq + Clone + core::fmt::Debug,
{
    let bytes_a = to_vec(m);
    let back: BTreeMap<K, V> =
        from_bytes_with_cap(&bytes_a, GENEROUS_CAP).expect("canonical bytes must decode");
    assert_eq!(&back, m, "round-trip identity for canonical map");
    let bytes_b = to_vec(&back);
    assert_eq!(bytes_a, bytes_b, "encoder is not deterministic");
}
fn assert_canonical_set<T>(s: &BTreeSet<T>)
where
    T: Encode + Decode + Ord + Clone + core::fmt::Debug,
{
    let bytes_a = to_vec(s);
    let back: BTreeSet<T> =
        from_bytes_with_cap(&bytes_a, GENEROUS_CAP).expect("canonical bytes must decode");
    assert_eq!(&back, s, "round-trip identity for canonical set");
    let bytes_b = to_vec(&back);
    assert_eq!(bytes_a, bytes_b, "encoder is not deterministic");
}
proptest! {
    #![proptest_config(ProptestConfig { cases : 256, ..ProptestConfig::default() })]
    #[test] fn canonical_btreemap_string_u32(m in strat_btreemap_string_u32()) {
    assert_canonical_map(& m); } #[test] fn canonical_btreeset_string(s in
    strat_btreeset_string()) { assert_canonical_set(& s); } #[test] fn
    canonical_btreeset_vec_u8(s in strat_btreeset_vec_u8()) { assert_canonical_set(& s);
    } #[test] fn round_trip_btreemap_string_u32(m in strat_btreemap_string_u32()) {
    round_trip(& m); } #[test] fn round_trip_btreeset_string(s in
    strat_btreeset_string()) { round_trip(& s); } #[test] fn round_trip_btreeset_vec_u8(s
    in strat_btreeset_vec_u8()) { round_trip(& s); } #[test] fn round_trip_option_u64(o
    in strat_option_u64()) { round_trip(& o); } #[test] fn round_trip_tuple_mixed(t in
    strat_tuple_mixed()) { round_trip(& t); } #[test] fn cap_clamp_btreemap_string_u32(m
    in strat_btreemap_string_u32(), cap in strat_small_cap()) { let bytes = to_vec(& m);
    if let Ok(decoded) = from_bytes_with_cap::< BTreeMap < String, u32 >> (& bytes, cap)
    { prop_assert_eq!(decoded, m); } } #[test] fn cap_clamp_btreeset_string(s in
    strat_btreeset_string(), cap in strat_small_cap()) { let bytes = to_vec(& s); if let
    Ok(decoded) = from_bytes_with_cap::< BTreeSet < String >> (& bytes, cap) {
    prop_assert_eq!(decoded, s); } } #[test] fn
    huge_len_prefix_vec_u8_terminates_under_small_cap(bytes in strat_huge_len_prefix(),
    cap in strat_small_cap()) { decode_terminates::< Vec < u8 >> (& bytes, cap); }
    #[test] fn huge_len_prefix_string_terminates_under_small_cap(bytes in
    strat_huge_len_prefix(), cap in strat_small_cap()) { decode_terminates::< String > (&
    bytes, cap); } #[test] fn huge_len_prefix_btreeset_terminates_under_small_cap(bytes
    in strat_huge_len_prefix(), cap in strat_small_cap()) { decode_terminates::< BTreeSet
    < u32 >> (& bytes, cap); } #[test] fn junk_bytes_terminates_btreemap_string_u32(bytes
    in strat_junk_bytes(), cap in strat_small_cap()) { decode_terminates::< BTreeMap <
    String, u32 >> (& bytes, cap); } #[test] fn
    junk_bytes_terminates_btreeset_string(bytes in strat_junk_bytes(), cap in
    strat_small_cap()) { decode_terminates::< BTreeSet < String >> (& bytes, cap); }
    #[test] fn junk_bytes_terminates_option_u64(bytes in strat_junk_bytes(), cap in
    strat_small_cap()) { decode_terminates::< Option < u64 >> (& bytes, cap); } #[test]
    fn junk_bytes_terminates_tuple_mixed(bytes in strat_junk_bytes(), cap in
    strat_small_cap()) { decode_terminates::< (u8, u16, u32, Vec < u8 >) > (& bytes,
    cap); }
}
