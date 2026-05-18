/// Marker trait enforcing deterministic serialization at compile time.
///
/// Types implementing `GenomeSafe` are guaranteed to produce deterministic,
/// fixed-layout binary output when serialized with pardosa-genome. The trait
/// rejects non-deterministic containers (`HashMap`, `HashSet`) and serde
/// attributes that break fixed-layout assumptions (`#[serde(flatten)]`,
/// `#[serde(tag)]`, `#[serde(untagged)]`, `#[serde(skip_serializing_if)]`).
///
/// For types used as `BTreeMap` keys or `BTreeSet` elements, the additional
/// [`GenomeOrd`] marker is required. See ADR-033.
///
/// # Associated Constants
///
/// - `SCHEMA_HASH`: 16-byte xxh3-128 fingerprint of the type's serde structure.
///   Computed at compile time. Embedded in every serialized message and verified
///   on deserialization. Mismatch produces `DeError::SchemaMismatch`. The width
///   widened from u64 (xxh64) to u128 (xxh3-128) per GEN-0035 to push the
///   collision floor past the birthday bound at 2^64.
///
/// - `SCHEMA_SOURCE`: Human-readable Rust source text describing the type's
///   structure. Embedded in genome file headers for inspection. Not used for
///   compatibility checks — the hash is authoritative.
///
/// # Derive
///
/// Use `#[derive(GenomeSafe)]` to implement this trait. The derive macro
/// performs syntactic rejection of unsupported serde attributes and computes
/// the schema hash from field names, types, and ordering.
///
/// ```ignore
/// use pardosa_genome::GenomeSafe;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize, GenomeSafe)]
/// struct Player {
///     name: String,
///     hp: u32,
/// }
/// ```
pub trait GenomeSafe: pardosa_traits::EventSafe {
    /// Compile-time schema fingerprint (xxh3-128).
    ///
    /// Computed from: root type name, field names, field types, enum variant
    /// names and shapes, type ordering. Deterministic across compilations.
    /// Width is `u128` (was `u64` under xxh64) per GEN-0035; the algorithm
    /// and width are jointly frozen, the inputs continue per GEN-0003.
    const SCHEMA_HASH: u128;

    /// Human-readable type definition for file header embedding.
    ///
    /// Contains the cleaned Rust struct/enum definition — field names, types,
    /// variant shapes. No imports, no impls, no doc comments. Plain UTF-8 text.
    ///
    /// For primitive types, this is the type name (e.g., `"u32"`).
    /// For derived types, this is the full structural definition.
    const SCHEMA_SOURCE: &'static str;
}

/// Marker trait for types with a deterministic, total [`Ord`] implementation
/// suitable for use as [`BTreeMap`] keys or [`BTreeSet`] elements in
/// genome-encoded data.
///
/// Only owned value types implement this trait. Runtime wrappers ([`Box`],
/// [`Arc`](std::sync::Arc), [`Cow`](std::borrow::Cow)) and borrowed types
/// (`&str`, `&[u8]`) are excluded — use the owned equivalent (e.g.,
/// [`String`] instead of `Cow<'_, str>`).
///
/// # Semantic Contract
///
/// Implementing `GenomeOrd` asserts that the type's [`Ord`] implementation is:
/// - **Total** — defined for all value pairs
/// - **Deterministic** — same inputs produce the same ordering across runs
/// - **Platform-independent** — no locale, environment, or runtime state dependency
///
/// `GenomeOrd` is a safe trait — the compiler cannot verify ordering properties.
/// [`verify_roundtrip`] provides defense-in-depth against incorrect implementations.
///
/// # Derive Macro Integration
///
/// The `#[derive(GenomeSafe)]` macro automatically detects generic type
/// parameters used in `BTreeMap` key or `BTreeSet` element position and adds
/// `GenomeOrd` bounds for them. For concrete types (e.g., `BTreeMap<String, V>`),
/// no user action is needed.
///
/// # Custom Key Types
///
/// To use a custom type as a map key, implement both traits:
///
/// ```ignore
/// use pardosa_genome::{GenomeSafe, GenomeOrd};
///
/// #[derive(PartialEq, Eq, PartialOrd, Ord, GenomeSafe)]
/// struct MyKey { id: u64 }
///
/// impl GenomeOrd for MyKey {}
/// ```
pub trait GenomeOrd: GenomeSafe {}

// ---------------------------------------------------------------------------
// Schema hash helpers
// ---------------------------------------------------------------------------

/// Compute xxh3-128 of a byte slice at compile time.
///
/// Wrapper around `xxhash_rust::const_xxh3::xxh3_128_with_seed` with a
/// fixed seed of 0.
///
/// # Stability Contract
///
/// The seed value (0) is **frozen** and must never change. Changing it
/// invalidates every schema hash ever computed, making all existing genome
/// files and bare messages unreadable. The xxh3-128 algorithm itself
/// (`xxhash_rust::const_xxh3`) is also part of this contract — per
/// GEN-0035 the algorithm widened from xxh64 to xxh3-128 to push the
/// collision floor past 2^64.
#[must_use]
pub const fn schema_hash_bytes(bytes: &[u8]) -> u128 {
    xxhash_rust::const_xxh3::xxh3_128_with_seed(bytes, 0)
}

