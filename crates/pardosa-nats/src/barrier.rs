use crate::config::ServerInfoObserver;
use std::future::Future;
use std::pin::Pin;

/// The ordering seam between "a connection was announced" and "the
/// connection's identity is readable".
///
/// `async-nats` announces `Event::Connected` from the connector
/// (`connector.rs:426`) while the server `INFO` for that same connection
/// is published later, by the connection handler
/// (`lib.rs:985`, inside `handle_reconnect`). Reading identity straight
/// off the event therefore races, and loses by returning the *previous*
/// server's identity — the one failure mode that matters here, because a
/// stale answer to "which broker is this?" is worse than no answer.
///
/// [`Self::ordering_barrier`] closes that race by construction rather
/// than by waiting. A round-trip through the client's command channel
/// cannot be serviced until the single-task connection handler returns to
/// its command loop, and it only returns there after `handle_reconnect`
/// has published the new `INFO`. So barrier-completion *happens after*
/// info-publication; the ordering is a property of the channel, not of
/// timing.
///
/// The trait exists so that ordering is testable without a broker: a test
/// double whose barrier publishes the new identity as its side effect
/// fails loudly if the barrier call is ever dropped.
pub trait ConnectionBarrier: Send + Sync {
    /// Complete only once every connection event announced before this
    /// call has been fully processed, including publication of the new
    /// server `INFO`.
    fn ordering_barrier<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + Sync + 'a>>;

    /// The identity of the currently connected broker, or `None` when no
    /// `INFO` has been observed yet.
    ///
    /// Never defaulted: an absent `INFO` must stay absent rather than
    /// becoming an all-empty record reporting `version: ""`.
    fn try_server_info(&self) -> Option<async_nats::ServerInfo>;
}

impl ConnectionBarrier for async_nats::Client {
    fn ordering_barrier<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + Sync + 'a>> {
        Box::pin(async move {
            drop(self.flush().await);
        })
    }

    fn try_server_info(&self) -> Option<async_nats::ServerInfo> {
        Self::try_server_info(self)
    }
}

pub(crate) async fn observe_connection<B, G>(
    barrier: &B,
    observer: &ServerInfoObserver,
    connect_generation: G,
) where
    B: ConnectionBarrier + ?Sized,
    G: Fn() -> u64,
{
    barrier.ordering_barrier().await;
    if let Some(info) = barrier.try_server_info() {
        observer.observe(&info, connect_generation());
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionBarrier, observe_connection};
    use crate::config::ServerInfoObserver;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex, PoisonError};

    /// A broker that has already reconnected to a newer version but has
    /// not yet published the corresponding `INFO`, exactly as
    /// `async-nats` leaves things at the moment `Event::Connected` is
    /// announced. Draining the barrier is what publishes it.
    struct StaleUntilBarrier {
        published: Mutex<Option<async_nats::ServerInfo>>,
        pending: Mutex<Option<async_nats::ServerInfo>>,
    }

    impl StaleUntilBarrier {
        fn new(published: &str, pending: &str) -> Self {
            Self {
                published: Mutex::new(Some(info(published))),
                pending: Mutex::new(Some(info(pending))),
            }
        }
    }

    impl ConnectionBarrier for StaleUntilBarrier {
        fn ordering_barrier<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + Sync + 'a>> {
            Box::pin(async move {
                let pending = self
                    .pending
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(pending) = pending {
                    *self
                        .published
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(pending);
                }
            })
        }

        fn try_server_info(&self) -> Option<async_nats::ServerInfo> {
            self.published
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    struct NeverPublished;

    impl ConnectionBarrier for NeverPublished {
        fn ordering_barrier<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + Sync + 'a>> {
            Box::pin(std::future::ready(()))
        }

        fn try_server_info(&self) -> Option<async_nats::ServerInfo> {
            None
        }
    }

    fn info(version: &str) -> async_nats::ServerInfo {
        async_nats::ServerInfo {
            version: version.to_owned(),
            ..async_nats::ServerInfo::default()
        }
    }

    type Recorded = Arc<Mutex<Vec<(String, u64)>>>;

    fn recording_observer() -> (ServerInfoObserver, Recorded) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let observer = ServerInfoObserver::new(Arc::new(
            move |info: &async_nats::ServerInfo, generation| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((info.version.clone(), generation));
            },
        ));
        (observer, seen)
    }

    #[tokio::test]
    async fn barrier_orders_the_read_after_the_new_info_is_published() {
        let broker = StaleUntilBarrier::new("2.14.5", "2.14.6");
        let (observer, seen) = recording_observer();

        observe_connection(&broker, &observer, || 2).await;

        assert_eq!(
            *seen.lock().unwrap_or_else(PoisonError::into_inner),
            vec![("2.14.6".to_owned(), 2)],
            "without the barrier this reads 2.14.5 -- the previous broker -- \
             and reports the upgrade as if it had not happened"
        );
    }

    #[tokio::test]
    async fn absent_info_after_the_barrier_emits_nothing() {
        let (observer, seen) = recording_observer();

        observe_connection(&NeverPublished, &observer, || 1).await;

        assert!(
            seen.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "an unpublished INFO must stay unpublished, never a defaulted empty record"
        );
    }

    #[tokio::test]
    async fn the_generation_is_read_after_the_barrier_not_before() {
        let broker = StaleUntilBarrier::new("2.14.5", "2.14.6");
        let (observer, seen) = recording_observer();
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(4));
        let probe = Arc::clone(&generation);

        observe_connection(&broker, &observer, move || {
            probe.load(std::sync::atomic::Ordering::Relaxed)
        })
        .await;
        generation.store(9, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(
            *seen.lock().unwrap_or_else(PoisonError::into_inner),
            vec![("2.14.6".to_owned(), 4)],
            "the recorded generation must belong to the connection whose info was read"
        );
    }
}
