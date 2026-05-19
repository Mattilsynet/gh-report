//! Domain types for adr-srv.
//!
//! All canonical-bytes payload types derive `pardosa_genome::GenomeSafe`,
//! which emits `pardosa_encoding::Encode` + `Decode` per
//! `crates/pardosa-derive/src/lib.rs` L26-33. Wire-shape decisions
//! are documented at each type; reorder / non-tail insertion is a
//! wire break per CHE-0022:R5 / CHE-0064:R2.

pub mod adr_date;
pub mod adr_id;
pub mod aggregate;
pub mod body_hash;
pub mod events;
pub mod frontmatter;

pub use adr_date::{AdrDate, AdrDateError};
pub use adr_id::{AdrId, AdrIdError, KNOWN_DOMAINS};
pub use aggregate::AdrDocument;
pub use body_hash::BodyHash;
pub use events::AdrIngested;
pub use frontmatter::{AdrFrontmatter, Status, Tier};
