//! Type-selection guide for pardosa event payloads.
//!
//! Field-type decisions for `#[derive(GenomeSafe)]` structs/enums, applying
//! `PGN-0013` (bounded-wrapper alphabet), `PGN-0003` (`SCHEMA_HASH`),
//! `PGN-0006` (rejections), `COM-0020` (reject invalid shapes at the serde
//! boundary) — does not restate those rules.
//!
//! # Decision tree
//!
//! - Required non-empty text: [`NonEmptyEventString<MAX>`](crate::NonEmptyEventString).
//! - Optional, empty means absent: `Option<NonEmptyEventString<MAX>>`.
//! - Legitimately empty (rare): [`EventString<MAX>`](crate::EventString).
//! - Bounded list: [`EventVec<T, MAX>`](crate::EventVec).
//! - Opaque bytes: [`EventBytes<MAX>`](crate::EventBytes).
//! - Closed state set: `#[repr(u8)]` enum, explicit discriminants.
//! - Tri-state: domain enum, not `Option<bool>`.
//! - Floats: `OrderedF32`/`RealF32` reject NaN/±Inf/subnormal;
//!   [`EventF32`](crate::EventF32) round-trips them when required.
//! - Unicode scalar: [`CharScalar`](crate::CharScalar) rejects surrogates.
//!
//! `MAX` is part of the type and schema hash (`PGN-0013:R6`); pick it from a
//! domain source. `GenomeSafe` closes under bounded field types, not
//! field/variant counts. Derive diagnostics (`EVT-001..EVT-014`) source from
//! `pardosa-derive/src/reject.rs`.
//!
//! # Worked example: adr-srv native tree
//!
//! Illustrative: `domain: String` becomes a `#[repr(u8)] AdrDomain` enum
//! over nine known prefixes.
//!
//! ```text
//! serde AdrId
//!   domain: String, one of AFM CHE PAR GEN SEC COM GND RST FLO
//!   number: u16
//!
//! native AdrIdEvent
//!   domain: AdrDomain, repr(u8) closed set with explicit discriminants
//!   number: u16
//!
//! repr(u8) enum AdrDomain
//!   Afm = 0
//!   Che = 1
//!   Par = 2
//!   Gen = 3
//!   Sec = 4
//!   Com = 5
//!   Gnd = 6
//!   Rst = 7
//!   Flo = 8
//!
//! native AdrFrontmatterEvent
//!   title: NonEmptyEventString<MAX_ADR_TITLE>
//!   date: AdrDateEvent
//!   last_reviewed: AdrDateEvent
//!   tier: AdrTier
//!   status: AdrStatus
//!
//! native DomainEvent::AdrIngested
//!   id: AdrIdEvent
//!   frontmatter: AdrFrontmatterEvent
//!   body_hash: [u8; 16]
//!   references: EventVec<AdrIdEvent, MAX_ADR_REFERENCES>
//! ```
