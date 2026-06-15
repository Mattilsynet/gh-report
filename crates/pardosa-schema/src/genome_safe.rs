/// Schema-aware marker trait extending [`pardosa_wire::EventSafe`].
///
/// `GenomeSafe` types carry a stable [`SCHEMA_HASH`](GenomeSafe::SCHEMA_HASH)
/// and a human-readable [`SCHEMA_SOURCE`](GenomeSafe::SCHEMA_SOURCE) so the
/// genome layer can detect schema drift across the wire and on disk.
///
/// # Sealing
/// `EventSafe` is strong-sealed via the private supertrait
/// `pardosa_wire::sealed::Sealed` per
/// [ADR-0014](../../../docs/adr/0014-sealed-trait-policy.md). `GenomeSafe`
/// adds the schema-hash gate on top; in-tree hand-impls are legal (and
/// necessary for blanket impls like `Option<T>`, primitives, etc.), but
/// downstream consumers route through `#[derive(GenomeSafe)]`, which emits
/// matching `Sealed` / `EventSafe` / `GenomeSafe` impls together.
pub trait GenomeSafe: pardosa_wire::EventSafe {
    const SCHEMA_HASH: u128;
    const SCHEMA_SOURCE: &'static str;
}
pub trait GenomeOrd: GenomeSafe {}
#[must_use]
pub const fn schema_hash_bytes(bytes: &[u8]) -> u128 {
    xxhash_rust::const_xxh3::xxh3_128_with_seed(bytes, 0)
}
#[must_use]
pub const fn schema_hash_combine(outer: u128, inner: u128) -> u128 {
    let o = outer.to_le_bytes();
    let i = inner.to_le_bytes();
    let bytes: [u8; 32] = [
        o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7], o[8], o[9], o[10], o[11], o[12], o[13],
        o[14], o[15], i[0], i[1], i[2], i[3], i[4], i[5], i[6], i[7], i[8], i[9], i[10], i[11],
        i[12], i[13], i[14], i[15],
    ];
    xxhash_rust::const_xxh3::xxh3_128_with_seed(&bytes, 0)
}
macro_rules! impl_genome_safe_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(impl GenomeSafe for $ty { const SCHEMA_HASH : u128 =
        schema_hash_bytes(stringify!($ty) .as_bytes()); const SCHEMA_SOURCE : &'static
        str = stringify!($ty); })+
    };
}
impl_genome_safe_primitive!(bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128,);
impl GenomeSafe for char {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"char");
    const SCHEMA_SOURCE: &'static str = "char";
}
impl GenomeSafe for pardosa_wire::Timestamp {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"Timestamp");
    const SCHEMA_SOURCE: &'static str = "Timestamp";
}
#[cfg(feature = "uuid")]
impl GenomeSafe for uuid::Uuid {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"uuid::Uuid");
    const SCHEMA_SOURCE: &'static str = "uuid::Uuid";
}
#[cfg(feature = "uuid")]
impl GenomeOrd for uuid::Uuid {}
impl<T: GenomeSafe> GenomeSafe for Option<T> {
    const SCHEMA_HASH: u128 = schema_hash_combine(schema_hash_bytes(b"Option"), T::SCHEMA_HASH);
    const SCHEMA_SOURCE: &'static str = "Option<_>";
}
impl<T: GenomeSafe, const N: usize> GenomeSafe for [T; N] {
    const SCHEMA_HASH: u128 = schema_hash_combine(
        schema_hash_bytes(b"array"),
        schema_hash_combine(T::SCHEMA_HASH, N as u128),
    );
    const SCHEMA_SOURCE: &'static str = "[_; N]";
}
macro_rules! impl_genome_safe_tuple {
    ($label:expr, $($T:ident),+) => {
        impl <$($T : GenomeSafe),+> GenomeSafe for ($($T,)+) { const SCHEMA_HASH : u128 =
        { let mut h = schema_hash_bytes($label .as_bytes()); $(h = schema_hash_combine(h,
        $T ::SCHEMA_HASH);)+ h }; const SCHEMA_SOURCE : &'static str = concat!("(",
        $(stringify!($T), ", ",)+ ")"); }
    };
}
impl_genome_safe_tuple!("tuple1", T0);
impl_genome_safe_tuple!("tuple2", T0, T1);
impl_genome_safe_tuple!("tuple3", T0, T1, T2);
impl_genome_safe_tuple!("tuple4", T0, T1, T2, T3);
impl_genome_safe_tuple!("tuple5", T0, T1, T2, T3, T4);
impl_genome_safe_tuple!("tuple6", T0, T1, T2, T3, T4, T5);
impl_genome_safe_tuple!("tuple7", T0, T1, T2, T3, T4, T5, T6);
impl_genome_safe_tuple!("tuple8", T0, T1, T2, T3, T4, T5, T6, T7);
impl_genome_safe_tuple!("tuple9", T0, T1, T2, T3, T4, T5, T6, T7, T8);
impl_genome_safe_tuple!("tuple10", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_genome_safe_tuple!("tuple11", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
impl_genome_safe_tuple!("tuple12", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
impl_genome_safe_tuple!(
    "tuple13", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12
);
impl_genome_safe_tuple!(
    "tuple14", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13
);
impl_genome_safe_tuple!(
    "tuple15", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14
);
impl_genome_safe_tuple!(
    "tuple16", T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
macro_rules! impl_genome_ord_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(impl GenomeOrd for $ty {})+
    };
}
impl_genome_ord_primitive!(bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, char,);
impl GenomeOrd for pardosa_wire::Timestamp {}
impl<T: GenomeOrd> GenomeOrd for Option<T> {}
impl<T: GenomeOrd, const N: usize> GenomeOrd for [T; N] {}
macro_rules! impl_genome_ord_tuple {
    ($($T:ident),+) => {
        impl <$($T : GenomeOrd),+> GenomeOrd for ($($T,)+) {}
    };
}
impl_genome_ord_tuple!(T0);
impl_genome_ord_tuple!(T0, T1);
impl_genome_ord_tuple!(T0, T1, T2);
impl_genome_ord_tuple!(T0, T1, T2, T3);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
impl_genome_ord_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
impl_genome_ord_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14
);
impl_genome_ord_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
