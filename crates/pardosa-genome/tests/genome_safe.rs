//! Tests for `GenomeSafe` trait, derive macro, and schema source generation.

use pardosa_genome::GenomeOrd;
use pardosa_genome::GenomeSafe;
use pardosa_genome::genome_safe::{schema_hash_bytes, schema_hash_combine};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Struct derive — schema source
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct Player {
    name: String,
    hp: u32,
}

#[test]
fn struct_schema_source_contains_fields() {
    let src = <Player as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("struct Player"), "got: {src}");
    assert!(src.contains("name: String"), "got: {src}");
    assert!(src.contains("hp: u32"), "got: {src}");
}

#[test]
fn struct_schema_hash_is_nonzero() {
    let hash = <Player as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(hash, 0);
}

#[test]
fn struct_schema_hash_is_deterministic() {
    let h1 = <Player as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Player as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_eq!(h1, h2);
}

// ---------------------------------------------------------------------------
// Field order changes hash
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct PlayerReordered {
    hp: u32,
    name: String,
}

#[test]
fn field_order_changes_hash() {
    let h1 = <Player as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <PlayerReordered as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "reordering fields must change the schema hash");
}

// ---------------------------------------------------------------------------
// Field type change changes hash
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct PlayerU64Hp {
    name: String,
    hp: u64,
}

#[test]
fn field_type_change_changes_hash() {
    let h1 = <Player as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <PlayerU64Hp as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "changing field type must change the schema hash");
}

// ---------------------------------------------------------------------------
// Enum derive
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
#[repr(u8)]
enum Direction {
    North = 0,
    South = 1,
    East = 2,
    West = 3,
}

#[test]
fn enum_schema_source_contains_variants() {
    let src = <Direction as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("enum Direction"), "got: {src}");
    assert!(src.contains("North"), "got: {src}");
    assert!(src.contains("South"), "got: {src}");
    assert!(src.contains("East"), "got: {src}");
    assert!(src.contains("West"), "got: {src}");
}

// ---------------------------------------------------------------------------
// Enum with data variants
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
#[repr(u8)]
enum Shape {
    Circle { radius: f64 } = 0,
    Rectangle { width: f64, height: f64 } = 1,
    Point = 2,
}

#[test]
fn enum_with_data_schema_source() {
    let src = <Shape as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Circle"), "got: {src}");
    assert!(src.contains("radius: f64"), "got: {src}");
    assert!(src.contains("Rectangle"), "got: {src}");
    assert!(src.contains("width: f64"), "got: {src}");
    assert!(src.contains("height: f64"), "got: {src}");
    assert!(src.contains("Point"), "got: {src}");
}

// ---------------------------------------------------------------------------
// Newtype struct
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct Meters(f64);

#[test]
fn newtype_schema_source() {
    let src = <Meters as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Meters"), "got: {src}");
    assert!(src.contains("f64"), "got: {src}");
}

// ---------------------------------------------------------------------------
// Tuple struct
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct Point(f64, f64);

#[test]
fn tuple_struct_schema_source() {
    let src = <Point as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Point"), "got: {src}");
    assert!(src.contains("f64"), "got: {src}");
}

// ---------------------------------------------------------------------------
// Generic struct
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct Wrapper<T> {
    inner: T,
}

#[test]
fn generic_struct_schema_source() {
    let src = <Wrapper<u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Wrapper"), "got: {src}");
    assert!(src.contains("<T>"), "got: {src}");
    assert!(src.contains("inner: T"), "got: {src}");
}

#[test]
fn generic_struct_different_type_args_different_hash() {
    let h1 = <Wrapper<u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Wrapper<u64> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "different type args must produce different hashes");
}

// ---------------------------------------------------------------------------
// Nested struct
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct GameState {
    player: Player,
    level: u32,
    items: Vec<String>,
}

#[test]
fn nested_struct_schema_source() {
    let src = <GameState as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("struct GameState"), "got: {src}");
    assert!(src.contains("player: Player"), "got: {src}");
    assert!(src.contains("level: u32"), "got: {src}");
    assert!(src.contains("items: Vec<String>"), "got: {src}");
}

