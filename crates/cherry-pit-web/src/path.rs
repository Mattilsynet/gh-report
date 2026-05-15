//! URL path handling errors.
//!
//! The path-handling helpers themselves ([`normalize_request_path`],
//! [`sanitize_path_segment`]) remain re-exported flat from `lib.rs` per
//! CHE-0049:R8 + R14. Their error type lives here so the public
//! surface stays narrow.
//!
//! [`normalize_request_path`]: crate::normalize_request_path
//! [`sanitize_path_segment`]: crate::sanitize_path_segment

pub use crate::middleware::PathSegmentError;
