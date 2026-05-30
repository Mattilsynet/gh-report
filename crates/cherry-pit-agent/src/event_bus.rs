//! In-process synchronous fan-out implementation of
//! [`cherry_pit_core::EventBus`].
//!
//! Per CHE-0051:R2: handlers are stored in a `Vec<HandlerFn>` registered
//! through an impl-specific [`InProcessEventBus::register`] method and
//! invoked synchronously inside `publish`, satisfying CHE-0024:§7
//! ("In-process delivery is synchronous within `publish` — the bus
//! calls each registered handler before returning.").
//!
//! Per CHE-0024:R1 + CHE-0024:R2: the `EventBus` port itself acquires no
//! `subscribe` method — registration is implementation-specific. Per
//! CHE-0024:R1 publication failure is non-fatal at the *system* level
//! because events are already persisted by the `EventStore` before the
//! `CommandBus` calls `publish`. Persist-then-publish *orchestration*
//! lives in `App` (S5), not in this bus.
//!
//! Per CHE-0005:R1 + CHE-0051:R2: one `InProcessEventBus<E>` instance
//! exists per aggregate type. Multi-aggregate composition is handled by
//! parameter expansion (CHE-0051:R9), never by a heterogeneous handler
//! registry.
//!
//! ## C1 boundary note (Linus R1 review focus)
//!
//! The handler storage uses `Box<dyn Fn(&EventEnvelope<E>) + Send + Sync>`.
//! This dynamic dispatch is over **user-supplied callback closures**, not
//! over an infrastructure port trait. CHE-0005:R1 forbids `Box<dyn>` over
//! `EventStore`, `EventBus`, `CommandBus`, `CommandGateway`, `Policy`,
//! `Projection` — i.e. Aggregate-bound infra ports. Closures registered
//! against a concrete bus impl are not infra ports; they are the
//! synchronous-fan-out callback shape mandated by CHE-0024:§7 and
//! CHE-0051:R2.
//!
//! If review rejects this dyn-closure shape, the documented mitigation
//! (per the WU-5 brief) is a pivot to a typed-tuple-of-handlers built on
//! the same registration surface. The pivot path is preserved by keeping
//! `register` on the impl (not on a trait surface that App would have to
//! re-declare).

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
/// Holds a `Vec<HandlerFn<E>>` of user-registered closures. `publish`
/// invokes every handler in registration order, synchronously, before
/// returning `Ok(())`.
///
/// Per CHE-0051:R2 + CHE-0005:R1: one instance per aggregate event type
/// `E`. Multi-aggregate composition expands the type-parameter list at
/// the consumer's `App` site (CHE-0051:R9), never via a heterogeneous
/// registry inside this bus.
///
/// ## Concurrency
///
/// Backed by `std::sync::Mutex<Arc<Vec<HandlerFn<E>>>>` (copy-on-write).
/// Registration briefly takes the lock to swap in a fresh `Arc<Vec<…>>`
/// carrying the appended handler. Publication briefly takes the lock to
/// clone the current snapshot `Arc`, releases the lock, then invokes
/// each handler against the snapshot. The handler-vector lock is
/// therefore never held across handler invocation, so a handler may
/// safely re-enter the bus (`register` or `publish` on the same
/// instance) without deadlocking on the bus's own mutex. The
/// synchronous-fanout contract (CHE-0024:§7) is preserved: handlers
/// still run synchronously inside `publish` before the returned future
/// resolves.
///
/// ## Failure model
///
/// `publish` returns `Result<(), BusError>` to satisfy the port
/// signature. `InProcessEventBus` itself never produces a `BusError` —
/// in-process delivery has no fallible transport. Failed *handler*
/// dispatch (panics) would propagate; per CHE-0051:R7, dead-letter
/// routing of failed *policy outputs* is `App`'s job (S5/S6), not the
/// bus's.
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
    /// registration method — the `cherry_pit_core::EventBus` port
    /// trait deliberately does NOT carry `subscribe` / `register`.
    /// Subscription is "inherently implementation-specific (in-process
    /// channels vs NATS subjects vs polling)" (CHE-0024:R2 commentary).
    ///
    /// Handlers must be `Send + Sync + 'static` because the bus itself
    /// is `Send + Sync + 'static` (the `EventBus` port requires it) and
    /// publication may happen across threads (e.g. tokio multi-threaded
    /// runtime).
    ///
    /// Registration is **copy-on-write**: the current handler vector is
    /// cloned, the new handler is appended to the clone, and the bus
    /// swaps the snapshot `Arc` under a brief lock. Existing snapshots
    /// observed by an in-flight `publish` continue to fire over the
    /// pre-registration handler set; subsequent publishes observe the
    /// new handler. This is the mechanism that makes reentrant
    /// `register` from inside a handler safe (no deadlock on the bus's
    /// own mutex).
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

    /// Publish synchronously to every registered handler in registration
    /// order, then return `Ok(())`.
    ///
    /// Per CHE-0024:§7: "In-process delivery is synchronous within
    /// `publish` — the bus calls each registered handler before
    /// returning." The returned `impl Future` resolves immediately on
    /// the calling task; no work is spawned.
    ///
    /// Per CHE-0024:R1: persist-then-publish ordering and the
    /// non-fatality of publication failure are enforced at the
    /// `CommandBus` / `App` level (S5), not here. This impl never
    /// returns `Err(BusError)`.
    ///
    /// # Hazards
    ///
    /// **Handlers run synchronously inside `publish` — no awaiting on
    /// publisher-held locks (CHE-0051:R8 advisory).** Each registered
    /// handler is invoked from the body of this method, but the
    /// handler-vector mutex is released *before* fan-out begins: the
    /// snapshot `Arc<Vec<…>>` of handlers is cloned under a brief lock
    /// and then the guard is dropped, so handler bodies never run with
    /// the bus's own mutex held. Reentrant `register` / `publish` from
    /// within a handler is therefore deadlock-safe at the bus boundary.
    ///
    /// The advisory continues to apply at the *publisher* boundary:
    /// when a handler — such as the `App::run` callback — bridges into
    /// async via `Handle::block_on(...)`, any future driven by that
    /// `block_on` that tries to acquire a lock the *publisher* holds
    /// will still deadlock. See `App::run`'s `# Hazards` section.
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
        // No panic, no error, no handler invocation possible.
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

        // 3 handlers × 2 envelopes = 6 invocations, ordered envelope-major.
        let log = recorded.lock().unwrap().clone();
        assert_eq!(log.len(), 6);
        assert_eq!(
            log,
            vec![(0, 10), (1, 10), (2, 10), (0, 20), (1, 20), (2, 20)]
        );
    }
}