// ---------------------------------------------------------------------------
// Distinct newtypes with same inner type produce different hashes
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
struct Seconds(f64);

#[test]
fn distinct_newtypes_different_hashes() {
    let h1 = <Meters as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Seconds as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(
        h1, h2,
        "Meters(f64) and Seconds(f64) must have different hashes"
    );
}

// ---------------------------------------------------------------------------
// Primitive impls
// ---------------------------------------------------------------------------

#[test]
fn primitive_schema_sources() {
    assert_eq!(
        <u32 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "u32"
    );
    assert_eq!(
        <bool as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "bool"
    );
    assert_eq!(
        <String as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "String"
    );
    assert_eq!(
        <() as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "()"
    );
}

#[test]
fn primitive_hashes_are_distinct() {
    let hashes = [
        <u8 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <u16 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <u32 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <u64 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <i32 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <f64 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <bool as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
        <String as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH,
    ];
    for i in 0..hashes.len() {
        for j in (i + 1)..hashes.len() {
            assert_ne!(
                hashes[i], hashes[j],
                "hash collision at indices {i} and {j}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Hash combine is order-dependent
// ---------------------------------------------------------------------------

#[test]
fn hash_combine_order_dependent() {
    let a = schema_hash_bytes(b"alpha");
    let b = schema_hash_bytes(b"beta");
    assert_ne!(schema_hash_combine(a, b), schema_hash_combine(b, a),);
}

// ---------------------------------------------------------------------------
// Format constants
// ---------------------------------------------------------------------------

#[test]
fn format_constants() {
    use pardosa_genome::format::*;
    assert_eq!(MAGIC, *b"PGNO");
    assert_eq!(FORMAT_VERSION, 3);
    assert_eq!(FILE_HEADER_SIZE, 40);
    assert_eq!(FILE_FOOTER_SIZE, 32);
    assert_eq!(INDEX_ENTRY_SIZE, 24);
    assert_eq!(MIN_FILE_SIZE, 72);
    assert_eq!(NONE_SENTINEL, 0xFFFF_FFFF);

    // Header field offsets: structural-coherence chain. Each field's
    // (offset + width) must equal the next field's offset, terminating at
    // FILE_HEADER_SIZE. Catches one-sided offset/size drift that pure
    // literal-pinning would miss. Field widths: magic/4, version/2,
    // flags/2, schema_hash/HEADER_SCHEMA_HASH_LEN, dict_id/4, page_class/1,
    // schema_size/4, reserved/HEADER_RESERVED_LEN.
    assert_eq!(HEADER_MAGIC_OFFSET, 0);
    assert_eq!(HEADER_SCHEMA_HASH_LEN, 16);
    assert_eq!(HEADER_RESERVED_LEN, 7);
    assert_eq!(HEADER_MAGIC_OFFSET + 4, HEADER_VERSION_OFFSET);
    assert_eq!(HEADER_VERSION_OFFSET + 2, HEADER_FLAGS_OFFSET);
    assert_eq!(HEADER_FLAGS_OFFSET + 2, HEADER_SCHEMA_HASH_OFFSET);
    assert_eq!(
        HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN,
        HEADER_DICT_ID_OFFSET
    );
    assert_eq!(HEADER_DICT_ID_OFFSET + 4, HEADER_PAGE_CLASS_OFFSET);
    assert_eq!(HEADER_PAGE_CLASS_OFFSET + 1, HEADER_SCHEMA_SIZE_OFFSET);
    assert_eq!(HEADER_SCHEMA_SIZE_OFFSET + 4, HEADER_RESERVED_OFFSET);
    assert_eq!(
        HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN,
        FILE_HEADER_SIZE
    );

    // Bare message header sizes: structural-coherence chain. Field widths
    // are inherent to the wire spec (version/2, algo/1, msg_data_size/4,
    // compressed_size/4); schema_hash width is reused from the file header.
    assert_eq!(BARE_HEADER_SIZE, 23);
    assert_eq!(BARE_HEADER_COMPRESSED_SIZE, 27);
    assert_eq!(2 + HEADER_SCHEMA_HASH_LEN + 1 + 4, BARE_HEADER_SIZE);
    assert_eq!(
        2 + HEADER_SCHEMA_HASH_LEN + 1 + 4 + 4,
        BARE_HEADER_COMPRESSED_SIZE
    );

    // Footer field offsets must sum correctly to FILE_FOOTER_SIZE
    assert_eq!(FOOTER_RESERVED_OFFSET, 16);
    assert_eq!(FOOTER_RESERVED_LEN, 4);
    assert_eq!(FOOTER_MAGIC_OFFSET, 20);
    assert_eq!(FOOTER_CHECKSUM_OFFSET, 24);
    assert_eq!(FOOTER_CHECKSUM_OFFSET + 8, FILE_FOOTER_SIZE);
}

#[test]
fn pad_to_8_cases() {
    use pardosa_genome::format::pad_to_8;
    assert_eq!(pad_to_8(0), 0);
    assert_eq!(pad_to_8(1), 8);
    assert_eq!(pad_to_8(7), 8);
    assert_eq!(pad_to_8(8), 8);
    assert_eq!(pad_to_8(9), 16);
    assert_eq!(pad_to_8(32), 32);
}

#[test]
fn messages_offset_no_schema() {
    use pardosa_genome::format::{FILE_HEADER_SIZE, messages_offset};
    // No schema: messages start right after the header.
    assert_eq!(messages_offset(0), FILE_HEADER_SIZE);
}

#[test]
fn messages_offset_with_schema() {
    use pardosa_genome::format::{FILE_HEADER_SIZE, messages_offset, pad_to_8};
    // Messages always start at FILE_HEADER_SIZE + pad_to_8(schema_size).
    // 100 → padded to 104; 8 → padded to 8.
    assert_eq!(messages_offset(100), FILE_HEADER_SIZE + pad_to_8(100));
    assert_eq!(messages_offset(8), FILE_HEADER_SIZE + pad_to_8(8));
    // Anchored literals so the chain has fixed endpoints (not a tautology).
    assert_eq!(pad_to_8(100), 104);
    assert_eq!(pad_to_8(8), 8);
}

// ---------------------------------------------------------------------------
// Config defaults
// ---------------------------------------------------------------------------

#[test]
fn decode_options_defaults() {
    let opts = pardosa_genome::DecodeOptions::default();
    assert_eq!(opts.max_depth, 128);
    assert_eq!(opts.max_total_elements, 256);
    assert_eq!(opts.max_uncompressed_size, 268_435_456);
    assert!(opts.reject_trailing_bytes);
}

#[test]
fn page_class_elements() {
    use pardosa_genome::PageClass;
    assert_eq!(PageClass::Page0.max_elements(), 256);
    assert_eq!(PageClass::Page1.max_elements(), 4_096);
    assert_eq!(PageClass::Page2.max_elements(), 65_536);
    assert_eq!(PageClass::Page3.max_elements(), 1_048_576);
}

#[test]
fn page_class_from_byte() {
    use pardosa_genome::PageClass;
    assert_eq!(PageClass::from_byte(0), Some(PageClass::Page0));
    assert_eq!(PageClass::from_byte(3), Some(PageClass::Page3));
    assert_eq!(PageClass::from_byte(4), None);
    assert_eq!(PageClass::from_byte(255), None);
}

// ---------------------------------------------------------------------------
// Schema header offset
// ---------------------------------------------------------------------------

#[test]
fn schema_size_header_offset() {
    use pardosa_genome::format::{HEADER_PAGE_CLASS_OFFSET, HEADER_SCHEMA_SIZE_OFFSET};
    // schema_size sits immediately after page_class (a 1-byte field).
    // Structural form; the literal pin lives in `format_constants` via
    // HEADER_RESERVED_OFFSET-derived chain.
    assert_eq!(HEADER_PAGE_CLASS_OFFSET + 1, HEADER_SCHEMA_SIZE_OFFSET);
}

// ---------------------------------------------------------------------------
// Blanket impl hash transparency (Box/Arc/Cow delegate to inner)
// ---------------------------------------------------------------------------

#[test]
fn box_hash_transparent() {
    let inner = <u32 as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let boxed = <Box<u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_eq!(inner, boxed, "Box<T> hash must equal T hash");
}

#[test]
fn arc_hash_transparent() {
    let inner = <String as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let arced = <std::sync::Arc<String> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_eq!(inner, arced, "Arc<T> hash must equal T hash");
}

#[test]
fn cow_hash_transparent() {
    let inner = <str as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let cow = <std::borrow::Cow<'_, str> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_eq!(inner, cow, "Cow<T> hash must equal T hash");
}

// ---------------------------------------------------------------------------
// Blanket impl schema source values
// ---------------------------------------------------------------------------

#[test]
fn option_schema_source() {
    // Blanket impl uses placeholder — this is expected behavior, documented.
    assert_eq!(
        <Option<u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "Option<_>"
    );
}

#[test]
fn vec_schema_source() {
    assert_eq!(
        <Vec<String> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "Vec<_>"
    );
}

#[test]
fn btreemap_schema_source() {
    assert_eq!(
        <std::collections::BTreeMap<String, u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE,
        "BTreeMap<_, _>"
    );
}

// ---------------------------------------------------------------------------
// Container hash distinctness
// ---------------------------------------------------------------------------

#[test]
fn option_u32_vs_option_u64_different_hash() {
    let h1 = <Option<u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Option<u64> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2);
}

#[test]
fn vec_u32_vs_vec_string_different_hash() {
    let h1 = <Vec<u32> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Vec<String> as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2);
}

// ---------------------------------------------------------------------------
// Unit enum vs data enum different hash
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, GenomeSafe)]
#[repr(u8)]
enum Color {
    Red = 0,
    Green = 1,
    Blue = 2,
}

