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
//!   migrate when that schema is next rebrewed.
//! - Numbers: use the smallest sufficient width, and make any sentinel value a
//!   deliberate domain choice rather than an accidental spare integer.
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