/// Combine two schema hashes into one. Used for composite types
/// (structs, enums, containers) to fold inner type hashes into the
/// outer type's hash.
///
/// Order-dependent: `combine(a, b) != combine(b, a)`.
///
/// # Stability Contract
///
/// The combine algorithm is **frozen**: LE-concatenate the two u128
/// values into a 32-byte array, then hash with
/// `xxh3_128_with_seed(bytes, seed=0)`. Changing the byte ordering,
/// concatenation method, seed, or algorithm invalidates all composite
/// schema hashes. The byte ordering rule carries forward from GEN-0003;
/// width widened from u64→u128 per GEN-0035.
#[must_use]
pub const fn schema_hash_combine(outer: u128, inner: u128) -> u128 {
    // Mix the inner hash into the outer by hashing the LE concatenation
    // of both as bytes. 32 bytes total (2 × 16-byte u128).
    let o = outer.to_le_bytes();
    let i = inner.to_le_bytes();
    let bytes: [u8; 32] = [
        o[0], o[1], o[2], o[3], o[4], o[5], o[6], o[7], o[8], o[9], o[10], o[11], o[12], o[13],
        o[14], o[15], i[0], i[1], i[2], i[3], i[4], i[5], i[6], i[7], i[8], i[9], i[10], i[11],
        i[12], i[13], i[14], i[15],
    ];
    xxhash_rust::const_xxh3::xxh3_128_with_seed(&bytes, 0)
}

// ---------------------------------------------------------------------------
// Blanket impls — primitives
// ---------------------------------------------------------------------------

macro_rules! impl_genome_safe_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl GenomeSafe for $ty {
                const SCHEMA_HASH: u128 = schema_hash_bytes(stringify!($ty).as_bytes());
                const SCHEMA_SOURCE: &'static str = stringify!($ty);
            }
        )+
    };
}

impl_genome_safe_primitive!(
    bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64,
);

// Raw `char` retains `GenomeSafe` for fields carrying a raw Unicode codepoint
// without the explicit-scalar contract. When the field's intent is "exactly
// one Unicode scalar with surrogate rejection at the boundary", use
// `CharScalar` instead (GEN-0045:R2). The two types are wire byte-identical
// (4-byte LE u32) but have distinct schema hashes.
impl GenomeSafe for char {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"char");
    const SCHEMA_SOURCE: &'static str = "char";
}

impl GenomeSafe for () {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"()");
    const SCHEMA_SOURCE: &'static str = "()";
}

// ---------------------------------------------------------------------------
// Blanket impls — string and byte types
// ---------------------------------------------------------------------------
//
// String type identity policy (frozen — changing this breaks schema hashes):
//
// Equivalence class 1: str == &str == Cow<'_, str> == Box<str>
//   All delegate to hash("str"). Changing between these is schema-compatible.
//
// Equivalence class 2: String (standalone)
//   Uses hash("String"). Changing from String to &str (or vice versa)
//   is a schema-breaking change, even though serde serializes all three
//   identically. This preserves strict Rust-type-identity to prevent
//   subtle zero-copy vs. owned semantics mismatches.

impl GenomeSafe for str {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"str");
    const SCHEMA_SOURCE: &'static str = "str";
}

impl GenomeSafe for String {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"String");
    const SCHEMA_SOURCE: &'static str = "String";
}

// ---------------------------------------------------------------------------
// Blanket impls — containers
// ---------------------------------------------------------------------------

impl<T: GenomeSafe> GenomeSafe for Option<T> {
    const SCHEMA_HASH: u128 = schema_hash_combine(schema_hash_bytes(b"Option"), T::SCHEMA_HASH);
    const SCHEMA_SOURCE: &'static str = "Option<_>";
}

impl<T: GenomeSafe> GenomeSafe for Vec<T> {
    const SCHEMA_HASH: u128 = schema_hash_combine(schema_hash_bytes(b"Vec"), T::SCHEMA_HASH);
    const SCHEMA_SOURCE: &'static str = "Vec<_>";
}

impl<T: GenomeSafe> GenomeSafe for Box<T> {
    const SCHEMA_HASH: u128 = T::SCHEMA_HASH;
    const SCHEMA_SOURCE: &'static str = T::SCHEMA_SOURCE;
}

impl<K: GenomeSafe + GenomeOrd + Ord, V: GenomeSafe> GenomeSafe
    for std::collections::BTreeMap<K, V>
{
    const SCHEMA_HASH: u128 = schema_hash_combine(
        schema_hash_combine(schema_hash_bytes(b"BTreeMap"), K::SCHEMA_HASH),
        V::SCHEMA_HASH,
    );
    const SCHEMA_SOURCE: &'static str = "BTreeMap<_, _>";
}

