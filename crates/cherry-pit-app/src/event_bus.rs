//! In-process synchronous fan-out implementation of
//! [`cherry_pit_core::EventBus`].
//!
//! Per CHE-0051:R2, handlers live in a `Vec<HandlerFn>` registered via
//! [`InProcessEventBus::register`], invoked synchronously inside
//! `publish` (CHE-0024:§7 delivery contract). `EventBus` carries no
//! `subscribe` method (CHE-0024:R1/R2); publication failure is
//! non-fatal since events persist before `publish` runs —
//! orchestration lives in `App` (S5). Per CHE-0005:R1 + CHE-0051:R2,
//! one instance exists per aggregate type; multi-aggregate composition
//! is parameter expansion (CHE-0051:R9), never a heterogeneous
//! registry.
//!
//! ## C1 boundary note (Linus R1 review focus)
//!
//! Handler storage is `Arc<dyn Fn(&EventEnvelope<E>) + Send + Sync>` —
//! dispatch over user closures, not an infra port trait. CHE-0005:R1
//! forbids `dyn` over `EventStore`, `EventBus`, `CommandBus`,
//! `CommandGateway`, `Policy`, `Projection`; closures aren't infra
//! ports. Rejected shape's mitigation is a typed-tuple pivot, keeping
//! `register` on the impl.
use std::sync::{Arc, Mutex};

use cherry_pit_core::{BusError, DomainEvent, EventBus, EventEnvelope};

/// Synchronous closure invoked for each published envelope.
///
/// Per CHE-0024:§7 in-process semantics, handlers run synchronously
/// inside `publish` — no spawn, no async. Stored under `Arc` so the
/// copy-on-write registration path (see [`InProcessEventBus::register`])
/// rebuilds the snapshot vector by cheap pointer clones rather than
/// moving the underlying closures.
type HandlerFn<E> = Arc<dyn Fn(&EventEnvelope<E>) + Send + Sync>;

/// In-process synchronous event bus implementing
/// [`cherry_pit_core::EventBus`] per CHE-0051:R2.
///
/// Holds `Vec<HandlerFn<E>>` of user-registered closures; `publish`
/// invokes every handler in order before returning `Ok(())`. One
/// instance exists per aggregate event type `E` (CHE-0051:R2 +
/// CHE-0005:R1); multi-aggregate composition expands the type
/// parameter at the `App` site (CHE-0051:R9), not a heterogeneous
/// registry.
///
/// ## Concurrency
///
/// Backed by `Mutex<Arc<Vec<HandlerFn<E>>>>` (copy-on-write):
/// registration swaps in a fresh `Arc`; publication clones the current
/// snapshot under a brief lock, releases it, then invokes each handler
/// — never held during invocation, so a handler may safely re-enter
/// the bus. CHE-0024:§7's fanout contract holds regardless.
///
/// ## Failure model
///
/// `publish` never produces `Err(BusError)` — no fallible in-process
/// transport. A panicking handler propagates; per CHE-0051:R7,
/// dead-letter routing of failed policy outputs is `App`'s job.
pub struct InProcessEventBus<E: DomainEvent> {
    handlers: Mutex<Arc<Vec<HandlerFn<E>>>>,
}

impl<E: DomainEvent> InProcessEventBus<E> {
    /// Construct an empty bus with no handlers registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            handlers: Mutex::new(Arc::new(Vec::new())),
        }
    }

    /// Register a handler closure invoked synchronously for every
    /// published envelope.
    ///
    /// Per CHE-0024:R2 + CHE-0051:R2 this is an **impl-specific**
    /// registration method — the `cherry_pit_core::EventBus` port trait
    /// deliberately carries no `subscribe`/`register`, since
    /// subscription is inherently implementation-specific (in-process
    /// channels vs NATS subjects vs polling).
    ///
    /// Handlers must be `Send + Sync + 'static` because the bus itself
    /// is, and publication may cross threads (e.g. tokio multi-threaded
    /// runtime).
    ///
    /// Registration is **copy-on-write**: the current handler vector is
    /// cloned, the new handler appended to the clone, and the bus swaps
    /// the snapshot `Arc` under a brief lock. Snapshots already
    /// observed by an in-flight `publish` keep firing over the
    /// pre-registration handler set; later publishes observe the new
    /// one. This makes reentrant `register` from inside a handler safe.
    pub fn register<F>(&self, handler: F)
    where
        F: Fn(&EventEnvelope<E>) + Send + Sync + 'static,
    {
        let mut guard = self
            .handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next: Vec<HandlerFn<E>> = Vec::with_capacity(guard.len() + 1);
        next.extend(guard.iter().map(Arc::clone));
        next.push(Arc::new(handler));
        *guard = Arc::new(next);
    }

    /// Number of currently registered handlers.
    ///
    /// Primarily for testing and diagnostic logging.
    #[must_use]
    pub fn handler_count(&self) -> usize {
        self.handlers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl<E: DomainEvent> Default for InProcessEventBus<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: DomainEvent> std::fmt::Debug for InProcessEventBus<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcessEventBus")
            .field("handler_count", &self.handler_count())
            .finish()
    }
}

impl<E: DomainEvent> EventBus for InProcessEventBus<E> {
    type Event = E;

