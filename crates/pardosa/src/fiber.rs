use serde::{Deserialize, Serialize};

use crate::error::PardosaError;
use crate::event::Index;

/// Raw deserialization helper for `Fiber`. Validates invariants via
/// `Fiber::new()` on conversion, preventing deserialization bypass.
#[derive(Deserialize)]
struct FiberRaw {
    anchor: Index,
    len: u64,
    current: Index,
}

impl TryFrom<FiberRaw> for Fiber {
    type Error = String;

    fn try_from(raw: FiberRaw) -> Result<Self, Self::Error> {
        Fiber::new(raw.anchor, raw.len, raw.current).map_err(|e| e.to_string())
    }
}

/// Tracks the position and length of a fiber within the line.
///
/// Invariants: `len >= 1`, `current >= anchor`, neither is `Index::NONE`.
///
/// Fibers are sparse — events from multiple fibers interleave in the
/// append-only line, so `len` may be less than `current - anchor + 1`.
/// The `len` field counts events belonging to this fiber, not contiguous
/// index positions.
///
/// GENOME LAYOUT: fields are serialized in declaration order.
/// Changing field order is a breaking change — `schema_id` will change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "FiberRaw")]
pub struct Fiber {
    anchor: Index,
    len: u64,
    current: Index,
}

impl Fiber {
    /// Create a new fiber. Returns error if invariants are violated:
    /// - `anchor` must not be `Index::NONE`
    /// - `current` must not be `Index::NONE`
    /// - `len` must be >= 1
    /// - `current` must be >= `anchor` (by value)
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberInvariantViolation`] when any of the
    /// above invariants are violated.
    pub fn new(anchor: Index, len: u64, current: Index) -> Result<Fiber, PardosaError> {
        if anchor.is_none() {
            return Err(PardosaError::FiberInvariantViolation(
                "anchor must not be Index::NONE".into(),
            ));
        }
        if current.is_none() {
            return Err(PardosaError::FiberInvariantViolation(
                "current must not be Index::NONE".into(),
            ));
        }
        if len < 1 {
            return Err(PardosaError::FiberInvariantViolation(
                "len must be >= 1".into(),
            ));
        }
        if current.value() < anchor.value() {
            return Err(PardosaError::FiberInvariantViolation(
                "current must be >= anchor".into(),
            ));
        }
        Ok(Fiber {
            anchor,
            len,
            current,
        })
    }

    #[must_use]
    pub fn anchor(&self) -> Index {
        self.anchor
    }

    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Always returns `false` — fibers have an invariant of `len >= 1`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    #[must_use]
    pub fn current(&self) -> Index {
        self.current
    }

    /// Update fiber after appending a new event at `new_current`.
    /// Returns error if `new_current` is not strictly greater than `current`,
    /// if `new_current` is the sentinel value, or if `len` overflows.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::FiberInvariantViolation`] if `new_current`
    /// is `Index::NONE`, not strictly greater than the current index, or
    /// if the internal length counter overflows.
    pub fn advance(&mut self, new_current: Index) -> Result<(), PardosaError> {
        if new_current.is_none() {
            return Err(PardosaError::FiberInvariantViolation(
                "new_current must not be Index::NONE".into(),
            ));
        }
        if new_current.value() <= self.current.value() {
            return Err(PardosaError::FiberInvariantViolation(
                "new_current must be > current".into(),
            ));
        }
        self.len = self
            .len
            .checked_add(1)
            .ok_or_else(|| PardosaError::FiberInvariantViolation("fiber len overflow".into()))?;
        self.current = new_current;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Fiber::new invariant checks ---

    #[test]
    fn fiber_new_valid() {
        let f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        assert_eq!(f.anchor(), Index::new(0));
        assert_eq!(f.len(), 1);
        assert_eq!(f.current(), Index::new(0));
    }

    #[test]
    fn fiber_new_anchor_none_rejected() {
        let err = Fiber::new(Index::NONE, 1, Index::new(0)).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains("anchor")),
            "expected anchor error, got: {err}"
        );
    }

    #[test]
    fn fiber_new_current_none_rejected() {
        let err = Fiber::new(Index::new(0), 1, Index::NONE).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains("current")),
            "expected current error, got: {err}"
        );
    }

    #[test]
    fn fiber_new_len_zero_rejected() {
        let err = Fiber::new(Index::new(0), 0, Index::new(0)).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains("len")),
            "expected len error, got: {err}"
        );
    }

    #[test]
    fn fiber_new_current_less_than_anchor_rejected() {
        let err = Fiber::new(Index::new(5), 1, Index::new(3)).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains("current must be >= anchor")),
            "expected ordering error, got: {err}"
        );
    }

    #[test]
    fn fiber_new_current_equals_anchor() {
        let f = Fiber::new(Index::new(5), 1, Index::new(5)).unwrap();
        assert_eq!(f.anchor(), Index::new(5));
        assert_eq!(f.current(), Index::new(5));
    }

    #[test]
    fn fiber_new_current_greater_than_anchor() {
        let f = Fiber::new(Index::new(5), 3, Index::new(10)).unwrap();
        assert_eq!(f.anchor(), Index::new(5));
        assert_eq!(f.current(), Index::new(10));
        assert_eq!(f.len(), 3);
    }

    // --- Fiber::advance ---

    #[test]
    fn fiber_advance_valid() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        f.advance(Index::new(3)).unwrap();
        assert_eq!(f.current(), Index::new(3));
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn fiber_advance_none_rejected() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        let err = f.advance(Index::NONE).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains("NONE")),
            "expected NONE error, got: {err}"
        );
    }

    #[test]
    fn fiber_advance_equal_rejected() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(5)).unwrap();
        let err = f.advance(Index::new(5)).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains('>')),
            "expected ordering error, got: {err}"
        );
    }

    #[test]
    fn fiber_advance_less_rejected() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(5)).unwrap();
        let err = f.advance(Index::new(3)).unwrap_err();
        assert!(
            matches!(err, PardosaError::FiberInvariantViolation(ref msg) if msg.contains('>')),
            "expected ordering error, got: {err}"
        );
    }

    // --- Fiber deserialization validates invariants ---

    #[test]
    fn fiber_deserialize_invalid_len_zero_rejected() {
        let json = r#"{"anchor":0,"len":0,"current":0}"#;
        let result: Result<Fiber, _> = serde_json::from_str(json);
        assert!(result.is_err(), "deserialization should reject len=0");
    }

    #[test]
    fn fiber_deserialize_invalid_anchor_none_rejected() {
        let json = r#"{"anchor":18446744073709551615,"len":1,"current":0}"#;
        let result: Result<Fiber, _> = serde_json::from_str(json);
        assert!(result.is_err(), "deserialization should reject anchor=NONE");
    }

    #[test]
    fn fiber_deserialize_invalid_current_less_than_anchor() {
        let json = r#"{"anchor":5,"len":1,"current":3}"#;
        let result: Result<Fiber, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "deserialization should reject current < anchor"
        );
    }

    // --- Fiber serde roundtrip ---

    #[test]
    fn fiber_serde_roundtrip() {
        let f = Fiber::new(Index::new(5), 3, Index::new(10)).unwrap();
        let json = serde_json::to_string(&f).unwrap();
        let back: Fiber = serde_json::from_str(&json).unwrap();
        assert_eq!(back.anchor(), f.anchor());
        assert_eq!(back.len(), f.len());
        assert_eq!(back.current(), f.current());
    }

    // --- LockedRescuePolicy serde ---

    #[test]
    fn locked_rescue_policy_serde_roundtrip() {
        use crate::fiber_state::LockedRescuePolicy;

        let policies = [
            LockedRescuePolicy::PreserveAuditTrail,
            LockedRescuePolicy::AcceptDataLoss,
        ];
        for policy in &policies {
            let json = serde_json::to_string(policy).unwrap();
            let back: LockedRescuePolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(*policy, back);
        }
    }
}
