//! Regression tests for F1: `Index` deserialize must reject the `u64::MAX`
//! sentinel except where explicitly permitted (e.g. `Event.precursor`).
//!
//! Why: `Index::new` asserts `v != u64::MAX`, but a derived `Deserialize`
//! bypasses the constructor. Handcrafted serialized forms could otherwise
//! inject `Index::NONE` into positions that semantically forbid it
//! (notably `Fiber.current`, which `Dragline::verify_precursor_chains`
//! treats as "first event in fiber" — a forged NONE silently looks like
//! genesis).

use pardosa::{DomainId, Event, Fiber, Index};

const NONE_LITERAL: &str = "18446744073709551615";

#[test]
fn bare_index_rejects_sentinel() {
    let result: Result<Index, _> = serde_json::from_str(NONE_LITERAL);
    assert!(
        result.is_err(),
        "bare Index deserialize must reject u64::MAX sentinel; got Ok"
    );
}

#[test]
fn bare_index_accepts_valid() {
    let result: Result<Index, _> = serde_json::from_str("42");
    assert!(
        result.is_ok(),
        "bare Index deserialize must accept valid u64"
    );
    assert_eq!(result.unwrap(), Index::new(42));
}

#[test]
fn bare_index_rejects_max_minus_one_boundary_still_valid() {
    // u64::MAX - 1 is the last valid Index; sanity check the cutoff.
    let json = (u64::MAX - 1).to_string();
    let result: Result<Index, _> = serde_json::from_str(&json);
    assert!(result.is_ok(), "u64::MAX - 1 must remain a valid Index");
}

#[test]
fn event_accepts_precursor_none() {
    // Genesis event: precursor = NONE is semantically valid inside Event.
    let event = Event::new(
        1,
        1_700_000_000_000,
        DomainId::new(1),
        false,
        Index::NONE,
        [0u8; 32],
        "created".to_string(),
    );
    let json = serde_json::to_string(&event).unwrap();
    let back: Event<String> =
        serde_json::from_str(&json).expect("Event must accept precursor=NONE");
    assert!(back.precursor().is_none());
}

#[test]
fn event_with_real_precursor_roundtrips() {
    let event = Event::new(
        2,
        1_700_000_000_001,
        DomainId::new(1),
        false,
        Index::new(0),
        [0u8; 32],
        "updated".to_string(),
    );
    let json = serde_json::to_string(&event).unwrap();
    let back: Event<String> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.precursor(), Index::new(0));
}

#[test]
fn fiber_forged_current_none_rejected() {
    // A handcrafted Fiber with current = u64::MAX must be rejected — Fiber
    // invariants forbid current = NONE, and FiberRaw routes through
    // Fiber::new which enforces them.
    let json = r#"{"anchor":0,"len":1,"current":18446744073709551615}"#;
    let result: Result<Fiber, _> = serde_json::from_str(json);
    assert!(
        result.is_err(),
        "Fiber deserialize must reject forged current=NONE"
    );
}

#[test]
fn fiber_forged_anchor_none_rejected() {
    let json = r#"{"anchor":18446744073709551615,"len":1,"current":0}"#;
    let result: Result<Fiber, _> = serde_json::from_str(json);
    assert!(
        result.is_err(),
        "Fiber deserialize must reject forged anchor=NONE"
    );
}