    /// Publish synchronously to every registered handler in
    /// registration order, then return `Ok(())`.
    ///
    /// Per CHE-0024:§7, in-process delivery is synchronous within
    /// `publish` — the bus calls each handler before returning. The
    /// returned `impl Future` resolves immediately; no work spawns.
    ///
    /// Per CHE-0024:R1, persist-then-publish ordering and non-fatal
    /// publication failure are enforced at `CommandBus`/`App` level.
    /// Never returns `Err(BusError)`.
    ///
    /// # Hazards
    ///
    /// **Handlers run synchronously inside `publish` — no awaiting on
    /// publisher-held locks (CHE-0051:R8 advisory).** The mutex
    /// releases before fan-out: the snapshot clones under a brief
    /// lock, then drops, so handlers never hold the bus's mutex —
    /// reentrant `register`/`publish` is deadlock-safe.
    ///
    /// The advisory still applies at the publisher boundary: a handler
    /// bridging into async via `Handle::block_on(...)` can deadlock on
    /// a publisher-held lock; see `App::run`'s `# Hazards` section.
    fn publish(
        &self,
        events: &[EventEnvelope<Self::Event>],
    ) -> impl std::future::Future<Output = Result<(), BusError>> + Send {
        let snapshot: Arc<Vec<HandlerFn<Self::Event>>> = {
            let guard = self
                .handlers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(&guard)
        };
        for envelope in events {
            for handler in snapshot.iter() {
                handler(envelope);
            }
        }
        async move { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;
    use std::sync::Arc;

    use cherry_pit_core::{AggregateId, DomainEvent, EventBus, EventEnvelope};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    enum TestEvent {
        Happened { value: u32 },
    }

    impl DomainEvent for TestEvent {
        fn event_type(&self) -> &'static str {
            "test.happened"
        }
    }

    fn envelope(value: u32, seq: u64) -> EventEnvelope<TestEvent> {
        EventEnvelope::new(
            uuid::Uuid::now_v7(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            NonZeroU64::new(seq).unwrap(),
            jiff::Timestamp::now(),
            None,
            None,
            TestEvent::Happened { value },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn publish_with_one_handler_invokes_once_per_envelope() {
        let bus: InProcessEventBus<TestEvent> = InProcessEventBus::new();
        let recorded: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded_clone = Arc::clone(&recorded);
        bus.register(move |env: &EventEnvelope<TestEvent>| {
            let TestEvent::Happened { value } = env.payload();
            recorded_clone.lock().unwrap().push(*value);
        });

        bus.publish(&[envelope(7, 1)]).await.unwrap();

        assert_eq!(*recorded.lock().unwrap(), vec![7]);
    }

    #[tokio::test]
    async fn publish_with_two_handlers_invokes_both() {
        let bus: InProcessEventBus<TestEvent> = InProcessEventBus::new();
        let a: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let b: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let a_c = Arc::clone(&a);
        let b_c = Arc::clone(&b);
        bus.register(move |env| {
            let TestEvent::Happened { value } = env.payload();
            *a_c.lock().unwrap() += value;
        });
        bus.register(move |env| {
            let TestEvent::Happened { value } = env.payload();
            *b_c.lock().unwrap() += value * 2;
        });

        bus.publish(&[envelope(3, 1)]).await.unwrap();

        assert_eq!(*a.lock().unwrap(), 3);
        assert_eq!(*b.lock().unwrap(), 6);
        assert_eq!(bus.handler_count(), 2);
    }

    #[tokio::test]
    async fn publish_with_zero_handlers_is_noop() {
        let bus: InProcessEventBus<TestEvent> = InProcessEventBus::new();
        bus.publish(&[envelope(1, 1), envelope(2, 2)])
            .await
            .unwrap();
        assert_eq!(bus.handler_count(), 0);
    }

    #[tokio::test]
    async fn publish_empty_slice_is_noop() {
        let bus: InProcessEventBus<TestEvent> = InProcessEventBus::new();
        let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
        let calls_c = Arc::clone(&calls);
        bus.register(move |_env| {
            *calls_c.lock().unwrap() += 1;
        });

        bus.publish(&[]).await.unwrap();

        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn default_constructs_empty_bus() {
        let bus: InProcessEventBus<TestEvent> = InProcessEventBus::default();
        assert_eq!(bus.handler_count(), 0);
    }

    #[tokio::test]
    async fn publish_iterates_all_envelopes_across_all_handlers() {
        let bus: InProcessEventBus<TestEvent> = InProcessEventBus::new();
        let recorded: Arc<Mutex<Vec<(usize, u32)>>> = Arc::new(Mutex::new(Vec::new()));
        for handler_idx in 0..3usize {
            let recorded_c = Arc::clone(&recorded);
            bus.register(move |env| {
                let TestEvent::Happened { value } = env.payload();
                recorded_c.lock().unwrap().push((handler_idx, *value));
            });
        }

        bus.publish(&[envelope(10, 1), envelope(20, 2)])
            .await
            .unwrap();

        let log = recorded.lock().unwrap().clone();
        assert_eq!(log.len(), 6);
        assert_eq!(
            log,
            vec![(0, 10), (1, 10), (2, 10), (0, 20), (1, 20), (2, 20)]
        );
    }
}
