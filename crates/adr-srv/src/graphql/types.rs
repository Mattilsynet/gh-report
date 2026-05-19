//! Flat GraphQL DTOs (Vernon Ch. 10 Rule 4).
//!
//! These types are NOT the domain aggregate (`AdrDocument`). They are
//! denormalised projection views: stable, additive, intended for wire
//! consumption. Mutating an aggregate field's representation does not
//! reshape these DTOs; widening these DTOs is an additive schema
//! change (new optional field) per GraphQL norms.

use async_graphql::SimpleObject;

/// Flat GraphQL DTO for an ADR.
#[derive(SimpleObject, Clone, Debug)]
pub struct AdrGql {
    /// Canonical id, `"AFM-0001"` form.
    pub id: String,
    /// Frontmatter title.
    pub title: String,
    /// `Date:` ISO-8601 (`YYYY-MM-DD`).
    pub date: String,
    /// `Last-reviewed:` ISO-8601 (`YYYY-MM-DD`).
    pub last_reviewed: String,
    /// One of `"S" | "A" | "B" | "C" | "D"` (`Tier::as_str`).
    pub tier: String,
    /// `Status::as_str` token, e.g. `"Accepted"`.
    pub status: String,
    /// Lowercase 32-char hex of xxh3-128 body hash.
    pub body_hash: String,
    /// Outbound `References:` edges in source order, duplicates
    /// preserved (M1.3 invariant).
    pub references: Vec<AdrRef>,
}

/// Flat reference DTO. Id-only in v0.1; Phase 3 may widen to embed
/// the referenced ADR's title or status without breaking the wire
/// (additive field).
#[derive(SimpleObject, Clone, Debug)]
pub struct AdrRef {
    /// Referenced ADR id, `"AFM-0001"` form.
    pub id: String,
}
