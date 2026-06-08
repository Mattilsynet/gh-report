#![forbid(unsafe_code)]
//! Workspace test-support harness for `pardosa`.
//!
//! Holds integration tests requiring the
//! `pardosa::store::test_support` surface (ADR-0018 §D7,
//! ADR-0022 §D11). Living here lets `pardosa` avoid a
//! `pardosa → pardosa` dev-dep cycle.
//!
//! The harness depends on `pardosa` with `features =
//! ["test-support"]`, but that does not make the test-support
//! surface part of the adopter API: `EventStore::open` is still
//! `pub(crate)` under default features; sealed
//! `AuthoritativeBackend` / `BackendSink` traits remain impossible
//! to impl from outside `pardosa`.
