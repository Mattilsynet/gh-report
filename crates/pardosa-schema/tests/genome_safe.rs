use pardosa_schema::genome_safe::{schema_hash_bytes, schema_hash_combine};
use pardosa_schema::{EventString, EventVec, GenomeOrd, GenomeSafe, OrderedF64};
use serde::{Deserialize, Serialize};
#[derive(GenomeSafe)]
struct Player {
    name: EventString<256>,
    hp: u32,
}
#[test]
fn struct_schema_source_contains_fields() {
    let src = <Player as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("struct Player"), "got: {src}");
    assert!(src.contains("name: EventString<256>"), "got: {src}");
    assert!(src.contains("hp: u32"), "got: {src}");
}
#[test]
fn struct_schema_hash_is_nonzero() {
    let hash = <Player as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(hash, 0);
}
#[test]
fn struct_schema_hash_is_deterministic() {
    let h1 = <Player as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Player as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_eq!(h1, h2);
}
#[derive(GenomeSafe)]
struct PlayerReordered {
    hp: u32,
    name: EventString<256>,
}
#[test]
fn field_order_changes_hash() {
    let h1 = <Player as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <PlayerReordered as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "reordering fields must change the schema hash");
}
#[derive(GenomeSafe)]
struct PlayerU64Hp {
    name: EventString<256>,
    hp: u64,
}
#[test]
fn field_type_change_changes_hash() {
    let h1 = <Player as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <PlayerU64Hp as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "changing field type must change the schema hash");
}
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
    let src = <Direction as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("enum Direction"), "got: {src}");
    assert!(src.contains("North"), "got: {src}");
    assert!(src.contains("South"), "got: {src}");
    assert!(src.contains("East"), "got: {src}");
    assert!(src.contains("West"), "got: {src}");
}
#[derive(Serialize, Deserialize, GenomeSafe)]
#[repr(u8)]
enum Shape {
    Circle {
        radius: OrderedF64,
    } = 0,
    Rectangle {
        width: OrderedF64,
        height: OrderedF64,
    } = 1,
    Point = 2,
}
#[test]
fn enum_with_data_schema_source() {
    let src = <Shape as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Circle"), "got: {src}");
    assert!(src.contains("radius: OrderedF64"), "got: {src}");
    assert!(src.contains("Rectangle"), "got: {src}");
    assert!(src.contains("width: OrderedF64"), "got: {src}");
    assert!(src.contains("height: OrderedF64"), "got: {src}");
    assert!(src.contains("Point"), "got: {src}");
}
#[derive(Serialize, Deserialize, GenomeSafe)]
struct Meters(OrderedF64);
#[test]
fn newtype_schema_source() {
    let src = <Meters as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Meters"), "got: {src}");
    assert!(src.contains("OrderedF64"), "got: {src}");
}
#[derive(Serialize, Deserialize, GenomeSafe)]
struct Point(OrderedF64, OrderedF64);
#[test]
fn tuple_struct_schema_source() {
    let src = <Point as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Point"), "got: {src}");
    assert!(src.contains("OrderedF64"), "got: {src}");
}
#[derive(Serialize, Deserialize, GenomeSafe)]
struct Wrapper<T> {
    inner: T,
}
#[test]
fn generic_struct_schema_source() {
    let src = <Wrapper<u32> as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("Wrapper"), "got: {src}");
    assert!(src.contains("<T>"), "got: {src}");
    assert!(src.contains("inner: T"), "got: {src}");
}
#[test]
fn generic_struct_different_type_args_different_hash() {
    let h1 = <Wrapper<u32> as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Wrapper<u64> as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "different type args must produce different hashes");
}
#[derive(GenomeSafe)]
struct GameState {
    player: Player,
    level: u32,
    items: EventVec<EventString<256>, 16>,
}
#[test]
fn nested_struct_schema_source() {
    let src = <GameState as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("struct GameState"), "got: {src}");
    assert!(src.contains("player: Player"), "got: {src}");
    assert!(src.contains("level: u32"), "got: {src}");
    assert!(
        src.contains("items: EventVec<EventString<256>, 16>"),
        "got: {src}"
    );
}
#[derive(Serialize, Deserialize, GenomeSafe)]
struct Seconds(OrderedF64);
#[test]
fn distinct_newtypes_different_hashes() {
    let h1 = <Meters as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    let h2 = <Seconds as pardosa_schema::genome_safe::GenomeSafe>::SCHEMA_HASH;
    assert_ne!(
        h1, h2,
        "Meters(OrderedF64) and Seconds(OrderedF64) must have different hashes",
    );
}
#[test]
fn primitive_schema_sources() {
    assert_eq!(<u32 as GenomeSafe>::SCHEMA_SOURCE, "u32");
    assert_eq!(<bool as GenomeSafe>::SCHEMA_SOURCE, "bool");
    assert_eq!(<() as GenomeSafe>::SCHEMA_SOURCE, "()");
}
#[test]
fn primitive_hashes_are_distinct() {
    let hashes = [
        <u8 as GenomeSafe>::SCHEMA_HASH,
        <u16 as GenomeSafe>::SCHEMA_HASH,
        <u32 as GenomeSafe>::SCHEMA_HASH,
        <u64 as GenomeSafe>::SCHEMA_HASH,
        <i32 as GenomeSafe>::SCHEMA_HASH,
        <bool as GenomeSafe>::SCHEMA_HASH,
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
#[test]
fn hash_combine_order_dependent() {
    let a = schema_hash_bytes(b"alpha");
    let b = schema_hash_bytes(b"beta");
    assert_ne!(schema_hash_combine(a, b), schema_hash_combine(b, a),);
}
#[test]
fn box_hash_transparent() {
    let inner = <u32 as GenomeSafe>::SCHEMA_HASH;
    let boxed = <Box<u32> as GenomeSafe>::SCHEMA_HASH;
    assert_eq!(inner, boxed, "Box<T> hash must equal T hash");
}
#[test]
fn option_schema_source() {
    assert_eq!(<Option<u32> as GenomeSafe>::SCHEMA_SOURCE, "Option<_>");
}
#[test]
fn option_u32_vs_option_u64_different_hash() {
    let h1 = <Option<u32> as GenomeSafe>::SCHEMA_HASH;
    let h2 = <Option<u64> as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2);
}
#[derive(Serialize, Deserialize, GenomeSafe)]
#[repr(u8)]
enum Color {
    Red = 0,
    Green = 1,
    Blue = 2,
}
#[test]
fn unit_enum_vs_data_enum_different_hash() {
    let h1 = <Direction as GenomeSafe>::SCHEMA_HASH;
    let h2 = <Color as GenomeSafe>::SCHEMA_HASH;
    let h3 = <Shape as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "different unit enums must have different hashes");
    assert_ne!(h1, h3, "unit enum vs data enum must differ");
    assert_ne!(h2, h3, "unit enum vs data enum must differ");
}
#[derive(Serialize, Deserialize, GenomeSafe)]
struct TraitAndDeriveTest {
    value: u32,
}
#[test]
fn trait_and_derive_coexist() {
    let hash = <TraitAndDeriveTest as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(hash, 0);
    let src = <TraitAndDeriveTest as GenomeSafe>::SCHEMA_SOURCE;
    assert!(src.contains("TraitAndDeriveTest"));
}
#[test]
fn phantom_data_type_erasure() {
    let h1 = <core::marker::PhantomData<u32> as GenomeSafe>::SCHEMA_HASH;
    let h2 = <core::marker::PhantomData<u64> as GenomeSafe>::SCHEMA_HASH;
    assert_eq!(h1, h2, "PhantomData ignores type parameter");
}
#[test]
fn array_length_changes_hash() {
    let h4 = <[u8; 4] as GenomeSafe>::SCHEMA_HASH;
    let h8 = <[u8; 8] as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h4, h8, "[u8; 4] and [u8; 8] must differ");
}
#[test]
fn nested_option_distinct() {
    let h1 = <Option<u32> as GenomeSafe>::SCHEMA_HASH;
    let h2 = <Option<Option<u32>> as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h1, h2, "Option<u32> and Option<Option<u32>> must differ");
}
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
#[test]
fn tuple_2_hash_stability() {
    let h = <(u32, u64) as GenomeSafe>::SCHEMA_HASH;
    let expected = {
        let mut h = schema_hash_bytes(b"tuple2");
        h = schema_hash_combine(h, schema_hash_bytes(b"u32"));
        h = schema_hash_combine(h, schema_hash_bytes(b"u64"));
        h
    };
    assert_eq!(h, expected, "tuple hash algorithm must not change");
}
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
}
#[test]
fn genome_ord_composite_impls() {
    assert_genome_ord::<Option<u32>>();
    assert_genome_ord::<[u8; 4]>();
    assert_genome_ord::<[u8; 32]>();
    assert_genome_ord::<(u32,)>();
    assert_genome_ord::<(u32, u64)>();
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
mod disc_a {
    use super::{Deserialize, GenomeSafe, Serialize};
    #[derive(Serialize, Deserialize, GenomeSafe)]
    #[repr(u8)]
    pub enum Discriminants {
        First = 0,
        Second = 1,
        Third = 2,
    }
}
mod disc_b {
    use super::{Deserialize, GenomeSafe, Serialize};
    #[derive(Serialize, Deserialize, GenomeSafe)]
    #[repr(u8)]
    pub enum Discriminants {
        First = 10,
        Second = 20,
        Third = 30,
    }
}
#[test]
fn enum_discriminant_values_change_schema_hash() {
    let a = <disc_a::Discriminants as GenomeSafe>::SCHEMA_HASH;
    let b = <disc_b::Discriminants as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(
        a, b,
        "GEN-0035:R9 — enum discriminant values must participate in SCHEMA_HASH; \
         got identical hashes for enums differing only in repr(u8) discriminant values",
    );
}
#[test]
fn timestamp_genome_safe_source_and_hash() {
    let src = <pardosa_wire::Timestamp as GenomeSafe>::SCHEMA_SOURCE;
    assert_eq!(src, "Timestamp");
    let h = <pardosa_wire::Timestamp as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(h, 0u128);
}
#[test]
fn timestamp_distinct_from_u64() {
    let ts = <pardosa_wire::Timestamp as GenomeSafe>::SCHEMA_HASH;
    let u = <u64 as GenomeSafe>::SCHEMA_HASH;
    assert_ne!(
        ts, u,
        "Timestamp must not collide with u64 — wire format identical, but \
         schema identity must distinguish (nonzero invariant)",
    );
}
#[test]
fn timestamp_is_genome_ord() {
    fn requires_ord<T: pardosa_schema::genome_safe::GenomeOrd>() {}
    requires_ord::<pardosa_wire::Timestamp>();
}
