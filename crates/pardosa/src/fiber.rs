use crate::error::{FiberInvariantKind, FiberLenReason, IndexOrderingKind, PardosaError};
use crate::event::Index;
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "FiberRaw")]
pub(crate) struct Fiber {
    anchor: Index,
    len: u64,
    current: Index,
}
impl Fiber {
    /// Build a `Fiber` from raw fields, validating the anchor/current/len invariants.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberInvariant` if `len < 1` or `current < anchor`.
    /// All `Index` values are legal positions (F1 / mission
    /// `f1-sentinel-removal-20260524`); the prior `Index::NONE` sentinel was
    /// removed and absence of a precursor is now expressed via
    /// `Precursor::Genesis` on `Event<T>`. A fiber always points at a real
    /// in-line position.
    pub fn new(anchor: Index, len: u64, current: Index) -> Result<Fiber, PardosaError> {
        if len < 1 {
            return Err(PardosaError::FiberInvariant(FiberInvariantKind::FiberLen(
                FiberLenReason::Zero,
            )));
        }
        if current.value() < anchor.value() {
            return Err(PardosaError::FiberInvariant(
                FiberInvariantKind::IndexOrdering(IndexOrderingKind::CurrentBelowAnchor {
                    anchor,
                    current,
                }),
            ));
        }
        Ok(Fiber {
            anchor,
            len,
            current,
        })
    }
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }
    #[must_use]
    pub fn current(&self) -> Index {
        self.current
    }
    /// Advance `current` to `new_current`, validating monotonicity and length overflow.
    ///
    /// # Errors
    /// Returns `PardosaError::FiberInvariant` if `new_current <= current` or
    /// `len + 1` would overflow `u64`.
    pub fn advance(&mut self, new_current: Index) -> Result<(), PardosaError> {
        self.check_advance(new_current)?;
        self.advance_unchecked(new_current);
        Ok(())
    }
    pub(crate) fn check_advance(&self, new_current: Index) -> Result<(), PardosaError> {
        if new_current.value() <= self.current.value() {
            return Err(PardosaError::FiberInvariant(
                FiberInvariantKind::IndexOrdering(IndexOrderingKind::NewCurrentNotAfterCurrent {
                    current: self.current,
                    new_current,
                }),
            ));
        }
        if self.len.checked_add(1).is_none() {
            return Err(PardosaError::FiberInvariant(FiberInvariantKind::FiberLen(
                FiberLenReason::Overflow,
            )));
        }
        Ok(())
    }
    pub(crate) fn advance_unchecked(&mut self, new_current: Index) {
        debug_assert!(
            self.check_advance(new_current).is_ok(),
            "advance_unchecked called without preceding check_advance for {new_current:?}"
        );
        self.len = self
            .len
            .checked_add(1)
            .expect("check_advance verified non-overflow");
        self.current = new_current;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fiber_new_valid() {
        let f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        assert_eq!(f.anchor, Index::new(0));
        assert_eq!(f.len(), 1);
        assert_eq!(f.current(), Index::new(0));
    }
    #[test]
    fn fiber_new_accepts_u64_max_anchor() {
        let f = Fiber::new(Index::new(u64::MAX), 1, Index::new(u64::MAX)).unwrap();
        assert_eq!(f.anchor, Index::new(u64::MAX));
        assert_eq!(f.current(), Index::new(u64::MAX));
    }
    #[test]
    fn fiber_new_len_zero_rejected() {
        let err = Fiber::new(Index::new(0), 0, Index::new(0)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::FiberLen(FiberLenReason::Zero))
            ),
            "expected len error, got: {err}"
        );
    }
    #[test]
    fn fiber_new_current_less_than_anchor_rejected() {
        let err = Fiber::new(Index::new(5), 1, Index::new(3)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::IndexOrdering(
                    IndexOrderingKind::CurrentBelowAnchor { .. }
                ))
            ),
            "expected ordering error, got: {err}"
        );
    }
    #[test]
    fn fiber_new_current_equals_anchor() {
        let f = Fiber::new(Index::new(5), 1, Index::new(5)).unwrap();
        assert_eq!(f.anchor, Index::new(5));
        assert_eq!(f.current(), Index::new(5));
    }
    #[test]
    fn fiber_new_current_greater_than_anchor() {
        let f = Fiber::new(Index::new(5), 3, Index::new(10)).unwrap();
        assert_eq!(f.anchor, Index::new(5));
        assert_eq!(f.current(), Index::new(10));
        assert_eq!(f.len(), 3);
    }
    #[test]
    fn fiber_advance_valid() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        f.advance(Index::new(3)).unwrap();
        assert_eq!(f.current(), Index::new(3));
        assert_eq!(f.len(), 2);
    }
    #[test]
    fn fiber_advance_equal_rejected() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(5)).unwrap();
        let err = f.advance(Index::new(5)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::IndexOrdering(
                    IndexOrderingKind::NewCurrentNotAfterCurrent { .. }
                ))
            ),
            "expected ordering error, got: {err}"
        );
    }
    #[test]
    fn fiber_advance_less_rejected() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(5)).unwrap();
        let err = f.advance(Index::new(3)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::IndexOrdering(
                    IndexOrderingKind::NewCurrentNotAfterCurrent { .. }
                ))
            ),
            "expected ordering error, got: {err}"
        );
    }
    #[test]
    fn fiber_check_advance_does_not_mutate() {
        let f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        f.check_advance(Index::new(3)).unwrap();
        assert_eq!(f.current(), Index::new(0));
        assert_eq!(f.len(), 1);
    }
    #[test]
    fn fiber_check_advance_surfaces_all_advance_errors() {
        let f = Fiber::new(Index::new(0), 1, Index::new(5)).unwrap();
        assert!(f.check_advance(Index::new(5)).is_err());
        assert!(f.check_advance(Index::new(3)).is_err());
    }
    #[test]
    fn fiber_advance_unchecked_mutates_in_place() {
        let mut f = Fiber::new(Index::new(0), 1, Index::new(0)).unwrap();
        f.check_advance(Index::new(7)).unwrap();
        f.advance_unchecked(Index::new(7));
        assert_eq!(f.current(), Index::new(7));
        assert_eq!(f.len(), 2);
    }
    #[test]
    fn fiber_deserialize_invalid_len_zero_rejected() {
        let json = r#"{"anchor":0,"len":0,"current":0}"#;
        let result: Result<Fiber, _> = serde_json::from_str(json);
        assert!(result.is_err(), "deserialization should reject len=0");
    }
    #[test]
    fn fiber_deserialize_accepts_anchor_u64_max() {
        let json = r#"{"anchor":18446744073709551615,"len":1,"current":18446744073709551615}"#;
        let result: Result<Fiber, _> = serde_json::from_str(json);
        assert!(
            result.is_ok(),
            "F1: u64::MAX is a legal Index value; Fiber must accept it"
        );
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
    #[test]
    fn fiber_serde_roundtrip() {
        let f = Fiber::new(Index::new(5), 3, Index::new(10)).unwrap();
        let json = serde_json::to_string(&f).unwrap();
        let back: Fiber = serde_json::from_str(&json).unwrap();
        assert_eq!(back.anchor, f.anchor);
        assert_eq!(back.len(), f.len());
        assert_eq!(back.current(), f.current());
    }
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
