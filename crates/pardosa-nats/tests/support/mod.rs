//! Local re-export of the canonical [`crate::test_support::LiveNatsServer`].
//!
//! The body lives in `pardosa-nats` itself behind the `test-support`
//! feature; this module preserves the historical local path so existing
//! `mod support;` decls in sibling `tests/` files continue to resolve
//! without churn.
pub use pardosa_nats::test_support::LiveNatsServer;
