//! Native `GenomeSafe` event tree for the `AdrIngested` scrape record
//! (CHE-0098 N-R2/N-R3).
//!
//! The serde [`AdrIngested`](crate::domain::events::AdrIngested) struct
//! in `events.rs` is the scrape-time DTO; it is NOT the durable pardosa
//! payload (CHE-0098 R2). This module defines the schema-hashed native
//! event tree that IS the durable payload, plus a total, field-
//! preserving mapping in both directions (CHE-0098 R3, N-R3).

use pardosa::prelude::*;

use crate::domain::adr_date::AdrDate;
use crate::domain::adr_id::{AdrId, AdrIdError};
use crate::domain::body_hash::BodyHash;
use crate::domain::events::AdrIngested;
use crate::domain::frontmatter::{AdrFrontmatter, Status, Tier};

pub mod limits {
    pub const MAX_ADR_TITLE: usize = 512;
    pub const MAX_ADR_REFERENCES: usize = 256;
}

use limits::{MAX_ADR_REFERENCES, MAX_ADR_TITLE};

/// Closed set of ADR domain prefixes (mirrors
/// [`crate::domain::adr_id::KNOWN_DOMAINS`]), schema-hashed as a
/// `#[repr(u8)]` discriminant per `PGN-0013`. Appended-only: removing a
/// variant breaks replay of events emitted under it (CHE-0022:R5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AdrDomain {
    Afm = 0,
    Che = 1,
    Par = 2,
    Gen = 3,
    Sec = 4,
    Com = 5,
    Gnd = 6,
    Rst = 7,
    Flo = 8,
}

impl PardosaType for AdrDomain {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::U8
    }

    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        buf.push(*self as u8);
        Ok(())
    }

    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        if buf.is_empty() {
            return Err(DecodeError::TruncatedPayload {
                expected: 1,
                available: 0,
            });
        }
        let val = match buf[0] {
            0 => Self::Afm,
            1 => Self::Che,
            2 => Self::Par,
            3 => Self::Gen,
            4 => Self::Sec,
            5 => Self::Com,
            6 => Self::Gnd,
            7 => Self::Rst,
            8 => Self::Flo,
            other => {
                return Err(DecodeError::UnknownVariantDiscriminant {
                    discriminant: u32::from(other),
                });
            }
        };
        Ok((val, 1))
    }
}

/// Rejects mapping a scrape-side domain prefix that is not one of the
/// nine schema-hashed [`AdrDomain`] variants onto the native tree.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NativeMapError {
    #[error("adr domain prefix {0:?} has no native AdrDomain variant")]
    UnknownDomain(String),
    #[error("native domain reconstruction: {0}")]
    Reconstruct(#[from] AdrIdError),
}

impl AdrDomain {
    fn from_prefix(prefix: &str) -> Result<Self, NativeMapError> {
        match prefix {
            "AFM" => Ok(Self::Afm),
            "CHE" => Ok(Self::Che),
            "PAR" => Ok(Self::Par),
            "GEN" => Ok(Self::Gen),
            "SEC" => Ok(Self::Sec),
            "COM" => Ok(Self::Com),
            "GND" => Ok(Self::Gnd),
            "RST" => Ok(Self::Rst),
            "FLO" => Ok(Self::Flo),
            other => Err(NativeMapError::UnknownDomain(other.to_string())),
        }
    }

    fn as_prefix(self) -> &'static str {
        match self {
            Self::Afm => "AFM",
            Self::Che => "CHE",
            Self::Par => "PAR",
            Self::Gen => "GEN",
            Self::Sec => "SEC",
            Self::Com => "COM",
            Self::Gnd => "GND",
            Self::Rst => "RST",
            Self::Flo => "FLO",
        }
    }
}

/// Native counterpart of [`AdrId`]: `domain` closes over
/// [`AdrDomain`]'s schema-hashed discriminant instead of a raw
/// `String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdrIdEvent {
    pub domain: AdrDomain,
    pub number: u16,
}

