//! Consumer-supplied idempotency key.
//!
//! Per CHE-0046 R3 + CHE-0049 R6, idempotency keys are *never*
//! synthesised by the framework — the consumer's chosen stability
//! semantics are the only authority. This invariant is enforced at the
//! type level per CHE-0041 R5, which ratifies the carrier-type seal: the
//! public constructor [`IdempotencyKey::from_header_value`] is the only
//! way to obtain `Some(IdempotencyKey)` from outside this crate, and it
//! requires an inbound header value. The `pub(crate)`
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
/// `Serialize`/`Deserialize` are opt-in per CHE-0014:R1–R2 (the
/// `Command` trait itself carries no serde bounds) — this type derives
/// its own, hand-written per the membrane-crossing carrier role
/// described on [`Command`]. `Deserialize` is a manual implementation,
/// not `#[derive]`, so the smart-constructor invariant (non-empty,
/// trimmed) is re-checked on every deserialize and can never be
/// bypassed by constructing the wrapped `String` directly.
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
///
/// // Empty / whitespace-only input is rejected — never auto-generated.
/// assert!(IdempotencyKey::from_header_value("").is_none());
/// assert!(IdempotencyKey::from_header_value("   ").is_none());
///
/// // Round-tripping through JSON re-validates the invariant rather
/// // than bypassing it.
/// let json = serde_json::to_string(&key).unwrap();
/// let back: IdempotencyKey = serde_json::from_str(&json).unwrap();
/// assert_eq!(back, key);
/// assert!(serde_json::from_str::<IdempotencyKey>("\"\"").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(String);

impl serde::Serialize for IdempotencyKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::from_header_value(&raw).ok_or_else(|| {
            serde::de::Error::custom(
                "IdempotencyKey must be non-empty after trimming surrounding whitespace",
            )
        })
    }
}

impl IdempotencyKey {
    /// Crate-private unchecked constructor.
    ///
    /// Intentionally `pub(crate)` — downstream crates have no access to
    /// this path, which is the structural enforcement of the
    /// never-synthesise invariant. The seal itself is ratified by
    /// CHE-0041 R5; the invariant it upholds is CHE-0046 R3 + CHE-0049 R6.
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

    #[test]
    fn deserialize_rejects_empty_string_without_bypassing_validation() {
        let err = serde_json::from_str::<IdempotencyKey>("\"\"").unwrap_err();
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn deserialize_rejects_whitespace_only_string() {
        let err = serde_json::from_str::<IdempotencyKey>("\"   \"").unwrap_err();
        assert!(err.to_string().contains("non-empty"));
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_valid_key() -> impl Strategy<Value = String> {
            "[a-zA-Z0-9]{1,32}"
        }

        proptest! {
            #[test]
            fn json_roundtrip_preserves_smart_constructor_invariant(raw in arb_valid_key()) {
                let key = IdempotencyKey::from_header_value(&raw).unwrap();
                let json = serde_json::to_string(&key).unwrap();
                let back: IdempotencyKey = serde_json::from_str(&json).unwrap();

                prop_assert_eq!(&back, &key);
                prop_assert!(!back.as_str().is_empty());
                prop_assert_eq!(back.as_str(), back.as_str().trim());
            }

            #[test]
            fn json_roundtrip_padded_input_still_validates(raw in arb_valid_key()) {
                let padded = format!("  {raw}  ");
                let key = IdempotencyKey::from_header_value(&padded).unwrap();
                let json = serde_json::to_string(&key).unwrap();
                let back: IdempotencyKey = serde_json::from_str(&json).unwrap();

                prop_assert_eq!(back.as_str(), raw.as_str());
                prop_assert!(!back.as_str().is_empty());
            }
        }
    }
}
