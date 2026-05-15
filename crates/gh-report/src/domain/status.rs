//! Collection status types for data-gathering workflows.
//!
//! Used by org-level alert collection and any collector that needs a
//! structured success/failure/unavailable outcome.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Status of the org-level alert collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CollectionStatus {
    /// Alert data was successfully collected.
    Success,
    /// Alert collection was not attempted.
    NotCollected,
    /// The API returned a permission error (403 or 404).
    PermissionDenied,
    /// The API returned a transient error; collection may succeed on retry.
    TransientError,
    /// The API capability is not available for this token or installation.
    Unavailable,
}

impl fmt::Display for CollectionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::NotCollected => write!(f, "not_collected"),
            Self::PermissionDenied => write!(f, "permission_denied"),
            Self::TransientError => write!(f, "transient_error"),
            Self::Unavailable => write!(f, "unavailable"),
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_all_variants() {
        let variants = [
            (CollectionStatus::Success, "\"success\""),
            (CollectionStatus::NotCollected, "\"not_collected\""),
            (CollectionStatus::PermissionDenied, "\"permission_denied\""),
            (CollectionStatus::TransientError, "\"transient_error\""),
            (CollectionStatus::Unavailable, "\"unavailable\""),
        ];
        for (status, expected_json) in &variants {
            let serialized = serde_json::to_string(status).unwrap();
            assert_eq!(
                &serialized, expected_json,
                "serialization mismatch for {status:?}"
            );
            let deserialized: CollectionStatus = serde_json::from_str(&serialized).unwrap();
            assert_eq!(*status, deserialized, "round-trip mismatch for {status:?}");
        }
    }

    #[test]
    fn display_matches_serde_names() {
        assert_eq!(CollectionStatus::Success.to_string(), "success");
        assert_eq!(CollectionStatus::NotCollected.to_string(), "not_collected");
        assert_eq!(
            CollectionStatus::PermissionDenied.to_string(),
            "permission_denied"
        );
        assert_eq!(
            CollectionStatus::TransientError.to_string(),
            "transient_error"
        );
        assert_eq!(CollectionStatus::Unavailable.to_string(), "unavailable");
    }

    #[test]
    fn deserialize_unknown_variant_fails() {
        let result = serde_json::from_str::<CollectionStatus>("\"bogus_variant\"");
        assert!(
            result.is_err(),
            "unknown variant should fail deserialization"
        );
    }

    #[test]
    fn deserialize_wrong_case_fails() {
        // PascalCase is not accepted — only snake_case.
        let result = serde_json::from_str::<CollectionStatus>("\"Success\"");
        assert!(
            result.is_err(),
            "PascalCase variant should fail deserialization"
        );
    }

    #[test]
    fn deserialize_empty_string_fails() {
        let result = serde_json::from_str::<CollectionStatus>("\"\"");
        assert!(result.is_err(), "empty string should fail deserialization");
    }
}