impl PardosaType for AdrIdEvent {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::Struct {
            name: "AdrIdEvent".to_string(),
            fields: vec![
                FieldDescriptor {
                    name: "domain".to_string(),
                    node: AdrDomain::descriptor_node(),
                },
                FieldDescriptor {
                    name: "number".to_string(),
                    node: u16::descriptor_node(),
                },
            ],
        }
    }

    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.domain.encode_type(buf)?;
        self.number.encode_type(buf)?;
        Ok(())
    }

    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut cursor = 0;
        let (domain, c) = AdrDomain::decode_type(&buf[cursor..])?;
        cursor += c;
        let (number, c) = u16::decode_type(&buf[cursor..])?;
        cursor += c;
        Ok((Self { domain, number }, cursor))
    }
}

impl AdrIdEvent {
    fn from_domain(id: &AdrId) -> Result<Self, NativeMapError> {
        Ok(Self {
            domain: AdrDomain::from_prefix(id.domain())?,
            number: id.number(),
        })
    }

    fn to_domain(self) -> Result<AdrId, NativeMapError> {
        Ok(AdrId::new(self.domain.as_prefix(), self.number)?)
    }
}

/// Native counterpart of [`AdrDate`]: same `(year, month, day)` wire
/// shape, promoted to a `PardosaType` struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdrDateEvent {
    pub year: i16,
    pub month: u8,
    pub day: u8,
}

impl PardosaType for AdrDateEvent {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::Struct {
            name: "AdrDateEvent".to_string(),
            fields: vec![
                FieldDescriptor {
                    name: "year".to_string(),
                    node: i16::descriptor_node(),
                },
                FieldDescriptor {
                    name: "month".to_string(),
                    node: u8::descriptor_node(),
                },
                FieldDescriptor {
                    name: "day".to_string(),
                    node: u8::descriptor_node(),
                },
            ],
        }
    }

    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.year.encode_type(buf)?;
        self.month.encode_type(buf)?;
        self.day.encode_type(buf)?;
        Ok(())
    }

    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut cursor = 0;
        let (year, c) = i16::decode_type(&buf[cursor..])?;
        cursor += c;
        let (month, c) = u8::decode_type(&buf[cursor..])?;
        cursor += c;
        let (day, c) = u8::decode_type(&buf[cursor..])?;
        cursor += c;
        Ok((Self { year, month, day }, cursor))
    }
}

impl From<AdrDate> for AdrDateEvent {
    fn from(d: AdrDate) -> Self {
        Self {
            year: d.year(),
            month: d.month(),
            day: d.day(),
        }
    }
}

impl AdrDateEvent {
    fn to_domain(self) -> AdrDate {
        AdrDate::new(self.year, self.month, self.day)
            .expect("AdrDateEvent was constructed from a valid AdrDate")
    }
}

/// Native counterpart of [`Tier`]. Variant order and discriminants
/// mirror the scrape-side enum; appended-only per CHE-0022:R5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AdrTier {
    S = 0,
    A = 1,
    B = 2,
    C = 3,
    D = 4,
}

impl PardosaType for AdrTier {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::U8
    }

    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        buf.push(*self as u8);
        Ok(())
    }

    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        if buf.is_empty() {
            return Err(DecodeError::TruncatedPayload {
                expected: 1,
                available: 0,
            });
        }
        let val = match buf[0] {
            0 => Self::S,
            1 => Self::A,
            2 => Self::B,
            3 => Self::C,
            4 => Self::D,
            other => {
                return Err(DecodeError::UnknownVariantDiscriminant {
                    discriminant: u32::from(other),
                });
            }
        };
        Ok((val, 1))
    }
}

impl From<Tier> for AdrTier {
    fn from(t: Tier) -> Self {
        match t {
            Tier::S => Self::S,
            Tier::A => Self::A,
            Tier::B => Self::B,
            Tier::C => Self::C,
            Tier::D => Self::D,
        }
    }
}

impl From<AdrTier> for Tier {
    fn from(t: AdrTier) -> Self {
        match t {
            AdrTier::S => Self::S,
            AdrTier::A => Self::A,
            AdrTier::B => Self::B,
            AdrTier::C => Self::C,
            AdrTier::D => Self::D,
        }
    }
}