#[test]
fn unit_enum_vs_data_enum_different_hash() {
    let h1 = <Direction as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Color as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h3 = <Shape as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "different unit enums must have different hashes");
    assert_ne!(h1, h3, "unit enum vs data enum must differ");
    assert_ne!(h2, h3, "unit enum vs data enum must differ");
}

// ---------------------------------------------------------------------------
// Trait and derive resolve together
// ---------------------------------------------------------------------------

/// This test verifies the derive macro and trait coexist via the same import.
/// If the re-export is broken, this file won't compile at all.
#[derive(Serialize, Deserialize, GenomeSafe)]
struct TraitAndDeriveTest {
    value: u32,
}

#[test]
fn trait_and_derive_coexist() {
    // Trait usage: access associated constants
    let hash = <TraitAndDeriveTest as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(hash, 0);
    let src = <TraitAndDeriveTest as pardosa_genome::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("TraitAndDeriveTest"));
}

// ---------------------------------------------------------------------------
// Error display
// ---------------------------------------------------------------------------

#[test]
fn ser_error_display() {
    let err = pardosa_genome::SerError::MessageTooLarge;
    let s = format!("{err}");
    assert!(s.contains("4 GiB"), "got: {s}");
}

#[test]
fn de_error_display() {
    let err = pardosa_genome::DeError::SchemaMismatch {
        expected: 0x1234,
        actual: 0x5678,
    };
    let s = format!("{err}");
    assert!(s.contains("1234"), "got: {s}");
    assert!(s.contains("5678"), "got: {s}");
}

