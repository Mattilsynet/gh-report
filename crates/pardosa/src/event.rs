use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::PardosaError;

/// Raw deserialization helper for `Index`. Routes through `Index::new`
/// to enforce the `u64::MAX` sentinel guard on the deser path — without
/// this wrapper, a derived `Deserialize` bypasses `Index::new`'s assert
/// and lets a handcrafted serialized form inject `Index::NONE` into
/// positions that semantically forbid it (notably `Fiber.current`).
/// Positions that legitimately carry `NONE` (e.g. `Event.precursor`) opt
/// in via `index_with_sentinel` below.
#[derive(Deserialize)]
pub(crate) struct IndexRaw(u64);

impl TryFrom<IndexRaw> for Index {
    type Error = String;

    fn try_from(raw: IndexRaw) -> Result<Self, Self::Error> {
        if raw.0 == u64::MAX {
            return Err("u64::MAX is reserved for Index::NONE — not valid in this position".into());
        }
        Ok(Index(raw.0))
    }
}

/// Serde adapter for `Index` fields that legitimately permit the
/// `Index::NONE` sentinel (e.g. `Event.precursor` marking a genesis
/// event). Use as `#[serde(with = "index_with_sentinel")]`.
pub(crate) mod index_with_sentinel {
    use super::Index;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "non-idiomatic Rust required: serde `#[serde(with = \"...\")]` mandates `serialize<S>(value: &T, s: S)` signature for the serialize callback regardless of `T`'s size or `Copy`-ness"
    )]
    pub fn serialize<S: Serializer>(index: &Index, s: S) -> Result<S::Ok, S::Error> {
        index.0.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Index, D::Error> {
        // Accept any u64 including u64::MAX; sentinel is meaningful here.
        u64::deserialize(d).map(Index)
    }
}

/// Position in the append-only line.
///
/// GENOME LAYOUT: single `u64` field. Do not add fields or reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "IndexRaw")]
pub struct Index(u64);

impl Index {
    pub const ZERO: Index = Index(0);

    /// Sentinel value representing "no index" (e.g., first event has no precursor).
    /// `u64::MAX` is permanently reserved — a line with that many events would
    /// require ~147 exabytes of storage.
    pub const NONE: Index = Index(u64::MAX);

    /// Create a new index. Panics if `v == u64::MAX` (reserved for `NONE`).
    /// Use `Index::NONE` to construct the sentinel explicitly.
    ///
    /// # Panics
    ///
    /// Panics if `v == u64::MAX`. This is a programmer-error guard, not a
    /// runtime-input check. `Index` values are assigned internally by the
    /// Dragline — callers never construct indices from external input.
    #[must_use]
    pub fn new(v: u64) -> Self {
        assert!(
            v != u64::MAX,
            "u64::MAX is reserved for Index::NONE — use Index::NONE directly"
        );
        Index(v)
    }

    /// Create an index without validating against the sentinel.
    /// Only for deserialization paths where the value has already been validated.
    #[cfg(test)]
    pub(crate) fn new_unchecked(v: u64) -> Self {
        Index(v)
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }

    /// Convert to `usize` for use as a `Vec` index.
    ///
    /// # Panics
    ///
    /// Panics on 32-bit targets if the value exceeds `usize::MAX`.
    /// In practice this cannot occur because `Index` values originate
    /// from `Vec::len()`, which is bounded by `usize`.
    #[must_use]
    pub fn as_usize(self) -> usize {
        usize::try_from(self.0).expect("Index value exceeds usize::MAX")
    }

    /// Returns `true` if this is the `NONE` sentinel.
    #[must_use]
    pub fn is_none(self) -> bool {
        self.0 == u64::MAX
    }

    /// Returns `true` if this is a valid position (not `NONE`).
    #[must_use]
    pub fn is_some(self) -> bool {
        self.0 != u64::MAX
    }

