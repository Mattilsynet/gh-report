use crate::event::{DomainEvent, EventEnvelope};

/// A policy reacts to domain events by producing commands, driving
/// cross-aggregate and cross-context coordination. Eventually consistent
/// by nature: it observes what happened and decides what should happen
/// next.
///
/// `Output` is a static associated type per CHE-0017 R1
/// (`Output: Send + Sync + 'static`, not `Box<dyn AnyCommand>`), letting
/// the compiler verify exhaustive dispatch. Policies receive
/// `EventEnvelope` rather than raw events for the metadata (timestamp,
/// `aggregate_id`) needed to target commands correctly.
///
/// `react` must be idempotent per CHE-0041 R2: delivery may be
/// at-least-once, so the same envelope must always produce the same
/// `Vec<Output>`.
///
/// # Examples
///
/// ```
/// use cherry_pit_core::{Policy, DomainEvent, EventEnvelope};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// enum OrderEvent { Placed }
/// impl DomainEvent for OrderEvent {
///     fn event_type(&self) -> &'static str { "order.placed" }
/// }
///
/// enum NotifyAction { SendEmail(String) }
///
/// struct OrderNotifier;
/// impl Policy for OrderNotifier {
///     type Event = OrderEvent;
///     type Output = NotifyAction;
///     fn react(&self, _event: &EventEnvelope<OrderEvent>) -> Vec<NotifyAction> {
///         vec![NotifyAction::SendEmail("placed".into())]
///     }
/// }
/// ```
pub trait Policy: Send + Sync + 'static {
    /// The event type this policy reacts to.
    type Event: DomainEvent;

    /// The output type — typically an enum of possible commands.
    type Output: Send + Sync + 'static;

    /// React to an event. Returns zero or more outputs to dispatch.
    ///
    /// An empty vec means this event is not relevant to this policy.
    /// Policies must be idempotent — reacting to the same event
    /// twice must produce the same outputs.
    #[must_use]
    fn react(&self, event: &EventEnvelope<Self::Event>) -> Vec<Self::Output>;
}

#[cfg(test)]
mod tests {
    //! Runtime coverage for CHE-0017 R1: `Policy::Output` is a static, concrete
    //! associated type — not `Box<dyn AnyCommand>` and not type-erased.
    //!
    //! The test is structural: we define an `impl Policy` whose `Output` is a
    //! plain enum (no `Box`, no `dyn`) and exercise `react`. Successful
    //! compilation is the assertion that `Output: Send + Sync + 'static` is
    //! satisfiable by a concrete type. A compile-time witness via a generic
    //! `_assert_sized` function pins the bound at type-check time.

    use std::num::NonZeroU64;

    use serde::{Deserialize, Serialize};

    use super::Policy;
    use crate::AggregateId;
    use crate::event::{DomainEvent, EventEnvelope};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    enum OrderEvent {
        Placed,
    }
    impl DomainEvent for OrderEvent {
        fn event_type(&self) -> &'static str {
            "order.placed"
        }
    }

    /// Concrete (unboxed, non-erased) policy output type.
    #[derive(Debug, PartialEq, Eq)]
    enum NotifyAction {
        SendEmail(String),
    }

    struct OrderNotifier;
    impl Policy for OrderNotifier {
        type Event = OrderEvent;
        type Output = NotifyAction;
        fn react(&self, _event: &EventEnvelope<OrderEvent>) -> Vec<NotifyAction> {
            vec![NotifyAction::SendEmail("placed".into())]
        }
    }

    /// Compile-time witness — instantiating this function forces the compiler
    /// to verify `P::Output: Send + Sync + 'static + Sized`. `Box<dyn Trait>`
    /// would satisfy these bounds (it IS Sized), so this assertion is about
    /// statically-known type identity, not unsizedness. The real binding
    /// constraint is the `'static` lifetime + concrete-type usage in `react`
    /// below: a `dyn`-typed Output would require `Box<_>` indirection in the
    /// return type, which `Vec<Self::Output>` does not permit without erasure.
    const fn assert_static_bounds<P: Policy>()
    where
        P::Output: Send + Sync + 'static + Sized,
    {
    }

    const _: () = assert_static_bounds::<OrderNotifier>();

    #[test]
    fn policy_output_is_unboxed_concrete_type() {
        let p = OrderNotifier;
        let envelope = EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).expect("non-zero")),
            NonZeroU64::new(1).expect("non-zero"),
            jiff::Timestamp::now(),
            None,
            None,
            OrderEvent::Placed,
        )
        .expect("valid envelope");
        let outputs = p.react(&envelope);
        assert_eq!(outputs.len(), 1);
        match &outputs[0] {
            NotifyAction::SendEmail(msg) => assert_eq!(msg, "placed"),
        }
    }
}
