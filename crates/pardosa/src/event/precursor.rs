use super::Index;
use pardosa_schema::{GenomeSafe, schema_hash_bytes};
use pardosa_wire::{Decode, DecodeError, Decoder, Encode, EventSafe};
use serde::{Deserialize, Serialize};
use std::fmt;
/// Optional precursor pointer carried by an `Event<T>`. `Genesis`
/// marks the first event in a fiber (or an event with no in-line
/// predecessor, e.g. `rescue` from `Locked`); `Of(Index)` carries the
/// predecessor's in-line position.
///
/// Replaces the pre-F1 `Index::NONE = Index(u64::MAX)` sentinel; every
/// `Index` including `u64::MAX` is now a legal position. Wire encoding
/// is one tag byte (`0` = `Genesis`, `1` = `Of`) followed by the
/// `Index` encoding for `Of`. See ADR-0003 / ADR-0005.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Precursor {
    Genesis,
    Of(Index),
}
impl Precursor {
    #[must_use]
    pub const fn is_genesis(self) -> bool {
        matches!(self, Precursor::Genesis)
    }
    #[must_use]
    pub const fn as_index(self) -> Option<Index> {
        match self {
            Precursor::Genesis => None,
            Precursor::Of(i) => Some(i),
        }
    }
}
impl fmt::Display for Precursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Precursor::Genesis => write!(f, "Genesis"),
            Precursor::Of(i) => write!(f, "Of({i})"),
        }
    }
}
impl Encode for Precursor {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Precursor::Genesis => 0u8.encode(out),
            Precursor::Of(i) => {
                1u8.encode(out);
                i.encode(out);
            }
        }
    }
}
impl Decode for Precursor {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let tag = u8::decode(d)?;
        match tag {
            0 => Ok(Precursor::Genesis),
            1 => Ok(Precursor::Of(Index::decode(d)?)),
            other => Err(DecodeError::TagOutOfRange {
                tag: u32::from(other),
            }),
        }
    }
}
impl pardosa_wire::sealed::Sealed for Precursor {}
impl EventSafe for Precursor {}
impl GenomeSafe for Precursor {
    const SCHEMA_HASH: u128 = schema_hash_bytes(b"Precursor{Genesis,Of(Index)}");
    const SCHEMA_SOURCE: &'static str = "Precursor";
}
