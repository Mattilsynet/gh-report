use super::*;
#[test]
fn index_zero_value() {
    assert_eq!(Index::ZERO.value(), 0);
}
#[test]
fn index_display() {
    assert_eq!(format!("{}", Index::new(42)), "42");
    assert_eq!(format!("{}", Index::new(u64::MAX)), format!("{}", u64::MAX));
}
#[test]
fn index_new_accepts_u64_max() {
    let i = Index::new(u64::MAX);
    assert_eq!(i.value(), u64::MAX);
}
#[test]
fn index_checked_next() {
    let i = Index::new(0);
    assert_eq!(i.checked_next().unwrap().value(), 1);
}
#[test]
fn index_checked_next_at_max_minus_1() {
    let i = Index::new(u64::MAX - 1);
    assert_eq!(i.checked_next().unwrap().value(), u64::MAX);
}
/// W3 (roadmap correctness 2026-05-24): `TryFrom<Index> for
/// usize` is infallible for values that fit in `usize`. On
/// 64-bit hosts this covers the entire `u64` domain; on 32-bit
/// hosts it covers `[0, u32::MAX]`.
#[test]
fn index_try_from_usize_ok_for_small_value() {
    assert_eq!(usize::try_from(Index::new(0)), Ok(0));
    assert_eq!(usize::try_from(Index::new(42)), Ok(42));
}
/// W3: on 64-bit targets `u64::MAX` fits `usize`, so the
/// conversion succeeds; on 32-bit targets it returns
/// `Err(IndexTooLargeForUsize(u64::MAX))` carrying the raw
/// decoded value losslessly so the caller can surface a typed
/// out-of-bounds error.
#[test]
fn index_try_from_usize_handles_u64_max_per_target_width() {
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(
            usize::try_from(Index::new(u64::MAX)),
            Ok(usize::try_from(u64::MAX).expect("64-bit: u64::MAX fits usize"))
        );
    }
    #[cfg(target_pointer_width = "32")]
    {
        assert_eq!(
            usize::try_from(Index::new(u64::MAX)),
            Err(IndexTooLargeForUsize(u64::MAX))
        );
    }
}
#[test]
fn index_checked_next_at_max_overflows() {
    let i = Index::new(u64::MAX);
    assert!(i.checked_next().is_err());
}
#[test]
fn index_serde_roundtrip_transparent() {
    let i = Index::new(42);
    let json = serde_json::to_string(&i).unwrap();
    assert_eq!(json, "42");
    let back: Index = serde_json::from_str(&json).unwrap();
    assert_eq!(back, i);
}
#[test]
fn index_serde_accepts_u64_max() {
    let json = "18446744073709551615";
    let i: Index = serde_json::from_str(json).unwrap();
    assert_eq!(i.value(), u64::MAX);
}
#[test]
fn precursor_genesis_is_genesis() {
    assert!(Precursor::Genesis.is_genesis());
    assert_eq!(Precursor::Genesis.as_index(), None);
}
#[test]
fn precursor_of_is_not_genesis() {
    let p = Precursor::Of(Index::new(7));
    assert!(!p.is_genesis());
    assert_eq!(p.as_index(), Some(Index::new(7)));
}
#[test]
fn precursor_display() {
    assert_eq!(format!("{}", Precursor::Genesis), "Genesis");
    assert_eq!(format!("{}", Precursor::Of(Index::new(5))), "Of(5)");
}
#[test]
fn precursor_genesis_encodes_as_single_zero_byte() {
    use pardosa_wire::to_vec;
    let bytes = to_vec(&Precursor::Genesis);
    assert_eq!(bytes, vec![0u8]);
}
#[test]
fn precursor_of_encodes_as_tag_then_u64_le() {
    use pardosa_wire::to_vec;
    let bytes = to_vec(&Precursor::Of(Index::new(7)));
    let mut expected = vec![1u8];
    expected.extend_from_slice(&7u64.to_le_bytes());
    assert_eq!(bytes, expected);
}
#[test]
fn precursor_roundtrip_via_wire() {
    use pardosa_wire::{from_bytes, to_vec};
    for p in [
        Precursor::Genesis,
        Precursor::Of(Index::new(0)),
        Precursor::Of(Index::new(42)),
        Precursor::Of(Index::new(u64::MAX)),
    ] {
        let bytes = to_vec(&p);
        let back: Precursor = from_bytes(&bytes).expect("decode Precursor");
        assert_eq!(back, p);
    }
}
#[test]
fn precursor_invalid_tag_rejected() {
    use pardosa_wire::from_bytes;
    let err = from_bytes::<Precursor>(&[2u8]).unwrap_err();
    assert!(
        matches!(err, pardosa_wire::DecodeError::TagOutOfRange { tag: 2 }),
        "expected TagOutOfRange(2), got: {err:?}"
    );
}
#[test]
fn precursor_serde_roundtrip() {
    let g = Precursor::Genesis;
    let o = Precursor::Of(Index::new(7));
    let gj = serde_json::to_string(&g).unwrap();
    let oj = serde_json::to_string(&o).unwrap();
    let gb: Precursor = serde_json::from_str(&gj).unwrap();
    let ob: Precursor = serde_json::from_str(&oj).unwrap();
    assert_eq!(gb, g);
    assert_eq!(ob, o);
}
#[test]
fn event_id_to_line_position_is_value_cast_on_64bit() {
    assert_eq!(event_id_to_line_position(EventId::new(0)), Ok(0));
    assert_eq!(event_id_to_line_position(EventId::new(42)), Ok(42));
    #[cfg(target_pointer_width = "64")]
    assert_eq!(
        event_id_to_line_position(EventId::new(u64::MAX)),
        Ok(usize::try_from(u64::MAX).expect("64-bit"))
    );
}
#[test]
fn fiber_id_checked_next() {
    let d = FiberId::new(0);
    assert_eq!(d.checked_next().unwrap().value(), 1);
}
#[test]
fn fiber_id_overflow() {
    let d = FiberId::new(u64::MAX);
    assert!(d.checked_next().is_err());
}
#[test]
fn event_constructor_and_accessors_genesis() {
    let event = Event::new_unchecked(
        1,
        FiberId::new(5),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "created".to_string(),
    );
    assert_eq!(event.event_id(), EventId::new(1));
    assert_eq!(event.fiber_id(), FiberId::new(5));
    assert!(!event.detached());
    assert_eq!(event.precursor(), Precursor::Genesis);
    assert_eq!(event.domain_event(), "created");
}
#[test]
fn event_with_precursor_of() {
    let event = Event::new_unchecked(
        2,
        FiberId::new(5),
        false,
        Precursor::Of(Index::new(0)),
        [0u8; 32],
        "updated".to_string(),
    );
    assert_eq!(event.event_id(), EventId::new(2));
    assert_eq!(event.precursor(), Precursor::Of(Index::new(0)));
    assert_eq!(event.precursor().as_index(), Some(Index::new(0)));
}
#[test]
fn event_serde_roundtrip_genesis() {
    let event = Event::new_unchecked(
        1,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "created".to_string(),
    );
    let json = serde_json::to_string(&event).unwrap();
    let back: Event<String> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.event_id(), event.event_id());
    assert_eq!(back.fiber_id(), event.fiber_id());
    assert_eq!(back.domain_event(), "created");
    assert_eq!(back.precursor(), Precursor::Genesis);
}
#[test]
fn event_with_precursor_serde_roundtrip() {
    let event = Event::new_unchecked(
        2,
        FiberId::new(1),
        false,
        Precursor::Of(Index::new(0)),
        [0u8; 32],
        "updated".to_string(),
    );
    let json = serde_json::to_string(&event).unwrap();
    let back: Event<String> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.precursor(), Precursor::Of(Index::new(0)));
}
#[test]
fn event_detached_flag() {
    let event = Event::new_unchecked(
        3,
        FiberId::new(1),
        true,
        Precursor::Of(Index::new(1)),
        [0u8; 32],
        "detached".to_string(),
    );
    assert!(event.detached());
}
#[test]
fn event_precursor_hash_accessor() {
    let event = Event::new_unchecked(
        7,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "hashed".to_string(),
    );
    assert_eq!(event.precursor_hash(), [0u8; 32]);
}
#[test]
fn event_precursor_hash_nonzero_roundtrip() {
    let hash: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];
    let event = Event::new_unchecked(
        8,
        FiberId::new(1),
        false,
        Precursor::Of(Index::new(7)),
        hash,
        "linked".to_string(),
    );
    assert_eq!(event.precursor_hash(), hash);
    let json = serde_json::to_string(&event).unwrap();
    let back: Event<String> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.precursor_hash(), hash);
}
#[test]
fn btreeset_index_roundtrip_includes_u64_max() {
    use pardosa_wire::{from_bytes, to_vec};
    use std::collections::BTreeSet;
    let mut s: BTreeSet<Index> = BTreeSet::new();
    s.insert(Index::new(1));
    s.insert(Index::new(42));
    s.insert(Index::new(1_000_000));
    s.insert(Index::new(u64::MAX));
    let bytes = to_vec(&s);
    let back: BTreeSet<Index> = from_bytes(&bytes).expect("decode");
    assert_eq!(back, s);
}
#[test]
fn fiber_id_encode_decode_roundtrip() {
    use pardosa_wire::{from_bytes, to_vec};
    for v in [0u64, 1, 42, u64::MAX - 1, u64::MAX] {
        let d = FiberId::new(v);
        let bytes = to_vec(&d);
        let back: FiberId = from_bytes(&bytes).expect("decode FiberId");
        assert_eq!(back, d, "round trip for {v}");
    }
}
/// Wire-byte equivalence: `EventId::encode` MUST produce identical bytes
/// to encoding the inner `u64`, so persisted GENOME records survive the
/// field-type promotion `Event<T>.event_id: u64 -> EventId`.
#[test]
fn event_id_encode_byte_equivalent_to_u64() {
    use pardosa_wire::to_vec;
    for v in [0u64, 1, 42, 1_000_000, u64::MAX - 1, u64::MAX] {
        let raw = to_vec(&v);
        let wrapped = to_vec(&EventId::new(v));
        assert_eq!(
            raw, wrapped,
            "EventId encode must be byte-equivalent to u64 for {v}"
        );
    }
}
#[test]
fn event_id_decode_roundtrip() {
    use pardosa_wire::{from_bytes, to_vec};
    for v in [0u64, 1, 42, u64::MAX - 1, u64::MAX] {
        let e = EventId::new(v);
        let bytes = to_vec(&e);
        let back: EventId = from_bytes(&bytes).expect("decode EventId");
        assert_eq!(back, e, "round trip for {v}");
    }
}
#[test]
fn event_id_checked_next() {
    assert_eq!(EventId::new(0).checked_next().unwrap(), EventId::new(1));
    assert!(EventId::new(u64::MAX).checked_next().is_err());
}
#[test]
fn event_string_encode_decode_roundtrip_genesis() {
    use pardosa_wire::{from_bytes, to_vec};
    let event = Event::new_unchecked(
        7,
        FiberId::new(99),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "genesis payload".to_string(),
    );
    let bytes = to_vec(&event);
    let back: Event<String> = from_bytes(&bytes).expect("decode Event<String>");
    assert_eq!(back.event_id(), event.event_id());
    assert_eq!(back.fiber_id(), event.fiber_id());
    assert_eq!(back.detached(), event.detached());
    assert_eq!(back.precursor(), event.precursor());
    assert_eq!(back.precursor_hash(), event.precursor_hash());
    assert_eq!(back.domain_event(), event.domain_event());
    assert_eq!(to_vec(&back), bytes);
}
#[test]
fn event_encode_decode_roundtrip_with_precursor_and_hash() {
    use pardosa_wire::{from_bytes, to_vec};
    let hash: [u8; 32] = [
        0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
        0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18,
        0x19, 0x1A,
    ];
    let event = Event::new_unchecked(
        42,
        FiberId::new(5),
        true,
        Precursor::Of(Index::new(40)),
        hash,
        "detached update".to_string(),
    );
    let bytes = to_vec(&event);
    let back: Event<String> = from_bytes(&bytes).expect("decode Event<String>");
    assert_eq!(to_vec(&back), bytes);
    assert_eq!(back.precursor_hash(), hash);
    assert!(back.detached());
    assert_eq!(back.precursor(), Precursor::Of(Index::new(40)));
}
#[test]
fn try_new_accepts_genesis_with_zero_precursor_hash() {
    let ev = Event::try_new(
        0u64,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "ok".to_string(),
    )
    .expect("genesis + zero hash is the valid shape");
    assert_eq!(ev.precursor(), Precursor::Genesis);
}
#[test]
fn try_new_rejects_genesis_with_non_zero_precursor_hash() {
    let mut bad_hash = [0u8; 32];
    bad_hash[0] = 1;
    let err = Event::try_new(
        0u64,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        bad_hash,
        "bad".to_string(),
    )
    .expect_err("genesis with non-zero precursor hash must reject");
    assert!(
        matches!(err, EnvelopeError::GenesisHasNonZeroPrecursorHash { hash } if hash
            == bad_hash),
        "got {err:?}"
    );
}
#[test]
fn try_new_accepts_precursor_of_with_any_hash() {
    let hash = [7u8; 32];
    let ev = Event::try_new(
        1u64,
        FiberId::new(1),
        false,
        Precursor::Of(Index::new(0)),
        hash,
        "ok".to_string(),
    )
    .expect("Of(_) accepts any hash at envelope layer");
    assert_eq!(ev.precursor_hash(), hash);
}
#[test]
fn validate_envelope_accepts_well_shaped_genesis() {
    let ev = Event::new_unchecked(
        0u64,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "ok".to_string(),
    );
    assert!(ev.validate_envelope().is_ok());
}
#[test]
fn validate_envelope_rejects_malformed_genesis() {
    let mut hash = [0u8; 32];
    hash[31] = 0xFF;
    let ev = Event::new_unchecked(
        0u64,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        hash,
        "bad".to_string(),
    );
    let err = ev.validate_envelope().expect_err("malformed Genesis");
    assert!(matches!(
        err,
        EnvelopeError::GenesisHasNonZeroPrecursorHash { .. }
    ));
}
#[test]
fn validate_trait_impl_for_event_delegates_to_validate_envelope() {
    use pardosa_wire::Validate;
    let good = Event::new_unchecked(
        0u64,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        [0u8; 32],
        "ok".to_string(),
    );
    assert!(Validate::validate(&good).is_ok());
    let mut bad_hash = [0u8; 32];
    bad_hash[0] = 1;
    let bad = Event::new_unchecked(
        0u64,
        FiberId::new(1),
        false,
        Precursor::Genesis,
        bad_hash,
        "bad".to_string(),
    );
    let err = Validate::validate(&bad).expect_err("non-zero genesis hash");
    assert!(matches!(
        err,
        EnvelopeError::GenesisHasNonZeroPrecursorHash { .. }
    ));
}
#[test]
fn validate_envelope_for_decoded_round_tripped_event_passes() {
    use pardosa_wire::{Validate, from_bytes, to_vec};
    let ev = Event::new_unchecked(
        7u64,
        FiberId::new(2),
        false,
        Precursor::Of(Index::new(6)),
        [9u8; 32],
        "payload".to_string(),
    );
    let bytes = to_vec(&ev);
    let back: Event<String> = from_bytes(&bytes).expect("decode");
    assert!(Validate::validate(&back).is_ok());
}
