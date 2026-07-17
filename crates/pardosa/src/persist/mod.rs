//! Persistence: write a Line's event sequence to a `.pgno` container
//! and rehydrate it on read.
//!
//! Crate-internal surface:
//!
//! - [`persist_with_source`] — write a `Line<T>`'s event line,
//!   optionally embedding a schema-source descriptor in the footer.
//! - [`stream_checked`] / [`stream_validated`]
//!   — iterate `Event<T>` from a `.pgno` source, with optional
//!   exclusive `resume_after`.
//! - [`rehydrate_unchecked`] — fold a stream into a `Line<T>`.
//!
//! Frontier folding lives only on [`rehydrate_unchecked`]; raw streams yield
//! `Event<T>` without rolling frontier.
pub(crate) mod checked;
pub(crate) mod error;
pub(crate) mod rehydrate;
pub(crate) mod validated;
pub use checked::{CheckedEventStream, stream_checked};
pub use error::{
    CheckedReplayKind, Error, RehydrateInvariant, UnpersistableKind, ValidatedReplayError,
};
#[cfg(any(test, feature = "test-support"))]
pub(crate) use rehydrate::persist_with_source_append;
pub(crate) use rehydrate::{persist_with_source, rehydrate_unchecked, rehydrate_validated};
pub use validated::{ValidatedEventStream, stream_validated};
#[cfg(test)]
mod tests;