impl<T: GenomeSafe + GenomeOrd + Ord> GenomeSafe for std::collections::BTreeSet<T> {
    const SCHEMA_HASH: u128 = schema_hash_combine(schema_hash_bytes(b"BTreeSet"), T::SCHEMA_HASH);
    const SCHEMA_SOURCE: &'static str = "BTreeSet<_>";
}

// ---------------------------------------------------------------------------
// Blanket impls — smart pointers and wrappers
// ---------------------------------------------------------------------------
//
// Box<T> and Arc<T> are hash-transparent: they delegate to T's hash.
// Wrapping or unwrapping Box/Arc is schema-compatible.
//
// No impl for Rc<T>: !Send, incompatible with async runtimes (Tokio/Axum).
// Users needing shared ownership should use Arc<T>.

impl<T: GenomeSafe> GenomeSafe for std::sync::Arc<T> {
    const SCHEMA_HASH: u128 = T::SCHEMA_HASH;
    const SCHEMA_SOURCE: &'static str = T::SCHEMA_SOURCE;
}

impl<T: GenomeSafe + ToOwned + ?Sized> GenomeSafe for std::borrow::Cow<'_, T> {
    const SCHEMA_HASH: u128 = T::SCHEMA_HASH;
    const SCHEMA_SOURCE: &'static str = T::SCHEMA_SOURCE;
}

// PhantomData always hashes as "PhantomData" regardless of T.
// Changing PhantomData<A> to PhantomData<B> is NOT a schema-breaking change.
impl<T: GenomeSafe + ?Sized> GenomeSafe for core::marker::PhantomData<T> {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"PhantomData");
    const SCHEMA_SOURCE: &'static str = "PhantomData";
}

// ---------------------------------------------------------------------------
// Blanket impls — references (for zero-copy deserialization)
// ---------------------------------------------------------------------------

impl GenomeSafe for &str {
    const SCHEMA_HASH: u128 = <str as GenomeSafe>::SCHEMA_HASH;
    const SCHEMA_SOURCE: &'static str = "&str";
}

impl GenomeSafe for &[u8] {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"bytes");
    const SCHEMA_SOURCE: &'static str = "&[u8]";
}

// ---------------------------------------------------------------------------
// Blanket impls — fixed-size arrays
// ---------------------------------------------------------------------------

impl<T: GenomeSafe, const N: usize> GenomeSafe for [T; N] {
    const SCHEMA_HASH: u128 = schema_hash_combine(
        schema_hash_bytes(b"array"),
        // Include the array length in the hash to distinguish [u8; 4] from [u8; 8].
        schema_hash_combine(T::SCHEMA_HASH, N as u128),
    );
    const SCHEMA_SOURCE: &'static str = "[_; N]";
}

// ---------------------------------------------------------------------------
// Blanket impls — tuples (up to 16 elements, matching serde's limit)
// ---------------------------------------------------------------------------

// Tuples use a chained combine: hash("tuple2") ⊕ T0::HASH ⊕ T1::HASH ...
// This is order-dependent by construction.

macro_rules! impl_genome_safe_tuple {
    ($label:expr, $($T:ident),+) => {
        impl<$($T: GenomeSafe),+> GenomeSafe for ($($T,)+) {
            const SCHEMA_HASH: u128 = {
                let mut h = schema_hash_bytes($label.as_bytes());
                $(
                    h = schema_hash_combine(h, $T::SCHEMA_HASH);
                )+
                h
            };
            const SCHEMA_SOURCE: &'static str = concat!("(", $(stringify!($T), ", ",)+ ")");
        }
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

// ---------------------------------------------------------------------------
// GenomeOrd impls — primitives
// ---------------------------------------------------------------------------
//
// Only types with a deterministic, total Ord are included.
// Excluded: f32, f64 (no Ord in std), Box, Arc, Cow, &str, &[u8] (runtime
// wrappers / borrowed types — not idiomatic as map keys).

macro_rules! impl_genome_ord_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(impl GenomeOrd for $ty {})+
    };
}

impl_genome_ord_primitive!(bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, char,);

impl GenomeOrd for () {}
impl GenomeOrd for String {}

// ---------------------------------------------------------------------------
// GenomeOrd impls — containers
// ---------------------------------------------------------------------------

impl<T: GenomeOrd> GenomeOrd for Option<T> {}

// ---------------------------------------------------------------------------
// GenomeOrd impls — fixed-size arrays
// ---------------------------------------------------------------------------

impl<T: GenomeOrd, const N: usize> GenomeOrd for [T; N] {}

// ---------------------------------------------------------------------------
// GenomeOrd impls — tuples (up to 16 elements, matching serde's limit)
// ---------------------------------------------------------------------------

macro_rules! impl_genome_ord_tuple {
    ($($T:ident),+) => {
        impl<$($T: GenomeOrd),+> GenomeOrd for ($($T,)+) {}
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
