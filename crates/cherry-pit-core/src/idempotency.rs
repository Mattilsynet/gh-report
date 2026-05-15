//! Consumer-supplied idempotency key.
//!
//! Per CHE-0046 R3 + CHE-0049 R6, idempotency keys are *never*
//! synthesised by the framework — the consumer's chosen stability
//! semantics are the only authority. This invariant is enforced at the
//! type level: the public constructor [`IdempotencyKey::from_header_value`]
//! is the only way to obtain `Some(IdempotencyKey)` from outside this
//! crate, and it requires an inbound header value. The `pub(crate)`
//! [`IdempotencyKey::new_unchecked`] is not re-exported and is
//! inaccessible to downstream crates.
//!
//! The type owns no I/O and pulls no async / storage / http dependencies
//! (CHE-0029 R4–R5).

/// Consumer-supplied idempotency key (CHE-0046 R3, CHE-0049 R6).
///
/// Stable newtype around [`String`]. The wrapped value is the raw
/// header string after trimming surrounding whitespace, byte-for-byte
/// what the consumer supplied — no normalisation, no canonicalisation.
/// Per CHE-0046 R3 the consumer's chosen stability semantics are the
/// only authority; the framework must not transform the value.
///
/// # Examples
///
/// ```
/// use cherry_pit_core::IdempotencyKey;
///
/// // The only public path that yields `Some` is one fed by an
/// // inbound header value (CHE-0046 R3 + CHE-0049 R6).
/// let key = IdempotencyKey::from_header_value("  client-key-42  ").unwrap();
/// assert_eq!(key.as_str(), "client-key-42");
/// assert_eq!(key.into_inner(), String::from("client-key-42"));
///
/// // Empty / whitespace-only input is rejected — never auto-generated.
/// assert!(IdempotencyKey::from_header_value("").is_none());
/// assert!(IdempotencyKey::from_header_value("   ").is_none());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Crate-private unchecked constructor.
    ///
    /// Intentionally `pub(crate)` — downstream crates have no access to
    /// this path, which is the structural enforcement of the
    /// never-synthesise invariant (CHE-0046 R3 + CHE-0049 R6).
    pub(crate) fn new_unchecked(s: String) -> Self {
        Self(s)
    }

    /// Construct an `IdempotencyKey` from an inbound header value.
    ///
    /// The value is trimmed of surrounding whitespace. Returns `None`
    /// when the trimmed value is empty. Per CHE-0046 R3 + CHE-0049 R6
    /// this never synthesises — the only path that yields `Some` is
    /// one fed by an inbound header.
    #[must_use]
    pub fn from_header_value(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::new_unchecked(trimmed.to_string()))
    }

    /// The raw header value, exactly as supplied by the consumer
    /// (whitespace-trimmed).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the inner string.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_header_value_some_for_non_empty() {
        let key = IdempotencyKey::from_header_value("abc").unwrap();
        assert_eq!(key.as_str(), "abc");
    }

    #[test]
    fn from_header_value_trims_surrounding_whitespace() {
        let key = IdempotencyKey::from_header_value("  abc  ").unwrap();
        assert_eq!(key.as_str(), "abc");
    }

    #[test]
    fn from_header_value_none_for_empty() {
        assert!(IdempotencyKey::from_header_value("").is_none());
    }

    #[test]
    fn from_header_value_none_for_whitespace_only() {
        assert!(IdempotencyKey::from_header_value("   \t ").is_none());
    }

    #[test]
    fn into_inner_returns_string() {
        let key = IdempotencyKey::from_header_value("xyz").unwrap();
        assert_eq!(key.into_inner(), String::from("xyz"));
    }
}