/// Native counterpart of [`Status`]. Mirrors all seven scrape-side
/// variants (`SupersededBy`/`Invalid` payload data is not carried by
/// either side today — see `frontmatter.rs`); appended-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AdrStatus {
    Draft = 0,
    Proposed = 1,
    Accepted = 2,
    Rejected = 3,
    Deprecated = 4,
    Superseded = 5,
    Invalid = 6,
}

impl PardosaType for AdrStatus {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::U8
    }

    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        buf.push(*self as u8);
        Ok(())
    }

    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        if buf.is_empty() {
            return Err(DecodeError::TruncatedPayload {
                expected: 1,
                available: 0,
            });
        }
        let val = match buf[0] {
            0 => Self::Draft,
            1 => Self::Proposed,
            2 => Self::Accepted,
            3 => Self::Rejected,
            4 => Self::Deprecated,
            5 => Self::Superseded,
            6 => Self::Invalid,
            other => {
                return Err(DecodeError::UnknownVariantDiscriminant {
                    discriminant: u32::from(other),
                });
            }
        };
        Ok((val, 1))
    }
}

impl From<Status> for AdrStatus {
    fn from(s: Status) -> Self {
        match s {
            Status::Draft => Self::Draft,
            Status::Proposed => Self::Proposed,
            Status::Accepted => Self::Accepted,
            Status::Rejected => Self::Rejected,
            Status::Deprecated => Self::Deprecated,
            Status::Superseded => Self::Superseded,
            Status::Invalid => Self::Invalid,
        }
    }
}

impl From<AdrStatus> for Status {
    fn from(s: AdrStatus) -> Self {
        match s {
            AdrStatus::Draft => Self::Draft,
            AdrStatus::Proposed => Self::Proposed,
            AdrStatus::Accepted => Self::Accepted,
            AdrStatus::Rejected => Self::Rejected,
            AdrStatus::Deprecated => Self::Deprecated,
            AdrStatus::Superseded => Self::Superseded,
            AdrStatus::Invalid => Self::Invalid,
        }
    }
}

/// Native counterpart of [`AdrFrontmatter`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdrFrontmatterEvent {
    pub title: NonEmptyEventString<MAX_ADR_TITLE>,
    pub date: AdrDateEvent,
    pub last_reviewed: AdrDateEvent,
    pub tier: AdrTier,
    pub status: AdrStatus,
}

impl PardosaType for AdrFrontmatterEvent {
    fn descriptor_node() -> DescriptorNode {
        DescriptorNode::Struct {
            name: "AdrFrontmatterEvent".to_string(),
            fields: vec![
                FieldDescriptor {
                    name: "title".to_string(),
                    node: NonEmptyEventString::<MAX_ADR_TITLE>::descriptor_node(),
                },
                FieldDescriptor {
                    name: "date".to_string(),
                    node: AdrDateEvent::descriptor_node(),
                },
                FieldDescriptor {
                    name: "last_reviewed".to_string(),
                    node: AdrDateEvent::descriptor_node(),
                },
                FieldDescriptor {
                    name: "tier".to_string(),
                    node: AdrTier::descriptor_node(),
                },
                FieldDescriptor {
                    name: "status".to_string(),
                    node: AdrStatus::descriptor_node(),
                },
            ],
        }
    }

    fn encode_type(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.title.encode_type(buf)?;
        self.date.encode_type(buf)?;
        self.last_reviewed.encode_type(buf)?;
        self.tier.encode_type(buf)?;
        self.status.encode_type(buf)?;
        Ok(())
    }

    fn decode_type(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let mut cursor = 0;
        let (title, c) = NonEmptyEventString::<MAX_ADR_TITLE>::decode_type(&buf[cursor..])?;
        cursor += c;
        let (date, c) = AdrDateEvent::decode_type(&buf[cursor..])?;
        cursor += c;
        let (last_reviewed, c) = AdrDateEvent::decode_type(&buf[cursor..])?;
        cursor += c;
        let (tier, c) = AdrTier::decode_type(&buf[cursor..])?;
        cursor += c;
        let (status, c) = AdrStatus::decode_type(&buf[cursor..])?;
        cursor += c;
        Ok((
            Self {
                title,
                date,
                last_reviewed,
                tier,
                status,
            },
            cursor,
        ))
    }
}

