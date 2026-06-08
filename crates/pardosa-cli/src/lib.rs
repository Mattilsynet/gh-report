#![forbid(unsafe_code)]
//! `pardosa-cli` crate root.
//!
//! Library surface for the `pardosa-cli` binary: re-exports the
//! [`DomainEvent`] enum (defined in [`event`]) so the binary and the
//! integration tests under `tests/` can both consume it without
//! depending on an internal module path. This crate is `publish =
//! false`; the only adopter-facing surface is the `pardosa-cli`
//! binary itself.
/// Domain-event vocabulary for the CLI: the [`DomainEvent`] enum, its
/// per-field bounded-string `MAX` constants under [`event::limits`],
/// and the impls (`Validate`, `HasEventSchemaSource`) wiring it into
/// [`pardosa::store::EventStore`].
pub mod event;
/// Re-export of [`event::DomainEvent`] so binary and integration-test
/// call sites can write `pardosa_cli::DomainEvent` without naming the
/// internal `event` module path.
pub use event::DomainEvent;
