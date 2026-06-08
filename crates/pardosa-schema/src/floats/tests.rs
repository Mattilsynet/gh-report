use super::*;
use core::f32::consts::PI as PI_F32;
use core::f64::consts::PI as PI_F64;
use pardosa_wire::ValidationCost;
use pardosa_wire::{from_bytes, to_vec};
#[test]
fn real_f32_roundtrip() {
    for &v in &[PI_F32, 0.0_f32, f32::MIN, f32::MAX] {
        let w = RealF32::try_from(v).unwrap();
        let wire = to_vec(&w);
        let back: RealF32 = from_bytes(&wire).unwrap();
        assert_eq!(w, back, "RealF32 roundtrip failed for {v}");
    }
}
#[test]
fn real_f32_rejects_nan() {
    assert_eq!(
        RealF32::try_from(f32::NAN).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn real_f32_rejects_infinities() {
    assert_eq!(
        RealF32::try_from(f32::INFINITY).unwrap_err(),
        DomainError::NotReal
    );
    assert_eq!(
        RealF32::try_from(f32::NEG_INFINITY).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn real_f32_rejects_subnormal() {
    let sub = f32::MIN_POSITIVE / 2.0;
    assert!(sub.is_subnormal(), "test setup: expected subnormal");
    assert_eq!(RealF32::try_from(sub).unwrap_err(), DomainError::NotReal);
}
#[test]
fn real_f32_normalises_negative_zero() {
    let w = RealF32::try_from(-0.0_f32).unwrap();
    assert!(
        !w.get().is_sign_negative(),
        "RealF32 must normalise -0 to +0"
    );
}
#[test]
fn real_f64_roundtrip() {
    for &v in &[PI_F64, 0.0_f64, f64::MIN, f64::MAX] {
        let w = RealF64::try_from(v).unwrap();
        let wire = to_vec(&w);
        let back: RealF64 = from_bytes(&wire).unwrap();
        assert_eq!(w, back, "RealF64 roundtrip failed for {v}");
    }
}
#[test]
fn real_f64_rejects_nan() {
    assert_eq!(
        RealF64::try_from(f64::NAN).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn real_f64_rejects_infinities() {
    assert_eq!(
        RealF64::try_from(f64::INFINITY).unwrap_err(),
        DomainError::NotReal
    );
    assert_eq!(
        RealF64::try_from(f64::NEG_INFINITY).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn real_f64_rejects_subnormal() {
    let sub = f64::MIN_POSITIVE / 2.0;
    assert!(sub.is_subnormal());
    assert_eq!(RealF64::try_from(sub).unwrap_err(), DomainError::NotReal);
}
#[test]
fn real_f64_normalises_negative_zero() {
    let w = RealF64::try_from(-0.0_f64).unwrap();
    assert!(!w.get().is_sign_negative());
}
#[test]
fn ordered_f32_roundtrip() {
    for &v in &[PI_F32, 0.0_f32, f32::MIN, f32::MAX, -1.0_f32, 1.0_f32] {
        let w = OrderedF32::try_from(v).unwrap();
        let wire = to_vec(&w);
        let back: OrderedF32 = from_bytes(&wire).unwrap();
        assert_eq!(w, back, "OrderedF32 roundtrip failed for {v}");
    }
}
#[test]
fn ordered_f32_rejects_nan() {
    assert_eq!(
        OrderedF32::try_from(f32::NAN).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn ordered_f32_rejects_infinities() {
    assert_eq!(
        OrderedF32::try_from(f32::INFINITY).unwrap_err(),
        DomainError::NotReal
    );
    assert_eq!(
        OrderedF32::try_from(f32::NEG_INFINITY).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn ordered_f32_rejects_subnormal() {
    let sub = f32::MIN_POSITIVE / 2.0;
    assert!(sub.is_subnormal());
    assert_eq!(OrderedF32::try_from(sub).unwrap_err(), DomainError::NotReal);
}
#[test]
fn ordered_f32_total_ordering() {
    let mut vals: Vec<OrderedF32> = [-1.0_f32, 0.0, 1.0, 100.0, -100.0, f32::MIN, f32::MAX]
        .iter()
        .map(|&v| OrderedF32::try_from(v).unwrap())
        .collect();
    vals.sort();
    let raw: Vec<f32> = vals.iter().map(|w| w.get()).collect();
    for win in raw.windows(2) {
        assert!(
            win[0] <= win[1],
            "ordering violated: {} > {}",
            win[0],
            win[1]
        );
    }
}
#[test]
fn ordered_f32_negative_zero_normalised() {
    let pos = OrderedF32::try_from(0.0_f32).unwrap();
    let neg = OrderedF32::try_from(-0.0_f32).unwrap();
    assert_eq!(pos, neg);
    assert_eq!(pos.cmp(&neg), core::cmp::Ordering::Equal);
}
#[test]
fn ordered_f64_roundtrip() {
    for &v in &[PI_F64, 0.0_f64, f64::MIN, f64::MAX, -1.0_f64, 1.0_f64] {
        let w = OrderedF64::try_from(v).unwrap();
        let wire = to_vec(&w);
        let back: OrderedF64 = from_bytes(&wire).unwrap();
        assert_eq!(w, back, "OrderedF64 roundtrip failed for {v}");
    }
}
#[test]
fn ordered_f64_rejects_nan() {
    assert_eq!(
        OrderedF64::try_from(f64::NAN).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn ordered_f64_rejects_infinities() {
    assert_eq!(
        OrderedF64::try_from(f64::INFINITY).unwrap_err(),
        DomainError::NotReal
    );
    assert_eq!(
        OrderedF64::try_from(f64::NEG_INFINITY).unwrap_err(),
        DomainError::NotReal
    );
}
#[test]
fn ordered_f64_rejects_subnormal() {
    let sub = f64::MIN_POSITIVE / 2.0;
    assert!(sub.is_subnormal());
    assert_eq!(OrderedF64::try_from(sub).unwrap_err(), DomainError::NotReal);
}
#[test]
fn ordered_f64_total_ordering() {
    let mut vals: Vec<OrderedF64> = [-1.0_f64, 0.0, 1.0, 100.0, -100.0, f64::MIN, f64::MAX]
        .iter()
        .map(|&v| OrderedF64::try_from(v).unwrap())
        .collect();
    vals.sort();
    let raw: Vec<f64> = vals.iter().map(|w| w.get()).collect();
    for win in raw.windows(2) {
        assert!(
            win[0] <= win[1],
            "ordering violated: {} > {}",
            win[0],
            win[1]
        );
    }
}
#[test]
fn ordered_f64_negative_zero_normalised() {
    let pos = OrderedF64::try_from(0.0_f64).unwrap();
    let neg = OrderedF64::try_from(-0.0_f64).unwrap();
    assert_eq!(pos, neg);
    assert_eq!(pos.cmp(&neg), core::cmp::Ordering::Equal);
}
#[test]
fn all_wrappers_validation_cost_cheap() {
    assert_eq!(<RealF32 as Validate>::COST, ValidationCost::Cheap);
    assert_eq!(<RealF64 as Validate>::COST, ValidationCost::Cheap);
    assert_eq!(<OrderedF32 as Validate>::COST, ValidationCost::Cheap);
    assert_eq!(<OrderedF64 as Validate>::COST, ValidationCost::Cheap);
}
#[test]
fn wire_compat_real_f64_vs_raw() {
    let v = PI_F64;
    let wrapped = to_vec(&RealF64::try_from(v).unwrap());
    let raw = to_vec(&v);
    assert_eq!(wrapped, raw, "RealF64 wire not byte-identical to f64");
}
#[test]
fn wire_compat_ordered_f32_vs_raw() {
    let v = 1.23_f32;
    let wrapped = to_vec(&OrderedF32::try_from(v).unwrap());
    let raw = to_vec(&v);
    assert_eq!(wrapped, raw, "OrderedF32 wire not byte-identical to f32");
}
#[test]
fn event_f32_classifies_inputs() {
    assert!(matches!(
        EventF32::from_f32(f32::NAN).unwrap(),
        EventF32::NaN
    ));
    assert!(matches!(
        EventF32::from_f32(f32::INFINITY).unwrap(),
        EventF32::PosInf
    ));
    assert!(matches!(
        EventF32::from_f32(f32::NEG_INFINITY).unwrap(),
        EventF32::NegInf
    ));
    assert!(matches!(
        EventF32::from_f32(1.5_f32).unwrap(),
        EventF32::Finite(_)
    ));
}
#[test]
fn event_f32_finite_subnormal_rejected() {
    let sub = f32::MIN_POSITIVE / 2.0;
    assert!(sub.is_subnormal());
    assert!(matches!(
        EventF32::from_f32(sub),
        Err(crate::error::DomainError::NotReal)
    ));
}
#[test]
fn event_f32_layout_tags() {
    assert_eq!(to_vec(&EventF32::NaN), vec![0u8]);
    assert_eq!(to_vec(&EventF32::NegInf), vec![1u8]);
    assert_eq!(to_vec(&EventF32::PosInf), vec![3u8]);
    let v = 1.5_f32;
    let wire = to_vec(&EventF32::Finite(OrderedF32::try_from(v).unwrap()));
    let mut expected = vec![2u8];
    expected.extend_from_slice(&v.to_le_bytes());
    assert_eq!(wire, expected, "Finite layout: tag 2 + 4 LE bytes");
    assert_eq!(wire.len(), 1 + 4);
}
/// Bucket compared by `mem::discriminant` rather than `PartialEq`
/// because the `NaN` variant's `to_f32()` projection is `f32::NAN`,
/// which is not reflexive under IEEE-754 equality; variant identity
/// is the semantic equality we mean here.
#[test]
fn event_f32_roundtrip_all_variants() {
    for v in [
        EventF32::NaN,
        EventF32::NegInf,
        EventF32::PosInf,
        EventF32::Finite(OrderedF32::try_from(PI_F32).unwrap()),
        EventF32::Finite(OrderedF32::try_from(0.0_f32).unwrap()),
    ] {
        let bytes = to_vec(&v);
        let back: EventF32 = from_bytes(&bytes).unwrap();
        assert_eq!(
            core::mem::discriminant(&v),
            core::mem::discriminant(&back),
            "EventF32 roundtrip mismatch"
        );
        if let (EventF32::Finite(a), EventF32::Finite(b)) = (v, back) {
            assert_eq!(a, b);
        }
    }
}
#[test]
fn event_f32_unknown_tag_rejected() {
    let err = from_bytes::<EventF32>(&[4u8]).unwrap_err();
    assert_eq!(err, pardosa_wire::DecodeError::TagOutOfRange { tag: 4 });
}
#[test]
fn event_f64_layout_tags() {
    assert_eq!(to_vec(&EventF64::NaN), vec![0u8]);
    assert_eq!(to_vec(&EventF64::NegInf), vec![1u8]);
    assert_eq!(to_vec(&EventF64::PosInf), vec![3u8]);
    let v = 1.5_f64;
    let wire = to_vec(&EventF64::Finite(OrderedF64::try_from(v).unwrap()));
    let mut expected = vec![2u8];
    expected.extend_from_slice(&v.to_le_bytes());
    assert_eq!(wire, expected, "Finite layout: tag 2 + 8 LE bytes");
    assert_eq!(wire.len(), 1 + 8);
}
#[test]
fn event_f64_roundtrip_all_variants() {
    for v in [
        EventF64::NaN,
        EventF64::NegInf,
        EventF64::PosInf,
        EventF64::Finite(OrderedF64::try_from(PI_F64).unwrap()),
    ] {
        let bytes = to_vec(&v);
        let back: EventF64 = from_bytes(&bytes).unwrap();
        assert_eq!(core::mem::discriminant(&v), core::mem::discriminant(&back));
        if let (EventF64::Finite(a), EventF64::Finite(b)) = (v, back) {
            assert_eq!(a, b);
        }
    }
}
#[test]
fn event_f64_unknown_tag_rejected() {
    let err = from_bytes::<EventF64>(&[7u8]).unwrap_err();
    assert_eq!(err, pardosa_wire::DecodeError::TagOutOfRange { tag: 7 });
}
/// `Finite` cannot hold a subnormal because construction routes
/// through `OrderedF32::try_from` (covered elsewhere); this test
/// confirms `validate()` on `Finite` delegates to
/// `OrderedF32::validate()` and the tag-only variants are always
/// `Ok`.
#[test]
fn event_float_validate_finite_subnormal_via_validate() {
    let v = EventF32::Finite(OrderedF32::try_from(1.0_f32).unwrap());
    assert!(v.validate().is_ok());
    assert!(EventF32::NaN.validate().is_ok());
    assert!(EventF32::PosInf.validate().is_ok());
    assert!(EventF32::NegInf.validate().is_ok());
}
#[test]
fn event_float_validation_cost_cheap() {
    assert_eq!(<EventF32 as Validate>::COST, ValidationCost::Cheap);
    assert_eq!(<EventF64 as Validate>::COST, ValidationCost::Cheap);
}
/// Pins [`EventF32::SCHEMA_SOURCE`] to the exact enum-form literal
/// containing the name, variant identifiers, `#[repr(u8)]`
/// discriminants, and the `Finite` payload type. Any rename,
/// reorder, renumber, or payload swap must update this string and
/// is therefore an ADR-0009 breaking change (caught here at test
/// time, not at decode time).
#[test]
fn event_f32_schema_source_pins_discriminants_and_payload() {
    let src = <EventF32 as GenomeSafe>::SCHEMA_SOURCE;
    assert_eq!(
        src,
        "enum EventF32 { NaN = 0, NegInf = 1, Finite(OrderedF32) = 2, PosInf = 3 }"
    );
    for needle in [
        "EventF32",
        "NaN = 0",
        "NegInf = 1",
        "Finite(OrderedF32) = 2",
        "PosInf = 3",
    ] {
        assert!(src.contains(needle), "SCHEMA_SOURCE missing `{needle}`");
    }
}
/// Pins [`EventF64::SCHEMA_SOURCE`] in the same way as `EventF32`;
/// the `Finite` arm carries `OrderedF64`.
#[test]
fn event_f64_schema_source_pins_discriminants_and_payload() {
    let src = <EventF64 as GenomeSafe>::SCHEMA_SOURCE;
    assert_eq!(
        src,
        "enum EventF64 { NaN = 0, NegInf = 1, Finite(OrderedF64) = 2, PosInf = 3 }"
    );
    for needle in [
        "EventF64",
        "NaN = 0",
        "NegInf = 1",
        "Finite(OrderedF64) = 2",
        "PosInf = 3",
    ] {
        assert!(src.contains(needle), "SCHEMA_SOURCE missing `{needle}`");
    }
}
/// Pins [`EventF32::SCHEMA_HASH`] to the exact combine expression:
/// `combine(hash(SCHEMA_SOURCE), OrderedF32::SCHEMA_HASH)`. If the
/// payload's schema hash changes (e.g. `OrderedF32` inner
/// representation revised), `EventF32::SCHEMA_HASH` changes too —
/// the property the mission contract requires. Restated here so
/// the composition cannot be silently reduced back to a bare
/// name-only hash.
#[test]
fn event_f32_schema_hash_composes_source_and_payload() {
    let expected = crate::genome_safe::schema_hash_combine(
        crate::genome_safe::schema_hash_bytes(<EventF32 as GenomeSafe>::SCHEMA_SOURCE.as_bytes()),
        <OrderedF32 as GenomeSafe>::SCHEMA_HASH,
    );
    assert_eq!(<EventF32 as GenomeSafe>::SCHEMA_HASH, expected);
    assert_ne!(
        <EventF32 as GenomeSafe>::SCHEMA_HASH,
        crate::genome_safe::schema_hash_bytes(b"EventF32"),
        "hash must compose payload, not be bare name-only"
    );
}
/// Pins [`EventF64::SCHEMA_HASH`] to its combine expression; see
/// `event_f32_schema_hash_composes_source_and_payload`.
#[test]
fn event_f64_schema_hash_composes_source_and_payload() {
    let expected = crate::genome_safe::schema_hash_combine(
        crate::genome_safe::schema_hash_bytes(<EventF64 as GenomeSafe>::SCHEMA_SOURCE.as_bytes()),
        <OrderedF64 as GenomeSafe>::SCHEMA_HASH,
    );
    assert_eq!(<EventF64 as GenomeSafe>::SCHEMA_HASH, expected);
    assert_ne!(
        <EventF64 as GenomeSafe>::SCHEMA_HASH,
        crate::genome_safe::schema_hash_bytes(b"EventF64"),
        "hash must compose payload, not be bare name-only"
    );
}
/// `EventF32` and `EventF64` must produce distinct schema hashes
/// even though their variant structure is identical — the payload
/// hash (`OrderedF32` vs `OrderedF64`) must propagate into the
/// outer hash, otherwise the two types would collide as
/// `pgno`-payload identities.
#[test]
fn event_f32_and_f64_schema_hashes_distinct() {
    assert_ne!(
        <EventF32 as GenomeSafe>::SCHEMA_HASH,
        <EventF64 as GenomeSafe>::SCHEMA_HASH
    );
}
#[test]
fn event_float_nan_payload_not_preserved() {
    let bytes = to_vec(&EventF32::NaN);
    assert_eq!(bytes, vec![0u8], "NaN encodes as 1-byte tag only");
    let back: EventF32 = from_bytes(&bytes).unwrap();
    assert!(matches!(back, EventF32::NaN));
    assert!(back.to_f32().is_nan());
}