/// Failure converting a scrape-side [`AdrIngested`] into
/// [`AdrIngestedEvent`] (N-R3 boundary mapping).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NativeConversionError {
    #[error("field {field}: {source}")]
    Map {
        field: &'static str,
        #[source]
        source: NativeMapError,
    },
    #[error("field {field}: title must be non-empty")]
    EmptyTitle { field: &'static str },
    #[error("field {field}: value exceeds bounded length")]
    TooLong { field: &'static str },
    #[error("field {field}: collection exceeds bounded capacity")]
    TooMany { field: &'static str },
}

/// Native, schema-hashed durable payload for the `AdrIngested` scrape
/// event (CHE-0098 R2). Field set is total over [`AdrIngested`]'s
/// vocabulary (CHE-0098 R3): `id`, `frontmatter`, `body_hash`,
/// `references` all have a native home.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdrIngestedEvent {
    pub id: AdrIdEvent,
    pub frontmatter: AdrFrontmatterEvent,
    pub body_hash: [u8; 16],
    pub references: EventVec<AdrIdEvent, MAX_ADR_REFERENCES>,
}

impl AdrIngestedEvent {
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        "AdrIngested"
    }

    /// Domain key for the native pardosa store port (CHE-0098 N-R4):
    /// one fiber per ADR-file aggregate, keyed by the same
    /// `DOMAIN-NNNN` string [`AdrId::to_string`] produces.
    #[must_use]
    pub fn domain_key(&self) -> String {
        format!("{}-{:04}", self.id.domain.as_prefix(), self.id.number)
    }
}

impl PardosaSchema for AdrIngestedEvent {
    fn schema_version() -> u32 {
        1
    }

    fn schema_descriptor() -> DescriptorNode {
        DescriptorNode::Struct {
            name: "AdrIngestedEvent".to_string(),
            fields: vec![
                FieldDescriptor {
                    name: "id".to_string(),
                    node: AdrIdEvent::descriptor_node(),
                },
                FieldDescriptor {
                    name: "frontmatter".to_string(),
                    node: AdrFrontmatterEvent::descriptor_node(),
                },
                FieldDescriptor {
                    name: "body_hash".to_string(),
                    node: DescriptorNode::EventBytes { max_bytes: 16 },
                },
                FieldDescriptor {
                    name: "references".to_string(),
                    node: DescriptorNode::EventVec {
                        inner: Box::new(AdrIdEvent::descriptor_node()),
                        max_items: u32::try_from(MAX_ADR_REFERENCES).expect("fits u32"),
                    },
                },
            ],
        }
    }

    fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<(), EncodeError> {
        self.id.encode_type(buf)?;
        self.frontmatter.encode_type(buf)?;
        buf.extend_from_slice(&self.body_hash);
        self.references.encode_type(buf)?;
        Ok(())
    }

    fn decode_payload(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut cursor = 0;
        let (id, c) = AdrIdEvent::decode_type(&buf[cursor..])?;
        cursor += c;
        let (frontmatter, c) = AdrFrontmatterEvent::decode_type(&buf[cursor..])?;
        cursor += c;
        if buf.len() < cursor + 16 {
            return Err(DecodeError::TruncatedPayload {
                expected: cursor + 16,
                available: buf.len(),
            });
        }
        let mut body_hash = [0u8; 16];
        body_hash.copy_from_slice(&buf[cursor..cursor + 16]);
        cursor += 16;
        let (references, _) = EventVec::decode_type(&buf[cursor..])?;
        Ok(Self {
            id,
            frontmatter,
            body_hash,
            references,
        })
    }
}

impl TryFrom<&AdrIngested> for AdrIngestedEvent {
    type Error = NativeConversionError;

