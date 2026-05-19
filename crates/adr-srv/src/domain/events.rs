//! Domain events for the `AdrDocument` aggregate.
//!
//! ## Wire-format invariant (CHE-0064:R2 + CHE-0022:R5)
//!
//! `AdrIngested` is the single event type for M1. M1.3 onward MUST
//! NOT change its payload shape — only additive evolution (tail-
//! appended fields) is permitted per CHE-0022:R5, and even those
//! changes are deferred to a future ADR. The canonical-bytes
//! invariant (CHE-0022:R2 + CHE-0064:R2) requires that an event
//! emitted today decodes identically forever.
//!
//! Future events (e.g. `AdrRatified`, `AdrSuperseded`, `AdrRetired`)
//! land as additional types in this module in Phase 3; the M1 single-
//! event-type shape is intentional per the M1 mission contract.
//!
//! ## Payload field order (frozen)
//!
//! 1. `id: AdrId`
//! 2. `frontmatter: AdrFrontmatter`
//! 3. `body_hash: BodyHash`
//! 4. `references: Vec<AdrId>`
//!
//! Reorder, removal, or non-tail insertion is a wire break.

use pardosa_genome::GenomeSafe;

use crate::domain::adr_id::AdrId;
use crate::domain::body_hash::BodyHash;
use crate::domain::frontmatter::AdrFrontmatter;

/// First event emitted for an `AdrDocument` aggregate: the
/// ADR file was observed and parsed.
///
/// `body_hash` enables M1.3's idempotency check (re-scrape: skip if
/// the file's `body_hash` matches the last `AdrIngested.body_hash`
/// for this `id`).
#[derive(Clone, Debug, PartialEq, Eq, GenomeSafe)]
pub struct AdrIngested {
    /// Parsed ADR identifier (e.g. `AFM-0001`).
    pub id: AdrId,
    /// Projected frontmatter subset (AFM-0027:R3).
    pub frontmatter: AdrFrontmatter,
    /// xxh3-128 of the raw ADR file bytes (AFM-0027:R4).
    pub body_hash: BodyHash,
    /// Outbound `References:` edges parsed from the ADR body.
    pub references: Vec<AdrId>,
}

impl cherry_pit_core::DomainEvent for AdrIngested {
    fn event_type(&self) -> &'static str {
        "AdrIngested"
    }
}
