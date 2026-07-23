//! Domain types for adr-srv.
//!
//! All event-payload types derive `serde::{Serialize, Deserialize}`.
//! On-disk encoding is via [`NativeAdrStore`](crate::store::NativeAdrStore)
//! (CHE-0098 R8/R9 hard cut off the transitional msgpack gateway
//! store). Wire-shape decisions are documented at each type; reorder
//! / non-tail field insertion remains a wire break per CHE-0022:R5.

pub mod adr_date;
pub mod adr_id;
pub mod aggregate;
pub mod body_hash;
pub mod events;
pub mod frontmatter;
pub mod native_event;

pub use adr_date::{AdrDate, AdrDateError};
pub use adr_id::{AdrId, AdrIdError, KNOWN_DOMAINS};
pub use aggregate::AdrDocument;
pub use body_hash::BodyHash;
pub use events::AdrIngested;
pub use frontmatter::{AdrFrontmatter, Status, Tier};
pub use native_event::{
    AdrDomain, AdrFrontmatterEvent, AdrIdEvent, AdrIngestedEvent, AdrStatus, AdrTier,
    NativeConversionError, NativeMapError,
};
