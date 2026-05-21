//! Domain types for adr-srv.
//!
//! All event-payload types derive `serde::{Serialize, Deserialize}`.
//! On-disk encoding is msgpack via `cherry_pit_gateway::MsgpackFileStore`
//! (rmp-serde). Wire-shape decisions are documented at each type;
//! reorder / non-tail field insertion remains a wire break per
//! CHE-0022:R5, but the canonical-bytes invariant of CHE-0064:R2 is
//! relaxed — msgpack is self-describing, so additive evolution is
//! safe without re-baselining a fingerprint.

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
