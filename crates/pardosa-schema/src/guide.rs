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
//! # 4. S1, S2, and S3 vocabulary
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
//! - [`Box<T>`](std::boxed::Box) is layout-only. It is wire- and
//!   hash-transparent, so `Option<Box<T>>` has the same schema identity as
//!   `Option<T>`. Choose it only to manage Rust layout, such as a large enum
//!   variant; never use it as a modeling signal.
//!
//! S3 degenerate or marker fields were removed in WS-1: `()` and
//! `core::marker::PhantomData<T>` are no longer representable as event fields.
//! Do not reach for marker types in events; if the information matters, model
//! it as an S1 value, and if it does not matter, leave it out.
//!
//! Current asymmetry to document, not fix here: [`Box<T>`](std::boxed::Box) is
//! `Sealed` + `EventSafe` + [`GenomeSafe`](crate::GenomeSafe);
//! [`Arc<T>`](std::sync::Arc) is `Sealed` + `EventSafe` but not
//! [`GenomeSafe`](crate::GenomeSafe); [`Cow`](std::borrow::Cow) and
//! [`Rc`](std::rc::Rc) are not event types. No current ADR explains this
//! asymmetry.
//!
//! # 5. Zero-width fields are impossible post-WS-1
//!
//! Every event field must be load-bearing on the wire and in the schema hash.
//! A field that decodes from zero bytes or erases its type parameter can create
//! schema collisions, so the event vocabulary no longer includes `()` or
//! `core::marker::PhantomData<T>`.
//!
//! # 6. Schema-hash discipline
//!
//! Any field-shape change rebrews [`GenomeSafe::SCHEMA_HASH`](crate::GenomeSafe::SCHEMA_HASH):
//! changing `MAX`, switching `EventString` to `NonEmptyEventString`, replacing
//! `Option<bool>` with an enum, reordering fields, or changing number widths.
//! Adding or removing [`Box`] alone is layout-only and hash
//! transparent; use it only for Rust layout pressure, not as a schema tool.
//!
//! There is no general migration path for a native pardosa event schema. When
//! event shape changes, plan to re-scrape or otherwise rebuild the affected
//! native event store rather than silently reading old bytes as the new type.
