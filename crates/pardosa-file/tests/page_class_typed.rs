//! Stage 1 type-trait hardening: typed `PageClass` sibling
//! APIs on `Writer` / `Reader`.
//!
//! Mission `rescue-pardosa-kfsk` under epic
//! `rescue-pardosa-on51`. Pins additive, non-breaking surface:
//!
//! 1. `Writer::with_page_class_typed(PageClass)` writes the
//!    `#[repr(u8)]` discriminant byte-identically to
//!    `with_page_class(disc as u8)`.
//! 2. `Reader::page_class_typed()` returns `Some(PageClass)`
//!    for known discriminants (0..=3), `None` for others.
//! 3. Raw `u8` ingress / accessor remain byte-compatible.
use pardosa_file::format::HEADER_PAGE_CLASS_OFFSET;
use pardosa_file::{PageClass, Reader, Writer};
use std::io::Cursor;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
/// SC1+SC2: typed writer setter produces the same on-disk byte as the
/// raw `u8` setter for every known `PageClass` discriminant, and the
/// typed reader accessor surfaces the matching enum variant.
#[test]
fn typed_writer_round_trips_through_typed_reader_for_known_classes() {
    for pc in [
        PageClass::Page0,
        PageClass::Page1,
        PageClass::Page2,
        PageClass::Page3,
    ] {
        let mut typed_buf = Vec::new();
        Writer::new(&mut typed_buf, KNOWN_HASH)
            .with_page_class_typed(pc)
            .finish()
            .expect("finish typed");
        let mut raw_buf = Vec::new();
        Writer::new(&mut raw_buf, KNOWN_HASH)
            .with_page_class(pc as u8)
            .finish()
            .expect("finish raw");
        assert_eq!(
            typed_buf, raw_buf,
            "typed setter must produce byte-identical output to raw setter for {pc:?}",
        );
        assert_eq!(typed_buf[HEADER_PAGE_CLASS_OFFSET], pc as u8);
        let r = Reader::open(Cursor::new(&typed_buf)).expect("Reader::open");
        assert_eq!(r.page_class(), pc as u8, "raw accessor unchanged");
        assert_eq!(
            r.page_class_typed(),
            Some(pc),
            "typed accessor surfaces known discriminant for {pc:?}",
        );
    }
}
/// SC2: a `.pgno` whose `page_class` byte is outside the known 0..=3
/// range is openable (ADR-0006 §5–§6: opaque byte) and the typed
/// accessor returns `None` while the raw accessor surfaces the byte
/// unchanged. Mirrors `h3_format_semantics::page_class_all_boundary_values_round_trip`
/// but on the typed side.
#[test]
fn typed_reader_returns_none_for_unknown_page_class_bytes() {
    for class in [4u8, 42, 127, 128, 254, 255] {
        let mut buf = Vec::new();
        Writer::new(&mut buf, KNOWN_HASH)
            .with_page_class(class)
            .finish()
            .expect("finish");
        let r = Reader::open(Cursor::new(&buf)).expect("Reader::open");
        assert_eq!(r.page_class(), class, "raw byte preserved");
        assert_eq!(
            r.page_class_typed(),
            None,
            "typed accessor must return None for unknown page_class byte {class}",
        );
    }
}
/// Pin the `#[repr(u8)]` discriminants. If anyone ever bumps a
/// discriminant they will break this test and the boundary-byte
/// `h3_format_semantics` suite, forcing an ADR-0006 amendment.
#[test]
fn page_class_discriminants_are_byte_stable() {
    assert_eq!(PageClass::Page0 as u8, 0);
    assert_eq!(PageClass::Page1 as u8, 1);
    assert_eq!(PageClass::Page2 as u8, 2);
    assert_eq!(PageClass::Page3 as u8, 3);
}
/// Cross-check `PageClass::from_byte` against `page_class_typed`: the
/// reader's typed accessor is defined to agree with `from_byte` on
/// the stored byte, so the two must never diverge.
#[test]
fn typed_reader_agrees_with_from_byte() {
    for class in 0u8..=8 {
        let mut buf = Vec::new();
        Writer::new(&mut buf, KNOWN_HASH)
            .with_page_class(class)
            .finish()
            .expect("finish");
        let r = Reader::open(Cursor::new(&buf)).expect("Reader::open");
        assert_eq!(
            r.page_class_typed(),
            PageClass::from_byte(class),
            "page_class_typed must equal PageClass::from_byte(stored_byte) for {class}",
        );
    }
}
/// `max_elements` mapping is pinned so future callers (e.g. page-routing
/// code) can rely on it. Mirrors `format_constants::page_class_elements`
/// but lives next to the typed-API tests so a discriminant remap is
/// caught here too.
#[test]
fn page_class_max_elements_mapping_is_pinned() {
    assert_eq!(PageClass::Page0.max_elements(), 256);
    assert_eq!(PageClass::Page1.max_elements(), 4_096);
    assert_eq!(PageClass::Page2.max_elements(), 65_536);
    assert_eq!(PageClass::Page3.max_elements(), 1_048_576);
}
