use core::fmt;
use pardosa_wire::{DecodeError, SchemaRejectionCode};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DomainError {
    /// Bounded length exceeded: `actual > max`.
    TooLong { max: usize, actual: usize },
    /// `NonEmpty` wrapper received zero-length input.
    Empty,
    /// Float wrapper received a value that is not a real number
    /// (`NaN`, ±Inf, or subnormal per `RealF*`/`OrderedF*` contract).
    NotReal,
    /// `char::from_u32` rejected the code point as a UTF-16 surrogate
    /// (U+D800..=U+DFFF) or as out-of-Unicode-range (> U+10FFFF).
    InvalidChar { code: u32 },
    /// Bytes did not form valid UTF-8 at the schema-bounded String layer.
    InvalidUtf8,
}
impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { max, actual } => {
                write!(f, "value too long: {actual} exceeds max {max}")
            }
            Self::Empty => f.write_str("value must not be empty"),
            Self::NotReal => {
                f.write_str("value must be a real number (NaN, ±inf, subnormal rejected)")
            }
            Self::InvalidChar { code } => {
                write!(f, "invalid Unicode scalar value: 0x{code:08X}")
            }
            Self::InvalidUtf8 => f.write_str("invalid UTF-8 in bounded string"),
        }
    }
}
impl core::error::Error for DomainError {}
/// Preserve structured rejection detail through the substrate boundary.
///
/// Each [`DomainError`] variant maps to a distinct
/// [`SchemaRejectionCode`] carried by
/// [`DecodeError::SchemaRejected`]. Prior to H1 every variant collapsed to a
/// single `DecodeError::InvalidInput`; that conflation is now removed.
impl From<DomainError> for DecodeError {
    fn from(e: DomainError) -> Self {
        let code = match e {
            DomainError::TooLong { .. } => SchemaRejectionCode::TooLong,
            DomainError::Empty => SchemaRejectionCode::Empty,
            DomainError::NotReal => SchemaRejectionCode::NotReal,
            DomainError::InvalidChar { .. } => SchemaRejectionCode::InvalidChar,
            DomainError::InvalidUtf8 => SchemaRejectionCode::InvalidUtf8,
        };
        DecodeError::SchemaRejected { code }
    }
}
#[cfg(test)]
mod tests {
    use super::DomainError;
    use pardosa_wire::DecodeError;
    #[test]
    fn domain_error_impls_core_error_trait() {
        fn assert_impl<T: core::error::Error>() {}
        assert_impl::<DomainError>();
    }
    #[test]
    fn domain_error_display_strings() {
        assert_eq!(
            DomainError::TooLong {
                max: 16,
                actual: 32
            }
            .to_string(),
            "value too long: 32 exceeds max 16",
        );
        assert_eq!(DomainError::Empty.to_string(), "value must not be empty");
        assert_eq!(
            DomainError::NotReal.to_string(),
            "value must be a real number (NaN, ±inf, subnormal rejected)",
        );
        assert_eq!(
            DomainError::InvalidChar { code: 0xD800 }.to_string(),
            "invalid Unicode scalar value: 0x0000D800",
        );
        assert_eq!(
            DomainError::InvalidUtf8.to_string(),
            "invalid UTF-8 in bounded string",
        );
    }
    #[test]
    fn from_domain_error_to_decode_error_preserves_structure() {
        use pardosa_wire::SchemaRejectionCode;
        let e_empty: DecodeError = DomainError::Empty.into();
        assert_eq!(
            e_empty,
            DecodeError::SchemaRejected {
                code: SchemaRejectionCode::Empty
            }
        );
        let e_too_long: DecodeError = DomainError::TooLong { max: 8, actual: 16 }.into();
        assert_eq!(
            e_too_long,
            DecodeError::SchemaRejected {
                code: SchemaRejectionCode::TooLong
            }
        );
        let e_invalid_char: DecodeError = DomainError::InvalidChar { code: 0xD800 }.into();
        assert_eq!(
            e_invalid_char,
            DecodeError::SchemaRejected {
                code: SchemaRejectionCode::InvalidChar
            }
        );
        let e_utf8: DecodeError = DomainError::InvalidUtf8.into();
        assert_eq!(
            e_utf8,
            DecodeError::SchemaRejected {
                code: SchemaRejectionCode::InvalidUtf8
            }
        );
        assert_ne!(e_empty, e_too_long);
        assert_ne!(e_empty, e_invalid_char);
        assert_ne!(e_too_long, e_invalid_char);
        assert_ne!(e_invalid_char, e_utf8);
    }
}
