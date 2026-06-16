//! Type-selection guide for pardosa event payloads.
//!
//! This module is a rustdoc contract for agents and humans choosing field
//! types for `#[derive(GenomeSafe)]` structs and enums. It describes the
//! post-WS-1 event vocabulary: event fields must carry wire bytes and schema
//! identity; zero-width marker fields are not event fields.
//!
//! # 1. Core principle: parse, do not re-validate
//!
//! Pick the most-constrained type that still admits every legal domain value.
//! Let construction and decode reject illegal states, then keep application
//! code on already-parsed values instead of re-validating raw shapes at every
//! use site.
//!
//! The constraint belongs in the type when it is part of the domain contract:
//! bounded text uses [`NonEmptyEventString`](crate::NonEmptyEventString) or
//! [`EventString`](crate::EventString), bounded lists use
//! [`EventVec`](crate::EventVec), bounded bytes use
//! [`EventBytes`](crate::EventBytes), timestamps use
//! [`Timestamp`](crate::Timestamp), and domain records derive
//! [`GenomeSafe`](crate::GenomeSafe).
//!
//! ## Parse-failure guidance, per COM-0020
//!
//! This is guidance rather than a new event vocabulary ban: make the safe path
//! the ordinary path, so a failed parse is a convert-time rejection at the
//! serde-to-native boundary rather than a durable `Invalid` state. Prefer
//! rejecting raw serde shapes before they enter the native event tree; if an
//! existing schema carries sentinel-`Invalid` values, treat them as an
//! anti-pattern to migrate when that schema is next rebrewed, not as proof that
//! `Option<T>` or sentinel states are forbidden.
//!
//! # 2. Per-field decision tree
//!
//! - Text that must be present and non-empty: use
//!   [`NonEmptyEventString<MAX>`](crate::NonEmptyEventString).
//! - Text that is optional and where an empty string means absent: use
//!   [`Option<NonEmptyEventString<MAX>>`](core::option::Option), not
//!   [`EventString`](crate::EventString), so the empty value is
//!   unrepresentable.
//! - Text that is legitimately present and empty: use
//!   [`EventString<MAX>`](crate::EventString). This is rare; justify it at the
//!   domain boundary.
//! - A bounded list: use [`EventVec<T, MAX>`](crate::EventVec). Choose `T`
//!   with the same decision tree.
//! - Opaque or performance-sensitive bytes: use
//!   [`EventBytes<MAX>`](crate::EventBytes). Prefer typed structures when the
//!   bytes have named fields or stable variants.
//! - A closed state set: use a `#[repr(u8)]` enum with explicit
//!   discriminants, never a stringly state or a pair of booleans.
//! - A tri-state: use a domain enum, not `Option<bool>`. `gh-report` currently
//!   carries four branch-protection `Option<bool>` flags as an anti-pattern to
//!   migrate when that schema is next rebrewed. This is authoring guidance, not
//!   an eligibility ban: PGN-0013:R8 includes `Option<T>` in the field-eligible
//!   vocabulary.
//! - Numbers: use the smallest sufficient width, and make any sentinel value a
//!   deliberate domain choice rather than an accidental spare integer.
//! - Floating-point payloads: first decide whether NaN and infinities are domain
//!   values. If they are rejected, use [`OrderedF32`](crate::OrderedF32) or
//!   [`OrderedF64`](crate::OrderedF64) when the value needs total order or
//!   [`GenomeOrd`](crate::GenomeOrd), for example as a `BTreeMap` or `BTreeSet`
//!   key; use [`RealF32`](crate::RealF32) or [`RealF64`](crate::RealF64) when
//!   `PartialOrd` is enough. Both families reject NaN, ±Inf, and subnormal
//!   values at construction, decode, and validation. If the payload must durably
//!   carry NaN or ±Inf, use [`EventF32`](crate::EventF32) or
//!   [`EventF64`](crate::EventF64): their `#[repr(u8)]` tags round-trip NaN,
//!   negative infinity, finite, and positive infinity; finite values go through
//!   `OrderedF*`, and NaN sign and payload bits are not preserved.
//! - Unicode scalar values: use [`CharScalar`](crate::CharScalar) when decode
//!   should reject UTF-16 surrogates `U+D800..=U+DFFF` and values above
//!   `U+10FFFF` with structured rejection detail. Its wire bytes are identical
//!   to raw `char`.
//!
//! # 3. `MAX` selection
//!
//! `MAX` is part of the type and part of the schema hash. Changing it changes
//! the field shape even when all current values still fit.
//!
//! Pick caps from a domain source: protocol limits, product limits, storage
//! budgets, external API guarantees, or observed maximums with margin. Avoid
//! using a large cap merely to avoid choosing; it weakens decode-time
//! rejection and makes later tightening a schema change.
//!
//! ## Rejection vocabulary
//!
//! Decode-time rejection vocabulary is intentionally small and structured.
//! [`DomainError`](crate::DomainError) currently names five native-domain
//! violations: `TooLong { max, actual }`, `Empty`, `NotReal`,
//! `InvalidChar { code }`, and `InvalidUtf8`. Crossing the substrate boundary
//! maps those variants to distinct `SchemaRejectionCode` values carried by
//! `DecodeError::SchemaRejected { code }`: maximum length exceeded becomes
//! `TooLong`, an empty `NonEmpty*` value becomes `Empty`, NaN/±Inf/subnormal
//! rejection becomes `NotReal`, surrogate or out-of-range Unicode becomes
//! `InvalidChar`, and malformed UTF-8 becomes `InvalidUtf8`.
//!
//! # 4. Closed algebra under bounded field types
//!
//! `GenomeSafe` is closed under bounded field types, not under field or variant
//! counts. Structs and enums are transparent combinators: a struct is bounded
//! by its fields, and an enum is bounded by its variants. Counting fields or
//! variants measures neither property.
//!
//! A large `Art` or `DomainEvent`-style enum whose variants each carry bounded
//! fields is the blessed idiom. It is the illegal-states-unrepresentable form,
//! not a smell. If a genuine multi-large-variant layout concern remains, split
//! the model into a second dragline with its own `ENVELOPE_HASH`; do not hide
//! heap indirection inside one event schema.
//!
//! # 5. S1, S2, and S3 vocabulary
//!
//! S1 domain value types carry the domain shape directly:
//! [`NonEmptyEventString`](crate::NonEmptyEventString),
//! [`EventString`](crate::EventString), [`EventBytes`](crate::EventBytes),
//! [`EventVec`](crate::EventVec), primitive booleans and integers,
//! [`Timestamp`](crate::Timestamp), and derived structs or enums.
//!
//! The PGN-0013:R1 bounded-wrapper alphabet is exactly `EventString<MAX>`,
//! `EventBytes<MAX>`, `EventVec<T, MAX>`, and
//! `NonEmptyEventString<MAX>`. Generic ownership or indirection wrappers are
//! not alphabet members.
//!
//! S2 structural combinators alter field presence or layout without inventing
//! a new domain value:
//!
//! - [`Option<T>`](core::option::Option) is the only sanctioned absence marker.
//!   Use it when the domain truly has an absent case.
//! - Structs and enums are transparent bounded combinators when their fields
//!   are bounded field types. Prefer named fields and domain variants over
//!   raw maps, strings, vectors, or ownership wrappers.
//!
//! S3 degenerate or marker fields were removed in WS-1: `()` and
//! `core::marker::PhantomData<T>` are no longer representable as event fields.
//! Do not reach for marker types in events; if the information matters, model
//! it as an S1 value, and if it does not matter, leave it out.
//!
//! [`Box<T>`](std::boxed::Box), [`Arc<T>`](std::sync::Arc),
//! [`Rc<T>`](std::rc::Rc), and [`Cow`](std::borrow::Cow) are outside the event
//! vocabulary per PGN-0013:R1/R8. They express layout, sharing, or
//! clone-on-write mechanics rather than bounded field invariants. Store the
//! owned bounded value in the event; add runtime sharing outside the event tree
//! if a consumer needs it.
//!
//! ## Derive rejection index
//!
//! The derive crate surfaces these existing diagnostics before an invalid event
//! shape reaches runtime:
//!
//! - `EVT-001`: `#[derive(GenomeSafe)]` rejects unions.
//! - `EVT-004`: `usize` is platform-sized; choose a fixed-width unsigned
//!   integer.
//! - `EVT-005`: `isize` is platform-sized; choose a fixed-width signed integer.
//! - `EVT-006`: `#[serde(flatten)]` is rejected because flattened fields break
//!   fixed-layout serialization.
//! - `EVT-007`: `#[serde(untagged)]` is rejected because untagged enums bypass
//!   fixed discriminant-based layout.
//! - `EVT-008`: `#[serde(default)]` is rejected because every event field is
//!   present on the wire.
//! - `EVT-009`: `#[serde(tag = "...")]` is rejected; use serde's externally
//!   tagged enum shape with `#[repr(u8)]` and explicit discriminants.
//! - `EVT-010`: `#[serde(content = "...")]` is rejected because adjacently
//!   tagged enums are incompatible with fixed discriminant-based layout.
//! - `EVT-011`: `#[serde(skip_serializing_if = "...")]` is rejected because
//!   conditional field omission breaks fixed-layout serialization.
//! - `EVT-012`: raw pointers `*const T` and `*mut T` have no canonical wire
//!   representation.
//! - `EVT-013`: bare function pointers `fn(..) -> ..` carry process-local
//!   addresses with no portable wire representation.
//! - `EVT-014`: unwrapped event fields are rejected for `String`, `Vec<T>`,
//!   `Vec<u8>`, `f32`, `f64`, `Box<T>`, `Arc<T>`, `Cow<'_, T>`, `str`, `&str`,
//!   `&[u8]`, `[u8]`, `HashMap`, `HashSet`, `BTreeMap`, and `BTreeSet`; choose
//!   the corresponding bounded wrapper, float wrapper, named struct fields, or
//!   enum.
//! - Enum derives must use `#[repr(u8)]` and give every variant an explicit
//!   integer-literal discriminant; diagnostics for this rule cite
//!   `PGN-0003:R1/R4`.
//!
//! Maintenance pointer for reviewers: `crates/pardosa-derive/src/reject.rs` is
//! the single source of truth for these derive diagnostics. Check
//! `reject.rs:6-49`, `reject.rs:131-148`, `reject.rs:180-199`,
//! `reject.rs:231-248`, `reject.rs:277-396`, and `reject.rs:435-492`, plus the
//! union branch in `crates/pardosa-derive/src/lib.rs:50-55`; if this guide and
//! the derive diagnostics diverge, update the guide from the derive source.
//!
//! # 6. Zero-width fields are impossible post-WS-1
//!
//! Every event field must be load-bearing on the wire and in the schema hash.
//! A field that decodes from zero bytes or erases its type parameter can create
//! schema collisions, so the event vocabulary no longer includes `()` or
//! `core::marker::PhantomData<T>`.
//!
//! # 7. Schema-hash discipline
//!
//! Any field-shape change rebrews [`GenomeSafe::SCHEMA_HASH`](crate::GenomeSafe::SCHEMA_HASH):
//! changing `MAX`, switching `EventString` to `NonEmptyEventString`, replacing
//! `Option<bool>` with an enum, reordering fields, or changing number widths.
//! Ownership-wrapper changes are not the schema-evolution mechanism: the
//! `GenomeSafe` vocabulary admits bounded field types, and multi-schema evolution
//! uses a separate dragline rather than an in-place native migration.
//!
//! There is no general migration path for a native pardosa event schema. When
//! event shape changes, plan to re-scrape or otherwise rebuild the affected
//! native event store rather than silently reading old bytes as the new type.
//!
//! # 8. Worked example: adr-srv native tree sketch
//!
//! This is an explanatory sketch, not a generated runtime layout and not a new
//! normative rule. It illustrates how the decision tree above shapes the
//! `adr-srv` MessagePack-to-native port design.
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
//!
//! The closed-set lever is `domain: String` to `AdrDomain`: nine known prefixes
//! are a finite vocabulary, so the native tree uses a `#[repr(u8)]` enum rather
//! than a bounded string. Net shape: only `title` remains a bounded string;
//! `references` becomes `EventVec<AdrIdEvent, MAX_ADR_REFERENCES>`;
//! `body_hash` stays `[u8; 16]`, already `GenomeSafe`; and four of the seven
//! native helper types are `Copy`.
//!
//! Conversion is therefore near-trivial. Only title conversion can fail for
//! empty or over-`MAX` strings, and references conversion can fail for more than
//! `MAX_ADR_REFERENCES` entries. Domain prefix matching, fixed-width
//! primitives, by-variant `#[repr(u8)]` enums, dates, and body-hash copies are
//! infallible once the serde layer has admitted the source value.
//!
//! `#[derive(GenomeSafe)]` emits the sealed trait stack and codec bundle:
//! `Sealed`, `EventSafe`, `GenomeSafe`, `Encode`, and `Decode`. It does not emit
//! the domain-level `Validate` implementation or the schema-source registry
//! hook; authors hand-write `impl Validate` and `impl HasEventSchemaSource` for
//! the native aggregate.