#[test]
fn file_error_display() {
    let err = pardosa_genome::FileError::InvalidSchemaSource;
    let s = format!("{err}");
    assert!(s.contains("UTF-8"), "got: {s}");
}

// ---------------------------------------------------------------------------
// String / str / Cow<str> schema hash identity (strict type-identity policy)
// ---------------------------------------------------------------------------

#[test]
fn string_str_cow_hash_identity() {
    let h_string = <String as GenomeSafe>::SCHEMA_HASH;
    let h_str = <str as GenomeSafe>::SCHEMA_HASH;
    let h_ref_str = <&str as GenomeSafe>::SCHEMA_HASH;
    let h_cow_str = <std::borrow::Cow<'_, str> as GenomeSafe>::SCHEMA_HASH;

    // &str and Cow<str> delegate to str — same hash
    assert_eq!(h_ref_str, h_str, "&str must equal str");
    assert_eq!(h_cow_str, h_str, "Cow<str> must equal str");
    // String is distinct (strict type identity)
    assert_ne!(h_string, h_str, "String must differ from str");
}

// ---------------------------------------------------------------------------
// PhantomData type erasure
// ---------------------------------------------------------------------------

#[test]
fn phantom_data_type_erasure() {
    let h1 = <core::marker::PhantomData<u32> as GenomeSafe>::SCHEMA_HASH;
    let h2 = <core::marker::PhantomData<String> as GenomeSafe>::SCHEMA_HASH;
    assert_eq!(h1, h2, "PhantomData ignores type parameter");
}

