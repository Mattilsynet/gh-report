//! `AdrFrontmatter` — value-object mirroring the subset of
//! `adr_fmt::model::AdrRecord` frontmatter fields exposed via the
//! GraphQL surface in M1.4.
//!
//! Per AFM-0027:R3, adr-srv RE-PROJECTS `adr_fmt::report::Diagnostic`
//! and friends into its own API types; it does NOT re-export them.
//! `AdrFrontmatter` is the same pattern for `AdrRecord`'s frontmatter:
//! local fields with locally-controlled wire shape.
//!
//! Wire shape (load-bearing, frozen for M1.2 onward; field order is
//! serde declaration-order via msgpack):
//!   1. `title: String`
//!   2. `date: AdrDate`
//!   3. `last_reviewed: AdrDate`
//!   4. `tier: Tier` — u8 discriminant per `#[repr(u8)]`
//!   5. `status: Status` — u8 discriminant per `#[repr(u8)]`
//!
//! `Tier` and `Status` are LOCAL enums (mirror of
//! `adr_fmt::model::{Tier, Status}`, but with locally-controlled
//! wire identity). Variant order is appended-only (CHE-0022:R5).

use serde::{Deserialize, Serialize};

use crate::domain::adr_date::AdrDate;

/// Tier classification mirror of `adr_fmt::model::Tier`. Variants
/// match `adr-fmt`'s tier set; the wire discriminant is local.
///
/// Wire shape: `u8` discriminant (`S=0, A=1, B=2, C=3, D=4`) — same
/// numeric ranks as `adr_fmt::model::Tier::rank()` for coincidence,
/// but the values are pinned here, not inherited.
///
/// Variants appended only (CHE-0022:R5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Tier {
    /// Paradigm / intent.
    S = 0,
    /// Self-organization / structural evolvability.
    A = 1,
    /// Design / type contracts.
    B = 2,
    /// Feedbacks / runtime behaviour.
    C = 3,
    /// Parameters / implementation details.
    D = 4,
}

impl Tier {
    /// Numeric rank (matches `adr_fmt::model::Tier::rank()` for
    /// coincidence, but the values are pinned here, not inherited).
    /// Used for stable ordering in projections (M1.4).
    #[must_use]
    pub fn rank(self) -> u8 {
        self as u8
    }

    /// Single-letter token (`"S"`, `"A"`, `"B"`, `"C"`, `"D"`).
    /// GraphQL `AdrGql.tier` projection (M1.4) is the only caller.
    // Stable token surface for GraphQL read-side; widening to a
    // longer label would be a wire break for any downstream client.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::S => "S",
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
        }
    }
}

impl core::fmt::Display for Tier {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Status {
    /// Single-token name (`"Draft"`, `"Proposed"`, …).
    // Pinned token surface for GraphQL `AdrGql.status` projection
    // (M1.4): avoids relying on Debug-formatting whose output is
    // technically allowed to drift between rustc versions.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Proposed => "Proposed",
            Self::Accepted => "Accepted",
            Self::Rejected => "Rejected",
            Self::Deprecated => "Deprecated",
            Self::Superseded => "Superseded",
            Self::Invalid => "Invalid",
        }
    }
}

impl core::fmt::Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ADR lifecycle status mirror of `adr_fmt::model::Status`.
///
/// Wire shape: `u8` discriminant. `adr_fmt::model::Status`'s payload
/// variants (`SupersededBy(AdrId)`, `Invalid(String)`) are NOT mirrored
/// here for M1.2 — the frontmatter projection captures the lifecycle
/// state only. M1.3+ surfaces the supersedes target via a separate
/// event (`AdrSuperseded` in Phase 3) per CHE-0064:R2 additive-event
/// evolution.
///
/// Variants appended only (CHE-0022:R5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Status {
    /// Draft — pre-proposal.
    Draft = 0,
    /// Proposed but not yet accepted.
    Proposed = 1,
    /// Accepted; binding.
    Accepted = 2,
    /// Rejected; never adopted.
    Rejected = 3,
    /// Deprecated without explicit superseder.
    Deprecated = 4,
    /// Superseded by another ADR. The target id is captured by a
    /// separate `AdrSuperseded` event in Phase 3, not by a payload
    /// on this discriminant.
    Superseded = 5,
    /// Status line in source did not parse to a known variant.
    Invalid = 6,
}

// adr-fmt → adr-srv `Status` / `Tier` conversion bridges land in M1.3
// alongside the scrape pipeline. AFM-0026:R1 does not currently
// re-export `adr_fmt::Status` (only `Tier`); the conversion call
// site in M1.3 is the right place to decide whether to (a) amend
// AFM-0026:R1's surface to add `Status`, or (b) bridge via a local
// helper that pattern-matches against `adr_fmt::parse_domain`
// output. M1.2's job is wire-shape pinning only.

/// Subset of ADR frontmatter exposed via adr-srv's GraphQL surface.
///
/// NOT a re-export of `adr_fmt::model::AdrRecord` per AFM-0027:R3 —
/// adr-srv re-projects.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdrFrontmatter {
    /// ADR title (the `# Title` heading, not the filename slug).
    pub title: String,
    /// Date the ADR was authored (frontmatter `Date:`).
    pub date: AdrDate,
    /// Date the ADR was last reviewed (`Last-reviewed:`).
    pub last_reviewed: AdrDate,
    /// Tier classification (`Tier:`).
    pub tier: Tier,
    /// Lifecycle status (`Status:`).
    pub status: Status,
}
