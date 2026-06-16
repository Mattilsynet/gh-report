//! Adopter prelude (ADR-0002 §D6).
//!
//! Re-exports the items consumers most commonly need at the top of
//! a file. Every item below is already reachable via
//! [`pardosa::store`](crate::store); the prelude broadens nothing
//! — it is a single-glob ergonomic shortcut for
//! `use pardosa::prelude::*;`. Substrate crates stay out of the
//! consumer surface. Drift pinned by
//! `tests/ui_pass/prelude_usable.rs`.
pub use crate::store::{
    AppendReceipt, CausalChain, CausalChainError, CausalChainIter, CausalChainStrictIter, Decode,
    DetachReceipt, DetachedFiber, Encode, EnvelopeError, Event, EventId, EventStore, ExtractError,
    FiberHistory, FiberHistoryIter, FiberId, FiberIndex, FiberLookup, FiberState, Frontier,
    FrontierPublisher, GenomeSafe, HasEventSchemaSource, HistoryStream, Index, LineCursor,
    LiveFiber, Lsn, PardosaError, Precursor, PublishError, RecoveryOutcome,
    RecoveryReaderErrorKind, StoreMetadata, StoreReader, StoreWriter, Validate,
    ValidatedReplayError, migrate, replay,
};
