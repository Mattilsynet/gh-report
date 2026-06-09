//! `AdrDocument` — event-sourced aggregate per CHE-0005:R1
//! (one aggregate PER ADR FILE, not corpus-wide).
//!
//! State is the latest-known projection of all events on this
//! aggregate's stream. M1 ships one event type (`AdrIngested`); Phase
//! 3 adds `AdrRatified` / `AdrSuperseded` / `AdrRetired`, at which
//! point `apply` becomes a `match` on the event enum.

use crate::domain::adr_id::AdrId;
use crate::domain::body_hash::BodyHash;
use crate::domain::events::AdrIngested;
use crate::domain::frontmatter::AdrFrontmatter;

/// Latest-known projection of an ADR file's event stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdrDocument {
    /// ADR identifier.
    pub id: AdrId,
    /// Latest projected frontmatter.
    pub frontmatter: AdrFrontmatter,
    /// Latest body hash.
    pub body_hash: BodyHash,
    /// Latest outbound references.
    pub references: Vec<AdrId>,
}

impl AdrDocument {
    /// Apply an `AdrIngested` event to produce the next state. With
    /// only one event type in M1 this is total-state replacement;
    /// Phase 3 events apply field-selective updates.
    #[must_use]
    pub fn apply(self, event: &AdrIngested) -> Self {
        let _ = self;
        Self {
            id: event.id.clone(),
            frontmatter: event.frontmatter.clone(),
            body_hash: event.body_hash,
            references: event.references.clone(),
        }
    }

    /// Construct the initial state from the first `AdrIngested` event
    /// in a stream. M1.3's load-replay path uses this to seed the
    /// fold before applying subsequent events.
    #[must_use]
    pub fn from_first(event: &AdrIngested) -> Self {
        Self {
            id: event.id.clone(),
            frontmatter: event.frontmatter.clone(),
            body_hash: event.body_hash,
            references: event.references.clone(),
        }
    }
}