    fn try_from(domain: &AdrIngested) -> Result<Self, Self::Error> {
        let id =
            AdrIdEvent::from_domain(&domain.id).map_err(|source| NativeConversionError::Map {
                field: "id",
                source,
            })?;
        let title =
            NonEmptyEventString::new(domain.frontmatter.title.as_str()).map_err(
                |err| match err {
                    DecodeError::LengthExceeded { .. } => {
                        NativeConversionError::TooLong { field: "title" }
                    }
                    _ => NativeConversionError::EmptyTitle { field: "title" },
                },
            )?;
        let frontmatter = AdrFrontmatterEvent {
            title,
            date: domain.frontmatter.date.into(),
            last_reviewed: domain.frontmatter.last_reviewed.into(),
            tier: domain.frontmatter.tier.into(),
            status: domain.frontmatter.status.into(),
        };
        let references = domain
            .references
            .iter()
            .map(|r| {
                AdrIdEvent::from_domain(r).map_err(|source| NativeConversionError::Map {
                    field: "references",
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let references = EventVec::new(references).map_err(|_| NativeConversionError::TooMany {
            field: "references",
        })?;
        Ok(Self {
            id,
            frontmatter,
            body_hash: *domain.body_hash.as_bytes(),
            references,
        })
    }
}

impl TryFrom<&AdrIngestedEvent> for AdrIngested {
    type Error = NativeMapError;

    fn try_from(native: &AdrIngestedEvent) -> Result<Self, Self::Error> {
        let id = native.id.to_domain()?;
        let frontmatter = AdrFrontmatter {
            title: native.frontmatter.title.as_str().to_string(),
            date: native.frontmatter.date.to_domain(),
            last_reviewed: native.frontmatter.last_reviewed.to_domain(),
            tier: native.frontmatter.tier.into(),
            status: native.frontmatter.status.into(),
        };
        let references = native
            .references
            .iter()
            .map(|r| r.to_domain())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id,
            frontmatter,
            body_hash: BodyHash::from(native.body_hash),
            references,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frontmatter() -> AdrFrontmatter {
        AdrFrontmatter {
            title: "Native Pardosa Store Port".to_string(),
            date: AdrDate::new(2026, 7, 23).expect("valid date"),
            last_reviewed: AdrDate::new(2026, 7, 23).expect("valid date"),
            tier: Tier::B,
            status: Status::Accepted,
        }
    }

    fn ingested() -> AdrIngested {
        AdrIngested {
            id: AdrId::new("CHE", 98).expect("valid id"),
            frontmatter: frontmatter(),
            body_hash: BodyHash::compute(b"adr body bytes"),
            references: vec![
                AdrId::new("PAR", 8).expect("valid id"),
                AdrId::new("PAR", 3).expect("valid id"),
            ],
        }
    }

    /// N-R3 evidence: every field on [`AdrIngested`] round-trips
    /// through [`AdrIngestedEvent`] unchanged.
    #[test]
    fn native_round_trip_preserves_every_adr_ingested_field() {
        let domain = ingested();
        let native = AdrIngestedEvent::try_from(&domain).expect("total mapping");
        let back = AdrIngested::try_from(&native).expect("total mapping back");

        assert_eq!(back.id, domain.id, "id field lost or altered");
        assert_eq!(
            back.frontmatter, domain.frontmatter,
            "frontmatter field lost or altered"
        );
        assert_eq!(
            back.body_hash, domain.body_hash,
            "body_hash field lost or altered"
        );
        assert_eq!(
            back.references, domain.references,
            "references field lost or altered"
        );
        assert_eq!(back, domain, "full struct round-trip must be exact");
    }

    #[test]
    fn native_event_wire_round_trips() {
        let domain = ingested();
        let native = AdrIngestedEvent::try_from(&domain).expect("total mapping");
        let mut wire = Vec::new();
        native
            .encode_payload(&mut wire)
            .expect("encode native event");
        let decoded = AdrIngestedEvent::decode_payload(&wire).expect("decode native event");
        assert_eq!(decoded, native);
        assert_eq!(decoded.event_type(), "AdrIngested");
    }

    #[test]
    fn native_event_handles_empty_references() {
        let mut domain = ingested();
        domain.references = Vec::new();
        let native = AdrIngestedEvent::try_from(&domain).expect("total mapping");
        let back = AdrIngested::try_from(&native).expect("total mapping back");
        assert_eq!(back.references, domain.references);
    }

    #[test]
    fn schema_hash_is_stable_across_reads() {
        let first = AdrIngestedEvent::schema_identity();
        let second = AdrIngestedEvent::schema_identity();
        assert_eq!(first, second);
    }
}
