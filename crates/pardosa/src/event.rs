use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::PardosaError;

/// Position in the append-only line.
///
/// GENOME LAYOUT: single `u64` field. Do not add fields or reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(clippy::struct_field_names)]
pub struct Event<T> {
    event_id: u64,
    timestamp: i64,
    domain_id: DomainId,
    detached: bool,
    precursor: Index,
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
        domain_event: T,
    ) -> Self {
        Event {
            event_id,
            timestamp,
            domain_id,
            detached,
            precursor,
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
    fn index_none_serde_roundtrip() {
        let i = Index::NONE;
        let json = serde_json::to_string(&i).unwrap();
        let back: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(back, i);
        assert!(back.is_none());
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
            "detached".to_string(),
        );
        assert!(event.detached());
    }
}