    /// Returns the next index, or `IndexOverflow` if at `u64::MAX - 1`
    /// (the last valid position before the sentinel).
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::IndexOverflow`] when `self` is at or beyond
    /// the last valid position (`u64::MAX - 1`).
    pub fn checked_next(self) -> Result<Index, PardosaError> {
        if self.0 >= u64::MAX - 1 {
            return Err(PardosaError::IndexOverflow);
        }
        Ok(Index(self.0 + 1))
    }
}

impl fmt::Display for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            write!(f, "NONE")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// Unique identifier for a domain entity / fiber.
///
/// GENOME LAYOUT: single `u64` field. Do not add fields or reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainId(u64);

impl DomainId {
    #[must_use]
    pub fn new(v: u64) -> Self {
        DomainId(v)
    }

    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }

    /// # Errors
    ///
    /// Returns [`PardosaError::DomainIdOverflow`] when `self` is `u64::MAX`.
    pub fn checked_next(self) -> Result<DomainId, PardosaError> {
        self.0
            .checked_add(1)
            .map(DomainId)
            .ok_or(PardosaError::DomainIdOverflow)
    }
}

impl fmt::Display for DomainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// An immutable event in the append-only line.
///
/// GENOME LAYOUT: fields are serialized in declaration order.
/// Changing field order is a breaking change — `schema_id` will change.
///
/// - `event_id`: globally monotonic across stream generations.
/// - `timestamp`: Unix epoch in milliseconds.
/// - `detached`: `true` when this event records a soft-delete (Detach operation).
/// - `precursor`: Index of the previous event in the same fiber (`Index::NONE` for the first event).
/// - `precursor_hash`: 32-byte BLAKE3 of the predecessor's canonical bytes (PAR-0021 R1).
///   Genesis events use `[0u8; 32]` since they have no predecessor. F2a ships
///   this field as plumbing-only — callers pass `[0u8; 32]` until F2b wires the
///   real BLAKE3 computation. Positioned adjacent to `precursor` because the
///   two are conceptually paired (hash of the event `precursor` points at).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(
    clippy::struct_field_names,
    reason = "non-idiomatic Rust required: field names (`event_id`, `domain_id`) are part of the GENOME wire layout per PAR-0003:R1; renaming for clippy taste would alter the serialized field tags and break replay across generations"
)]
pub struct Event<T> {
    event_id: u64,
    timestamp: i64,
    domain_id: DomainId,
    detached: bool,
    // Precursor legitimately carries Index::NONE for genesis events.
    // Bypass the sentinel-rejecting IndexRaw guard for this field only.
    #[serde(with = "index_with_sentinel")]
    precursor: Index,
    precursor_hash: [u8; 32],
    domain_event: T,
}

impl<T> Event<T> {
    #[must_use]
    pub fn new(
        event_id: u64,
        timestamp: i64,
        domain_id: DomainId,
        detached: bool,
        precursor: Index,
        precursor_hash: [u8; 32],
        domain_event: T,
    ) -> Self {
        Event {
            event_id,
            timestamp,
            domain_id,
            detached,
            precursor,
            precursor_hash,
            domain_event,
        }
    }

    #[must_use]
    pub fn event_id(&self) -> u64 {
        self.event_id
    }

    #[must_use]
    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    #[must_use]
    pub fn domain_id(&self) -> DomainId {
        self.domain_id
    }

    #[must_use]
    pub fn detached(&self) -> bool {
        self.detached
    }

    #[must_use]
    pub fn precursor(&self) -> Index {
        self.precursor
    }

    #[must_use]
    pub fn precursor_hash(&self) -> [u8; 32] {
        self.precursor_hash
    }

    #[must_use]
    pub fn domain_event(&self) -> &T {
        &self.domain_event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Index::NONE sentinel ---

    #[test]
    fn index_none_is_none() {
        assert!(Index::NONE.is_none());
        assert!(!Index::NONE.is_some());
    }

    #[test]
    fn index_zero_is_not_none() {
        assert!(!Index::ZERO.is_none());
        assert!(Index::ZERO.is_some());
    }

    #[test]
    fn index_none_display() {
        assert_eq!(format!("{}", Index::NONE), "NONE");
    }

    #[test]
    fn index_valid_display() {
        assert_eq!(format!("{}", Index::new(42)), "42");
    }

    #[test]
    #[should_panic(expected = "u64::MAX is reserved for Index::NONE")]
    fn index_new_rejects_sentinel() {
        let _ = Index::new(u64::MAX);
    }

    #[test]
    fn index_new_accepts_max_minus_one() {
        let i = Index::new(u64::MAX - 1);
        assert_eq!(i.value(), u64::MAX - 1);
        assert!(i.is_some());
    }

    #[test]
    fn index_unchecked_allows_sentinel() {
        let i = Index::new_unchecked(u64::MAX);
        assert!(i.is_none());
    }

    // --- Index::checked_next ---

    #[test]
    fn index_checked_next() {
        let i = Index::new(0);
        assert_eq!(i.checked_next().unwrap().value(), 1);
    }

    #[test]
    fn index_checked_next_at_max_minus_2() {
        let i = Index::new(u64::MAX - 2);
        let next = i.checked_next().unwrap();
        assert_eq!(next.value(), u64::MAX - 1);
    }

    #[test]
    fn index_checked_next_at_max_minus_1_overflows() {
        let i = Index::new(u64::MAX - 1);
        assert!(i.checked_next().is_err());
    }

    #[test]
    fn index_none_checked_next_overflows() {
        assert!(Index::NONE.checked_next().is_err());
    }

    // --- Index roundtrip ---

    #[test]
    fn index_roundtrip() {
        let i = Index::new(42);
        assert_eq!(i.value(), 42);
    }

    #[test]
    fn index_serde_roundtrip() {
        let i = Index::new(42);
        let json = serde_json::to_string(&i).unwrap();
        let back: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(back, i);
    }

    #[test]
    fn index_none_serde_via_event_precursor() {
        // Index::NONE cannot deserialize bare (would bypass the sentinel
        // guard); it round-trips only inside positions that opt in via
        // index_with_sentinel — e.g. Event.precursor for genesis events.
        let event = Event::new(
            1,
            1_700_000_000_000,
            DomainId::new(1),
            false,
            Index::NONE,
            [0u8; 32],
            "genesis".to_string(),
        );
        let json = serde_json::to_string(&event).unwrap();
        let back: Event<String> = serde_json::from_str(&json).unwrap();
        assert!(back.precursor().is_none());
    }

    #[test]
    fn index_bare_deserialize_rejects_sentinel() {
        let result: Result<Index, _> = serde_json::from_str("18446744073709551615");
        assert!(
            result.is_err(),
            "bare Index deserialize must reject u64::MAX sentinel"
        );
    }

    // --- DomainId ---

    #[test]
    fn domain_id_checked_next() {
        let d = DomainId::new(0);
        assert_eq!(d.checked_next().unwrap().value(), 1);
    }

    #[test]
    fn domain_id_overflow() {
        let d = DomainId::new(u64::MAX);
        assert!(d.checked_next().is_err());
    }

    // --- Event<T> ---

    #[test]
    fn event_constructor_and_accessors() {
        let event = Event::new(
            1,
            1_700_000_000_000,
            DomainId::new(5),
            false,
            Index::NONE,
            [0u8; 32],
            "created".to_string(),
        );
        assert_eq!(event.event_id(), 1);
        assert_eq!(event.timestamp(), 1_700_000_000_000);
        assert_eq!(event.domain_id(), DomainId::new(5));
        assert!(!event.detached());
        assert!(event.precursor().is_none());
        assert_eq!(event.domain_event(), "created");
    }

    #[test]
    fn event_with_precursor() {
        let event = Event::new(
            2,
            1_700_000_000_001,
            DomainId::new(5),
            false,
            Index::new(0),
            [0u8; 32],
            "updated".to_string(),
        );
        assert_eq!(event.event_id(), 2);
        assert!(event.precursor().is_some());
        assert_eq!(event.precursor().value(), 0);
    }

    #[test]
    fn event_serde_roundtrip() {
        let event = Event::new(
            1,
            1_700_000_000_000,
            DomainId::new(1),
            false,
            Index::NONE,
            [0u8; 32],
            "created".to_string(),
        );
        let json = serde_json::to_string(&event).unwrap();
        let back: Event<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_id(), event.event_id());
        assert_eq!(back.domain_id(), event.domain_id());
        assert_eq!(back.domain_event(), "created");
        assert!(back.precursor().is_none());
    }

    #[test]
    fn event_with_precursor_serde_roundtrip() {
        let event = Event::new(
            2,
            1_700_000_000_001,
            DomainId::new(1),
            false,
            Index::new(0),
            [0u8; 32],
            "updated".to_string(),
        );
        let json = serde_json::to_string(&event).unwrap();
        let back: Event<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.precursor(), Index::new(0));
        assert!(back.precursor().is_some());
    }

    #[test]
    fn event_detached_flag() {
        let event = Event::new(
            3,
            1_700_000_000_002,
            DomainId::new(1),
            true,
            Index::new(1),
            [0u8; 32],
            "detached".to_string(),
        );
        assert!(event.detached());
    }

    // --- precursor_hash plumbing (PAR-0021 R1; F2a) ---

    #[test]
    fn event_precursor_hash_accessor() {
        // F2a is plumbing-only: callers provide [0u8; 32] sentinel until F2b
        // wires real BLAKE3 computation. Pin the accessor shape + position so
        // F2b can change the *value* without disturbing the surface.
        let event = Event::new(
            7,
            1_700_000_000_007,
            DomainId::new(1),
            false,
            Index::NONE,
            [0u8; 32],
            "hashed".to_string(),
        );
        assert_eq!(event.precursor_hash(), [0u8; 32]);
    }

    #[test]
    fn event_precursor_hash_nonzero_roundtrip() {
        // Distinguish "field exists" from "field is hard-zeroed somewhere
        // downstream" — pass a non-trivial bit-pattern and read it back.
        let hash: [u8; 32] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
            0x1d, 0x1e, 0x1f, 0x20,
        ];
        let event = Event::new(
            8,
            1_700_000_000_008,
            DomainId::new(1),
            false,
            Index::new(7),
            hash,
            "linked".to_string(),
        );
        assert_eq!(event.precursor_hash(), hash);

        // Serde round-trip pins that the field is serialized — `[u8; 32]`
        // under GEN-0035:R1 is fixed-width verbatim (no length prefix).
        let json = serde_json::to_string(&event).unwrap();
        let back: Event<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.precursor_hash(), hash);
    }
}
