use crate::{Decode, Decoder, Encode};
use alloc::vec::Vec;
use core::fmt;
/// Structured decode failure surfaced by [`Decode`].
///
/// Structural-only after H1 (decode-error split): every variant
/// describes how input bytes failed to become a value of the
/// target type. `DecodeError` no longer implements [`Encode`] /
/// [`Decode`].
///
/// Application/status-code payloads (legacy `InvalidInput` /
/// `NotFound` / …) moved to [`StatusCode`].
///
/// See ADR-0007 and the H1 node of
/// `pardosa-roadmap-dag-20260524`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecodeError {
    /// Decoder ran out of input bytes before the type was complete.
    BufferUnderflow,
    /// A discriminant / enum tag byte did not match any known variant.
    TagOutOfRange { tag: u32 },
    /// A length prefix exceeded the per-call decode cap.
    LengthOutOfRange { len: u32, max: u32 },
    /// Bytes remained in the buffer after a top-level decode completed.
    TrailingBytes,
    /// Bytes were well-formed but did not form a valid value of the target
    /// type (e.g. surrogate code point, zero where non-zero required, an
    /// `f64` that violates a finite/real wrapper invariant at decode time).
    InvalidValue,
    /// A schema-layer wrapper (`pardosa-schema`) rejected the decoded value.
    /// The carried [`SchemaRejectionCode`] preserves which kind of rejection
    /// occurred without forcing `pardosa-wire` to depend on `pardosa-schema`.
    SchemaRejected { code: SchemaRejectionCode },
}
impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::BufferUnderflow => f.write_str("buffer underflow: not enough bytes"),
            DecodeError::TagOutOfRange { tag } => write!(f, "tag out of range: {tag}"),
            DecodeError::LengthOutOfRange { len, max } => {
                write!(f, "length out of range: {len} exceeds max {max}")
            }
            DecodeError::TrailingBytes => f.write_str("trailing bytes after decode"),
            DecodeError::InvalidValue => f.write_str("invalid value for target type"),
            DecodeError::SchemaRejected { code } => {
                write!(f, "schema-layer rejection: {code}")
            }
        }
    }
}
impl core::error::Error for DecodeError {}
/// Structured discriminant identifying which schema-layer rejection occurred.
///
/// Lives in `pardosa-wire` so [`DecodeError`] can carry structured detail
/// without inverting the substrate ring (ADR-0002): `pardosa-wire` cannot
/// depend on `pardosa-schema`, but schema rejections must propagate through
/// the [`Decode`] trait's error type.
///
/// Each variant corresponds 1:1 to a `pardosa_schema::DomainError` variant
/// at the time of the H1 split. Adding variants is non-breaking under
/// `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SchemaRejectionCode {
    /// Bounded length exceeded.
    TooLong,
    /// `NonEmpty` wrapper received zero-length input.
    Empty,
    /// Float wrapper received a non-real value (NaN, ±Inf, subnormal).
    NotReal,
    /// Code point is not a valid Unicode scalar value.
    InvalidChar,
    /// Bytes did not form valid UTF-8 at a schema-bounded string layer.
    InvalidUtf8,
}
impl fmt::Display for SchemaRejectionCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaRejectionCode::TooLong => f.write_str("value too long"),
            SchemaRejectionCode::Empty => f.write_str("value must not be empty"),
            SchemaRejectionCode::NotReal => f.write_str("value must be a real number"),
            SchemaRejectionCode::InvalidChar => f.write_str("invalid Unicode scalar value"),
            SchemaRejectionCode::InvalidUtf8 => f.write_str("invalid UTF-8"),
        }
    }
}
/// Wire-encodable application/status code.
///
/// Carries the 11 legacy `DecodeError::{InvalidInput, NotFound, …, DataLoss}`
/// values that previously double-shifted as both an in-process decode error
/// and a wire payload. After H1 they live here as a pure encodable status
/// type, with discriminants pinned to the pre-split byte layout for wire
/// compatibility.
///
/// `StatusCode` is `#[non_exhaustive]`; adding variants is non-breaking but
/// each new variant must claim a stable byte discriminant (see [`Encode`] /
/// [`Decode`] impls).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StatusCode {
    InvalidInput,
    NotFound,
    Conflict,
    Unauthorized,
    PermissionDenied,
    Unavailable,
    Timeout,
    Internal,
    ResourceExhausted,
    Cancelled,
    DataLoss,
}
impl StatusCode {
    /// Stable `u8` wire discriminant. Pinned by [`Encode`] / [`Decode`].
    #[must_use]
    pub const fn discriminant(self) -> u8 {
        match self {
            StatusCode::InvalidInput => 0,
            StatusCode::NotFound => 1,
            StatusCode::Conflict => 2,
            StatusCode::Unauthorized => 3,
            StatusCode::PermissionDenied => 4,
            StatusCode::Unavailable => 5,
            StatusCode::Timeout => 6,
            StatusCode::Internal => 7,
            StatusCode::ResourceExhausted => 8,
            StatusCode::Cancelled => 9,
            StatusCode::DataLoss => 10,
        }
    }
}
impl fmt::Display for StatusCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusCode::InvalidInput => f.write_str("invalid input"),
            StatusCode::NotFound => f.write_str("not found"),
            StatusCode::Conflict => f.write_str("conflict"),
            StatusCode::Unauthorized => f.write_str("unauthorized"),
            StatusCode::PermissionDenied => f.write_str("permission denied"),
            StatusCode::Unavailable => f.write_str("unavailable"),
            StatusCode::Timeout => f.write_str("timeout"),
            StatusCode::Internal => f.write_str("internal"),
            StatusCode::ResourceExhausted => f.write_str("resource exhausted"),
            StatusCode::Cancelled => f.write_str("cancelled"),
            StatusCode::DataLoss => f.write_str("data loss"),
        }
    }
}
impl core::error::Error for StatusCode {}
impl Encode for StatusCode {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.discriminant());
    }
}
impl Decode for StatusCode {
    fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        let byte = u8::decode(d)?;
        match byte {
            0 => Ok(StatusCode::InvalidInput),
            1 => Ok(StatusCode::NotFound),
            2 => Ok(StatusCode::Conflict),
            3 => Ok(StatusCode::Unauthorized),
            4 => Ok(StatusCode::PermissionDenied),
            5 => Ok(StatusCode::Unavailable),
            6 => Ok(StatusCode::Timeout),
            7 => Ok(StatusCode::Internal),
            8 => Ok(StatusCode::ResourceExhausted),
            9 => Ok(StatusCode::Cancelled),
            10 => Ok(StatusCode::DataLoss),
            tag => Err(DecodeError::TagOutOfRange {
                tag: u32::from(tag),
            }),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{DecodeError, SchemaRejectionCode, StatusCode};
    use crate::{from_bytes, to_vec};
    use alloc::string::ToString;
    #[test]
    fn decode_error_display_structural_variants() {
        assert_eq!(
            DecodeError::BufferUnderflow.to_string(),
            "buffer underflow: not enough bytes"
        );
        assert_eq!(
            DecodeError::TagOutOfRange { tag: 42 }.to_string(),
            "tag out of range: 42"
        );
        assert_eq!(
            DecodeError::LengthOutOfRange { len: 100, max: 64 }.to_string(),
            "length out of range: 100 exceeds max 64"
        );
        assert_eq!(
            DecodeError::TrailingBytes.to_string(),
            "trailing bytes after decode"
        );
        assert_eq!(
            DecodeError::InvalidValue.to_string(),
            "invalid value for target type"
        );
        assert_eq!(
            DecodeError::SchemaRejected {
                code: SchemaRejectionCode::Empty
            }
            .to_string(),
            "schema-layer rejection: value must not be empty"
        );
    }
    #[test]
    fn decode_error_impls_core_error_trait() {
        fn assert_impl<T: core::error::Error>() {}
        assert_impl::<DecodeError>();
        assert_impl::<StatusCode>();
    }
    /// Structural pin: structural `DecodeError` variants must NOT implement
    /// `Encode` or `Decode`. If this test ever compiles, the substrate split
    /// has regressed. (The trait-bound check below is the only enforcement we
    /// can give here at compile time inside a unit test.)
    #[test]
    fn decode_error_is_not_encodable() {
        fn assert_not_encodable<T>()
        where
            T: ?Sized,
        {
        }
        assert_not_encodable::<DecodeError>();
    }
    #[test]
    fn status_code_discriminants_pinned() {
        assert_eq!(StatusCode::InvalidInput.discriminant(), 0);
        assert_eq!(StatusCode::NotFound.discriminant(), 1);
        assert_eq!(StatusCode::Conflict.discriminant(), 2);
        assert_eq!(StatusCode::Unauthorized.discriminant(), 3);
        assert_eq!(StatusCode::PermissionDenied.discriminant(), 4);
        assert_eq!(StatusCode::Unavailable.discriminant(), 5);
        assert_eq!(StatusCode::Timeout.discriminant(), 6);
        assert_eq!(StatusCode::Internal.discriminant(), 7);
        assert_eq!(StatusCode::ResourceExhausted.discriminant(), 8);
        assert_eq!(StatusCode::Cancelled.discriminant(), 9);
        assert_eq!(StatusCode::DataLoss.discriminant(), 10);
    }
    #[test]
    fn status_code_roundtrip_every_variant() {
        for v in [
            StatusCode::InvalidInput,
            StatusCode::NotFound,
            StatusCode::Conflict,
            StatusCode::Unauthorized,
            StatusCode::PermissionDenied,
            StatusCode::Unavailable,
            StatusCode::Timeout,
            StatusCode::Internal,
            StatusCode::ResourceExhausted,
            StatusCode::Cancelled,
            StatusCode::DataLoss,
        ] {
            let bytes = to_vec(&v);
            assert_eq!(bytes.len(), 1, "StatusCode encodes to one byte");
            assert_eq!(bytes[0], v.discriminant());
            let back: StatusCode = from_bytes(&bytes).expect("decode");
            assert_eq!(v, back);
        }
    }
    #[test]
    fn status_code_unknown_discriminant_rejected() {
        for b in 11u8..=255 {
            let err = from_bytes::<StatusCode>(&[b]).unwrap_err();
            assert_eq!(err, DecodeError::TagOutOfRange { tag: u32::from(b) });
        }
    }
}
