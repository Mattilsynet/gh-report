use super::event_float_tag;
use super::{OrderedF32, OrderedF64};
use crate::error::DomainError;
use crate::genome_safe::{GenomeSafe, schema_hash_bytes, schema_hash_combine};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode};
use pardosa_wire::{EventSafe, Validate};
/// Total-classification `f32`: NaN and ±∞ as tag-only
/// variants; finite values via [`OrderedF32`] (rejects NaN,
/// ±∞, subnormals).
///
/// Wire = `tag: u8 ++ [payload]`. Tags `0,1,3` carry no
/// payload; tag `2` carries 4 LE bytes of `OrderedF32`'s
/// inner `f32`. Other tags →
/// [`DecodeError::TagOutOfRange`]. NaN payload/sign not
/// preserved (decode emits `f32::NAN`).
///
/// Impls [`EventSafe`], [`GenomeSafe`], [`Encode`],
/// [`Decode`], [`Validate`]. No `GenomeOrd`, no
/// `PartialOrd`/`Ord`. Bead `rescue-pardosa-clys`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum EventF32 {
    /// IEEE-754 not-a-number. Sign and payload bits are not preserved.
    NaN = 0,
    /// Negative infinity.
    NegInf = 1,
    /// Finite real value (NaN, ±∞, subnormals all excluded by
    /// [`OrderedF32`]).
    Finite(OrderedF32) = 2,
    /// Positive infinity.
    PosInf = 3,
}
impl EventF32 {
    /// Stable schema source string for [`EventF32`].
    ///
    /// Includes the enum name, every variant identifier, and every
    /// `#[repr(u8)]` discriminant in declaration order, plus the
    /// payload type for `Finite`. This is the byte sequence fed into
    /// [`SCHEMA_HASH`](GenomeSafe::SCHEMA_HASH); altering any
    /// variant, discriminant, or payload type changes both the
    /// source and the hash and is therefore an ADR-0009 breaking
    /// change.
    pub const SCHEMA_SOURCE_STR: &'static str =
        "enum EventF32 { NaN = 0, NegInf = 1, Finite(OrderedF32) = 2, PosInf = 3 }";
    /// Classify a raw `f32` into the corresponding variant.
    ///
    /// # Errors
    /// Returns [`DomainError::NotReal`] when the finite branch would
    /// produce a subnormal (rejected by [`OrderedF32`]). NaN and ±∞
    /// map to their tag-only variants and never error.
    pub fn from_f32(v: f32) -> Result<Self, DomainError> {
        if v.is_nan() {
            Ok(Self::NaN)
        } else if v == f32::INFINITY {
            Ok(Self::PosInf)
        } else if v == f32::NEG_INFINITY {
            Ok(Self::NegInf)
        } else {
            Ok(Self::Finite(OrderedF32::try_from(v)?))
        }
    }
    /// Project back to a raw `f32`. NaN-variant produces the canonical
    /// `f32::NAN` bit pattern (signalling/quieting and sign are not
    /// round-tripped).
    #[must_use]
    pub fn to_f32(self) -> f32 {
        match self {
            Self::NaN => f32::NAN,
            Self::NegInf => f32::NEG_INFINITY,
            Self::Finite(v) => v.get(),
            Self::PosInf => f32::INFINITY,
        }
    }
}
impl pardosa_wire::sealed::Sealed for EventF32 {}
impl EventSafe for EventF32 {}
impl GenomeSafe for EventF32 {
    const SCHEMA_HASH: u128 = schema_hash_combine(
        schema_hash_bytes(Self::SCHEMA_SOURCE_STR.as_bytes()),
        OrderedF32::SCHEMA_HASH,
    );
    const SCHEMA_SOURCE: &'static str = Self::SCHEMA_SOURCE_STR;
}
impl Validate for EventF32 {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        match self {
            Self::Finite(v) => v.validate(),
            Self::NaN | Self::NegInf | Self::PosInf => Ok(()),
        }
    }
}
impl Encode for EventF32 {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::NaN => out.push(event_float_tag::NAN),
            Self::NegInf => out.push(event_float_tag::NEG_INF),
            Self::Finite(v) => {
                out.push(event_float_tag::FINITE);
                v.encode(out);
            }
            Self::PosInf => out.push(event_float_tag::POS_INF),
        }
    }
}
impl Decode for EventF32 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let tag = u8::decode(d)?;
        match tag {
            event_float_tag::NAN => Ok(Self::NaN),
            event_float_tag::NEG_INF => Ok(Self::NegInf),
            event_float_tag::FINITE => Ok(Self::Finite(OrderedF32::decode(d)?)),
            event_float_tag::POS_INF => Ok(Self::PosInf),
            other => Err(DecodeError::TagOutOfRange {
                tag: u32::from(other),
            }),
        }
    }
}
/// Total-classification `f64` payload — the [`EventF32`] companion for
/// `f64`. See [`EventF32`] for the wire-layout table and rationale;
/// payload bytes are 8 LE bytes ([`OrderedF64`] inner `f64`) and tag
/// discriminants are identical. See [`EventF32`] for the rationale
/// behind omitting `PartialOrd` / `Ord`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum EventF64 {
    /// IEEE-754 not-a-number. Sign and payload bits are not preserved.
    NaN = 0,
    /// Negative infinity.
    NegInf = 1,
    /// Finite real value (NaN, ±∞, subnormals all excluded by
    /// [`OrderedF64`]).
    Finite(OrderedF64) = 2,
    /// Positive infinity.
    PosInf = 3,
}
impl EventF64 {
    /// Stable schema source string for [`EventF64`]. See
    /// [`EventF32::SCHEMA_SOURCE_STR`] for composition rules.
    pub const SCHEMA_SOURCE_STR: &'static str =
        "enum EventF64 { NaN = 0, NegInf = 1, Finite(OrderedF64) = 2, PosInf = 3 }";
    /// Classify a raw `f64` into the corresponding variant.
    ///
    /// # Errors
    /// Returns [`DomainError::NotReal`] when the finite branch would
    /// produce a subnormal (rejected by [`OrderedF64`]).
    pub fn from_f64(v: f64) -> Result<Self, DomainError> {
        if v.is_nan() {
            Ok(Self::NaN)
        } else if v == f64::INFINITY {
            Ok(Self::PosInf)
        } else if v == f64::NEG_INFINITY {
            Ok(Self::NegInf)
        } else {
            Ok(Self::Finite(OrderedF64::try_from(v)?))
        }
    }
    /// Project back to a raw `f64`.
    #[must_use]
    pub fn to_f64(self) -> f64 {
        match self {
            Self::NaN => f64::NAN,
            Self::NegInf => f64::NEG_INFINITY,
            Self::Finite(v) => v.get(),
            Self::PosInf => f64::INFINITY,
        }
    }
}
impl pardosa_wire::sealed::Sealed for EventF64 {}
impl EventSafe for EventF64 {}
impl GenomeSafe for EventF64 {
    const SCHEMA_HASH: u128 = schema_hash_combine(
        schema_hash_bytes(Self::SCHEMA_SOURCE_STR.as_bytes()),
        OrderedF64::SCHEMA_HASH,
    );
    const SCHEMA_SOURCE: &'static str = Self::SCHEMA_SOURCE_STR;
}
impl Validate for EventF64 {
    type Error = DomainError;
    fn validate(&self) -> Result<(), DomainError> {
        match self {
            Self::Finite(v) => v.validate(),
            Self::NaN | Self::NegInf | Self::PosInf => Ok(()),
        }
    }
}
impl Encode for EventF64 {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::NaN => out.push(event_float_tag::NAN),
            Self::NegInf => out.push(event_float_tag::NEG_INF),
            Self::Finite(v) => {
                out.push(event_float_tag::FINITE);
                v.encode(out);
            }
            Self::PosInf => out.push(event_float_tag::POS_INF),
        }
    }
}
impl Decode for EventF64 {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let tag = u8::decode(d)?;
        match tag {
            event_float_tag::NAN => Ok(Self::NaN),
            event_float_tag::NEG_INF => Ok(Self::NegInf),
            event_float_tag::FINITE => Ok(Self::Finite(OrderedF64::decode(d)?)),
            event_float_tag::POS_INF => Ok(Self::PosInf),
            other => Err(DecodeError::TagOutOfRange {
                tag: u32::from(other),
            }),
        }
    }
}
