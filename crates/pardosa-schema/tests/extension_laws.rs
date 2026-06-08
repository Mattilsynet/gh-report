//! Extension-law harness coverage for `pardosa-schema`'s in-tree
//! `Validate` implementors (o1ix.7, roadmap correctness 7).
//!
//! The harness lives in `pardosa-wire::laws`; this file exercises it
//! against the bounded-newtype, float, and `CharScalar` types that
//! `pardosa-schema` exposes. Any future codec evolution that breaks
//! roundtrip, determinism, trailing-byte rejection, truncated-input
//! rejection, cap-respect, or `Validate` consistency on these types
//! surfaces here.
#![cfg(test)]
use pardosa_schema::{
    CharScalar, EventBytes, EventF32, EventF64, EventString, EventVec, NonEmptyEventString,
    OrderedF32, OrderedF64, RealF32, RealF64,
};
use pardosa_wire::laws;
#[test]
fn event_string_obeys_all_laws() {
    let samples: [EventString<32>; 3] = [
        EventString::try_from(String::new()).unwrap(),
        EventString::try_from(String::from("hi")).unwrap(),
        EventString::try_from(String::from("0123456789abcdef")).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn event_bytes_obeys_all_laws() {
    let samples: [EventBytes<32>; 3] = [
        EventBytes::try_from(Vec::<u8>::new()).unwrap(),
        EventBytes::try_from(vec![1u8, 2, 3]).unwrap(),
        EventBytes::try_from((0u8..=20).collect::<Vec<u8>>()).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn event_vec_u32_obeys_all_laws() {
    let samples: [EventVec<u32, 8>; 3] = [
        EventVec::try_from(Vec::<u32>::new()).unwrap(),
        EventVec::try_from(vec![1u32, 2, 3, 4]).unwrap(),
        EventVec::try_from(vec![0u32; 8]).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn nonempty_event_string_obeys_all_laws() {
    let samples: [NonEmptyEventString<32>; 2] = [
        NonEmptyEventString::try_new("x").unwrap(),
        NonEmptyEventString::try_new("foo bar baz").unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn real_f32_obeys_all_laws() {
    let samples = [
        RealF32::try_from(0.0f32).unwrap(),
        RealF32::try_from(1.5f32).unwrap(),
        RealF32::try_from(-1.0e3f32).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn real_f64_obeys_all_laws() {
    let samples = [
        RealF64::try_from(0.0f64).unwrap(),
        RealF64::try_from(1.5f64).unwrap(),
        RealF64::try_from(-1.0e9f64).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn ordered_f32_obeys_all_laws() {
    let samples = [
        OrderedF32::try_from(0.0f32).unwrap(),
        OrderedF32::try_from(-0.0f32).unwrap(),
        OrderedF32::try_from(1.5f32).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn ordered_f64_obeys_all_laws() {
    let samples = [
        OrderedF64::try_from(0.0f64).unwrap(),
        OrderedF64::try_from(-0.0f64).unwrap(),
        OrderedF64::try_from(2.5e10f64).unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn char_scalar_obeys_all_laws() {
    let samples = [
        CharScalar::try_from('a').unwrap(),
        CharScalar::try_from('Z').unwrap(),
        CharScalar::try_from('日').unwrap(),
        CharScalar::try_from('\u{D7FF}').unwrap(),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn event_f32_obeys_all_laws() {
    let samples = [
        EventF32::NaN,
        EventF32::NegInf,
        EventF32::PosInf,
        EventF32::Finite(OrderedF32::try_from(0.0f32).unwrap()),
        EventF32::Finite(OrderedF32::try_from(1.5f32).unwrap()),
        EventF32::Finite(OrderedF32::try_from(-1.0e3f32).unwrap()),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
#[test]
fn event_f64_obeys_all_laws() {
    let samples = [
        EventF64::NaN,
        EventF64::NegInf,
        EventF64::PosInf,
        EventF64::Finite(OrderedF64::try_from(0.0f64).unwrap()),
        EventF64::Finite(OrderedF64::try_from(1.5f64).unwrap()),
        EventF64::Finite(OrderedF64::try_from(-1.0e9f64).unwrap()),
    ];
    laws::all_laws_validate_for_samples(&samples);
}
