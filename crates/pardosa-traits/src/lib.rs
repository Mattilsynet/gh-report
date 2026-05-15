//! Sealed trait substrate for pardosa events (GEN-0036).
//!
//! Hosts the root marker trait [`EventSafe`] and the private sealing module
//! [`sealed`]. The trait stack is `GenomeOrd: GenomeSafe: EventSafe: Sealed`;
//! `Sealed` is the gating supertrait. External crates cannot construct a
//! `Sealed` impl, so they cannot impl `EventSafe` (or anything above it) for
//! their own types. The only blessed path is `#[derive(GenomeSafe)]`, which
//! emits `Sealed + EventSafe + GenomeSafe` atomically.
//!
//! `EventSafe` is intentionally a sealed marker in this crate revision; the
//! `Encode` supertrait bound is deferred to A2.2 (see GEN-0036 Context).
//!
//! # Why std-aware substrate
//!
//! The trusted-blanket pattern requires that impls of a foreign trait for
//! foreign types live in the trait's defining crate (Rust orphan rule E0117/
//! E0210). Sealed + EventSafe blankets for std types (`Box<T>`, `Vec<T>`,
//! `Arc<T>`, `BTreeMap<K, V>`, primitives, tuples, …) therefore live here.
//! `pardosa-traits` keeps zero external dependencies — std types only.

#![forbid(unsafe_code)]

use core::marker::PhantomData;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Sealing module. The `Sealed` trait is `pub` so it can appear in supertrait
/// bounds, but its only impls live in trusted crates that depend on
/// `pardosa-traits` plus the trusted blankets below. External crates cannot
/// impl `Sealed` for their own types, so they cannot impl `EventSafe` either.
pub mod sealed {
    /// Sealing supertrait. Implementing this is the gate that proves a type
    /// has been blessed by the derive macro (or a trusted std blanket impl).
    pub trait Sealed {}
}

/// Root marker trait of the pardosa event-type stack.
///
/// Implementations are restricted by the [`sealed::Sealed`] supertrait. Only
/// trusted crates in the pardosa workspace (and `#[derive(GenomeSafe)]`-blessed
/// user types) can satisfy the bound.
///
/// The `Encode` supertrait bound is deferred to A2.2 per GEN-0036; this crate
/// revision ships `EventSafe` as a pure sealed marker.
pub trait EventSafe: sealed::Sealed {}

// ---------------------------------------------------------------------------
// Trusted blanket impls — primitives
// ---------------------------------------------------------------------------
//
// Sealed + EventSafe coverage for every type with a hand-written GenomeSafe
// impl in pardosa-genome. Co-located here (not in pardosa-genome) because the
// orphan rule forbids impl'ing foreign traits for foreign types from a
// non-defining crate.

macro_rules! seal_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl sealed::Sealed for $ty {}
            impl EventSafe for $ty {}
        )+
    };
}

seal_primitive!(
    bool, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, f32, f64, char
);

impl sealed::Sealed for () {}
impl EventSafe for () {}

impl sealed::Sealed for str {}
impl EventSafe for str {}

impl sealed::Sealed for String {}
impl EventSafe for String {}

impl sealed::Sealed for &str {}
impl EventSafe for &str {}

impl sealed::Sealed for &[u8] {}
impl EventSafe for &[u8] {}

// ---------------------------------------------------------------------------
// Trusted blanket impls — std containers and wrappers
// ---------------------------------------------------------------------------
//
// Bounds intentionally use `T: EventSafe` (not `T: GenomeSafe`) — pardosa-traits
// cannot know GenomeSafe. The wider EventSafe bound is safe: every GenomeSafe
// type is by construction EventSafe (supertrait), and these blankets only
// need the sealing chain to reach down to leaf types.

impl<T: EventSafe> sealed::Sealed for Option<T> {}
impl<T: EventSafe> EventSafe for Option<T> {}

impl<T: EventSafe> sealed::Sealed for Vec<T> {}
impl<T: EventSafe> EventSafe for Vec<T> {}

impl<T: EventSafe> sealed::Sealed for Box<T> {}
impl<T: EventSafe> EventSafe for Box<T> {}

impl<K: EventSafe, V: EventSafe> sealed::Sealed for BTreeMap<K, V> {}
impl<K: EventSafe, V: EventSafe> EventSafe for BTreeMap<K, V> {}

impl<T: EventSafe> sealed::Sealed for BTreeSet<T> {}
impl<T: EventSafe> EventSafe for BTreeSet<T> {}

impl<T: EventSafe> sealed::Sealed for Arc<T> {}
impl<T: EventSafe> EventSafe for Arc<T> {}

impl<T: EventSafe + ToOwned + ?Sized> sealed::Sealed for Cow<'_, T> {}
impl<T: EventSafe + ToOwned + ?Sized> EventSafe for Cow<'_, T> {}

impl<T: EventSafe + ?Sized> sealed::Sealed for PhantomData<T> {}
impl<T: EventSafe + ?Sized> EventSafe for PhantomData<T> {}

impl<T: EventSafe, const N: usize> sealed::Sealed for [T; N] {}
impl<T: EventSafe, const N: usize> EventSafe for [T; N] {}

// ---------------------------------------------------------------------------
// Trusted blanket impls — tuples (1..=16, matching serde and pardosa-genome)
// ---------------------------------------------------------------------------

macro_rules! seal_tuple {
    ($($T:ident),+) => {
        impl<$($T: EventSafe),+> sealed::Sealed for ($($T,)+) {}
        impl<$($T: EventSafe),+> EventSafe for ($($T,)+) {}
    };
}

seal_tuple!(T0);
seal_tuple!(T0, T1);
seal_tuple!(T0, T1, T2);
seal_tuple!(T0, T1, T2, T3);
seal_tuple!(T0, T1, T2, T3, T4);
seal_tuple!(T0, T1, T2, T3, T4, T5);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12);
seal_tuple!(T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13);
seal_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14
);
seal_tuple!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