// ---------------------------------------------------------------------------
// Array length in hash
// ---------------------------------------------------------------------------

#[test]
fn array_length_changes_hash() {
    let h4 = <[u8; 4] as GenomeSafe>::SCHEMA_HASH;
    let h8 = <[u8; 8] as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h4, h8, "[u8; 4] and [u8; 8] must differ");
}

// ---------------------------------------------------------------------------
// Nested Option distinctness
// ---------------------------------------------------------------------------

#[test]
fn nested_option_distinct() {
    let h1 = <Option<u32> as GenomeSafe>::SCHEMA_HASH;
    let h2 = <Option<Option<u32>> as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "Option<u32> and Option<Option<u32>> must differ");
}

// ---------------------------------------------------------------------------
// Tuple arity 16 compiles (matches serde's limit)
// ---------------------------------------------------------------------------

#[test]
fn tuple_16_compiles() {
    let h = <(
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
    ) as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h, 0);
}

// ---------------------------------------------------------------------------
// Tuple hash stability (regression guard)
// ---------------------------------------------------------------------------

#[test]
fn tuple_2_hash_stability() {
    let h = <(u32, u64) as GenomeSafe>::SCHEMA_HASH;
    // Pin the hash value. If this fails after a change, the hash algorithm
    // was inadvertently altered.
    let expected = {
        let mut h = schema_hash_bytes(b"tuple2");
        h = schema_hash_combine(h, schema_hash_bytes(b"u32"));
        h = schema_hash_combine(h, schema_hash_bytes(b"u64"));
        h
    };
    assert_eq!(h, expected, "tuple hash algorithm must not change");
}

// ---------------------------------------------------------------------------
// GenomeOrd — compile-time assertions
// ---------------------------------------------------------------------------

fn assert_genome_ord<T: GenomeOrd>() {}

#[test]
fn genome_ord_primitive_impls() {
    assert_genome_ord::<bool>();
    assert_genome_ord::<u8>();
    assert_genome_ord::<u16>();
    assert_genome_ord::<u32>();
    assert_genome_ord::<u64>();
    assert_genome_ord::<u128>();
    assert_genome_ord::<i8>();
    assert_genome_ord::<i16>();
    assert_genome_ord::<i32>();
    assert_genome_ord::<i64>();
    assert_genome_ord::<i128>();
    assert_genome_ord::<char>();
    assert_genome_ord::<()>();
    assert_genome_ord::<String>();
}

#[test]
fn genome_ord_composite_impls() {
    assert_genome_ord::<Option<u32>>();
    assert_genome_ord::<Option<String>>();
    assert_genome_ord::<[u8; 4]>();
    assert_genome_ord::<[u8; 32]>();
    assert_genome_ord::<(u32,)>();
    assert_genome_ord::<(u32, String)>();
    assert_genome_ord::<(u8, u16, u32, u64)>();
    assert_genome_ord::<(
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
        u8,
    )>();
}

#[test]
fn genome_ord_btreemap_with_string_key() {
    // String implements GenomeOrd — BTreeMap<String, V> must work.
    let _ = <std::collections::BTreeMap<String, u32> as GenomeSafe>::SCHEMA_HASH;
}

#[test]
fn genome_ord_btreeset_with_u32() {
    let _ = <std::collections::BTreeSet<u32> as GenomeSafe>::SCHEMA_HASH;
}
