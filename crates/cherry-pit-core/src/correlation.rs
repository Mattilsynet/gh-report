/// Context for correlating events across aggregates and bounded
/// contexts.
///
/// Passed explicitly through `CommandGateway` → `CommandBus` →
/// `EventStore`. The store stamps these values onto every
/// [`EventEnvelope`](crate::EventEnvelope) it creates.
/// (CHE-0039: correlation context propagation.)
///
/// # Design rationale
///
/// Does not implement `Default` — callers must explicitly choose
/// [`none()`](Self::none), [`correlated()`](Self::correlated), or
/// [`new()`](Self::new). The name communicates intent: forgetting
/// correlation is a conscious omission, not an accidental default.
/// (CHE-0039 R2: no `Default` impl.)
///
/// # Nil UUIDs
///
/// The constructors accept any `Uuid` value, including
/// [`Uuid::nil()`](uuid::Uuid::nil). A nil correlation or causation
/// ID is semantically valid (albeit unusual) — it is the caller's
/// responsibility to pass meaningful IDs. Typically these are UUID
/// v7 values generated at command dispatch time.
///
/// # Examples
///
/// ```
/// use cherry_pit_core::CorrelationContext;
///
/// // User-initiated command — no correlation.
/// let ctx = CorrelationContext::none();
/// assert!(ctx.correlation_id().is_none());
/// assert!(ctx.causation_id().is_none());
///
/// // Policy-initiated — full correlation chain.
/// let corr = uuid::Uuid::now_v7();
/// let cause = uuid::Uuid::now_v7();
/// let ctx = CorrelationContext::new(corr, cause);
/// assert_eq!(ctx.correlation_id(), Some(corr));
/// assert_eq!(ctx.causation_id(), Some(cause));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CorrelationContext {
    /// Groups related events into a single logical operation.
    /// Propagated through policies and sagas.
    correlation_id: Option<uuid::Uuid>,

    /// The `event_id` of the event that caused this command to be
    /// dispatched (via a policy or saga). `None` for user-initiated
    /// commands.
    causation_id: Option<uuid::Uuid>,
}

impl CorrelationContext {
    /// No correlation context — user-initiated command, no tracing.
    #[must_use]
    pub fn none() -> Self {
        Self {
            correlation_id: None,
            causation_id: None,
        }
    }

    /// Full correlation context — typically propagated from a policy
    /// reacting to a prior event.
    #[must_use]
    pub fn new(correlation_id: uuid::Uuid, causation_id: uuid::Uuid) -> Self {
        Self {
            correlation_id: Some(correlation_id),
            causation_id: Some(causation_id),
        }
    }

    /// Correlation only — first command in a logical operation chain,
    /// no causation event yet.
    #[must_use]
    pub fn correlated(correlation_id: uuid::Uuid) -> Self {
        Self {
            correlation_id: Some(correlation_id),
            causation_id: None,
        }
    }

    /// The correlation ID, if set.
    #[must_use]
    pub fn correlation_id(&self) -> Option<uuid::Uuid> {
        self.correlation_id
    }

    /// The causation ID, if set.
    #[must_use]
    pub fn causation_id(&self) -> Option<uuid::Uuid> {
        self.causation_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_produces_no_ids() {
        let ctx = CorrelationContext::none();
        assert!(ctx.correlation_id().is_none());
        assert!(ctx.causation_id().is_none());
    }

    #[test]
    fn new_produces_both_ids() {
        let corr = uuid::Uuid::now_v7();
        let cause = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::new(corr, cause);
        assert_eq!(ctx.correlation_id(), Some(corr));
        assert_eq!(ctx.causation_id(), Some(cause));
    }

    #[test]
    fn correlated_produces_correlation_only() {
        let corr = uuid::Uuid::now_v7();
        let ctx = CorrelationContext::correlated(corr);
        assert_eq!(ctx.correlation_id(), Some(corr));
        assert!(ctx.causation_id().is_none());
    }

    #[test]
    fn clone_produces_equal_context() {
        let ctx = CorrelationContext::new(uuid::Uuid::now_v7(), uuid::Uuid::now_v7());
        let cloned = ctx.clone();
        assert_eq!(ctx, cloned);
    }

    #[test]
    fn partial_eq_works() {
        let a = CorrelationContext::none();
        let b = CorrelationContext::none();
        assert_eq!(a, b);

        let corr = uuid::Uuid::now_v7();
        let c = CorrelationContext::correlated(corr);
        let d = CorrelationContext::correlated(corr);
        assert_eq!(c, d);
        assert_ne!(a, c);
    }
}
