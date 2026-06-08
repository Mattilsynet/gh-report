use crate::error::{FiberInvariantKind, LinevecAppendKind, PardosaError};
use crate::event::{Event, EventId};
#[derive(Debug)]
pub(crate) struct Linevec<T>(Vec<Event<T>>);
impl<T> Default for Linevec<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Linevec<T> {
    pub(crate) const fn new() -> Self {
        Linevec(Vec::new())
    }
    pub(crate) fn append_validated(
        &mut self,
        event: Event<T>,
        expected_event_id: EventId,
    ) -> Result<(), PardosaError> {
        if event.event_id() != expected_event_id {
            return Err(PardosaError::FiberInvariant(
                FiberInvariantKind::LinevecAppend(LinevecAppendKind::EventIdMismatch {
                    actual: event.event_id().value(),
                    expected: expected_event_id.value(),
                }),
            ));
        }
        if let Some(last) = self.0.last()
            && expected_event_id <= last.event_id()
        {
            return Err(PardosaError::FiberInvariant(
                FiberInvariantKind::LinevecAppend(LinevecAppendKind::EventIdNotMonotonic {
                    event_id: expected_event_id.value(),
                    last_event_id: last.event_id().value(),
                }),
            ));
        }
        let precursor = event.precursor();
        if let Some(precursor_idx) = precursor.as_index() {
            let p = match usize::try_from(precursor_idx) {
                Ok(v) => v,
                Err(e) => {
                    return Err(PardosaError::FiberInvariant(
                        FiberInvariantKind::LinevecAppend(
                            LinevecAppendKind::PrecursorIndexOutOfBounds {
                                precursor_index: e.0,
                                line_len: self.0.len(),
                            },
                        ),
                    ));
                }
            };
            if p >= self.0.len() {
                return Err(PardosaError::FiberInvariant(
                    FiberInvariantKind::LinevecAppend(
                        LinevecAppendKind::PrecursorIndexOutOfBounds {
                            precursor_index: p as u64,
                            line_len: self.0.len(),
                        },
                    ),
                ));
            }
            if self.0[p].fiber_id() != event.fiber_id() {
                return Err(PardosaError::BrokenPrecursorChain {
                    event_id: event.event_id().value(),
                    precursor: precursor_idx,
                });
            }
        }
        self.0.push(event);
        Ok(())
    }
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
    pub(crate) fn as_slice(&self) -> &[Event<T>] {
        &self.0
    }
    #[cfg(test)]
    pub(crate) fn force_push_unchecked(&mut self, event: Event<T>) {
        self.0.push(event);
    }
    pub(crate) fn from_raw_unchecked(events: Vec<Event<T>>) -> Self {
        Linevec(events)
    }
}
impl<T: Clone> Clone for Linevec<T> {
    fn clone(&self) -> Self {
        Linevec(self.0.clone())
    }
}
impl<T> std::ops::Index<usize> for Linevec<T> {
    type Output = Event<T>;
    fn index(&self, i: usize) -> &Event<T> {
        &self.0[i]
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{FiberInvariantKind, LinevecAppendKind};
    use crate::event::EventId;
    use crate::{Event, FiberId, Index, PardosaError, event::Precursor};
    #[test]
    fn linevec_append_validated_rejects_mismatched_event_id() {
        let mut lv = Linevec::<&str>::new();
        let e = Event::new_unchecked(
            5,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "x",
        );
        let err = lv.append_validated(e, EventId::new(7)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::LinevecAppend(
                    LinevecAppendKind::EventIdMismatch { .. }
                ))
            ),
            "got: {err}"
        );
        assert_eq!(lv.len(), 0, "rejected append must not mutate line");
    }
    #[test]
    fn linevec_append_validated_rejects_non_monotonic_event_id() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new_unchecked(
            0,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, EventId::new(0)).unwrap();
        let e_dup = Event::new_unchecked(
            0,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "dup",
        );
        let err = lv.append_validated(e_dup, EventId::new(0)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::LinevecAppend(
                    LinevecAppendKind::EventIdNotMonotonic { .. }
                ))
            ),
            "got: {err}"
        );
        assert_eq!(lv.len(), 1, "rejected append must not mutate line");
    }
    #[test]
    fn linevec_append_validated_rejects_forward_precursor() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new_unchecked(
            0,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, EventId::new(0)).unwrap();
        let bad = Event::new_unchecked(
            1,
            FiberId::new(0),
            false,
            Precursor::Of(Index::new(5)),
            [0u8; 32],
            "bad",
        );
        let err = lv.append_validated(bad, EventId::new(1)).unwrap_err();
        assert!(
            matches!(
                err,
                PardosaError::FiberInvariant(FiberInvariantKind::LinevecAppend(
                    LinevecAppendKind::PrecursorIndexOutOfBounds { .. }
                ))
            ),
            "got: {err}"
        );
        assert_eq!(lv.len(), 1, "rejected append must not mutate line");
    }
    /// W3 (roadmap correctness 2026-05-24): the `precursor_index`
    /// field is now `u64`, carrying the raw decoded value losslessly
    /// even on 32-bit targets where it could exceed `usize::MAX`.
    /// Pin the field type explicitly here so any future widening or
    /// re-narrowing is caught.
    #[test]
    fn linevec_append_validated_carries_precursor_index_as_u64() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new_unchecked(
            0,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, EventId::new(0)).unwrap();
        let bad = Event::new_unchecked(
            1,
            FiberId::new(0),
            false,
            Precursor::Of(Index::new(5)),
            [0u8; 32],
            "bad",
        );
        let err = lv.append_validated(bad, EventId::new(1)).unwrap_err();
        let PardosaError::FiberInvariant(FiberInvariantKind::LinevecAppend(
            LinevecAppendKind::PrecursorIndexOutOfBounds {
                precursor_index,
                line_len,
            },
        )) = err
        else {
            panic!("expected PrecursorIndexOutOfBounds, got: {err}");
        };
        let _: u64 = precursor_index;
        assert_eq!(precursor_index, 5u64);
        assert_eq!(line_len, 1);
    }
    #[test]
    fn linevec_append_validated_rejects_cross_domain_precursor() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new_unchecked(
            0,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, EventId::new(0)).unwrap();
        let bad = Event::new_unchecked(
            1,
            FiberId::new(99),
            false,
            Precursor::Of(Index::new(0)),
            [0u8; 32],
            "bad",
        );
        let err = lv.append_validated(bad, EventId::new(1)).unwrap_err();
        assert!(
            matches!(err, PardosaError::BrokenPrecursorChain { .. }),
            "got: {err}"
        );
        assert_eq!(lv.len(), 1, "rejected append must not mutate line");
    }
    #[test]
    fn linevec_append_validated_accepts_valid_event() {
        let mut lv = Linevec::<&str>::new();
        let e0 = Event::new_unchecked(
            0,
            FiberId::new(0),
            false,
            Precursor::Genesis,
            [0u8; 32],
            "a",
        );
        lv.append_validated(e0, EventId::new(0)).unwrap();
        let e1 = Event::new_unchecked(
            1,
            FiberId::new(0),
            false,
            Precursor::Of(Index::new(0)),
            [0u8; 32],
            "b",
        );
        lv.append_validated(e1, EventId::new(1)).unwrap();
        assert_eq!(lv.len(), 2);
    }
}
