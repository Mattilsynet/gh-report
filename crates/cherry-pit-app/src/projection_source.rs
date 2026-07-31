use std::collections::HashMap;
use std::sync::Arc;

use cherry_pit_core::Projection;
use cherry_pit_projection::InMemoryProjection;
use cherry_pit_web::{PageEntry, PageUpdate, ProjectionSource};
use tokio::sync::broadcast;

const DEFAULT_PAGE_UPDATE_CAPACITY: usize = 16;

/// App-side adapter from a `cherry_pit_projection` in-memory view to
/// `cherry_pit_web`'s projection transport port.
///
/// The adapter owns one [`InMemoryProjection`] and a consumer-supplied renderer
/// from the domain projection state into the `HashMap<String, PageEntry>` shape
/// consumed by `cherry-pit-web`. It is generic over the projection state and
/// renderer closure, while the wrapped store type is concrete.
///
/// This adapter is currently snapshot-only: nothing calls `send` on the
/// internal broadcast channel, so [`subscribe`](Self::subscribe) hands back a
/// receiver that never observes a live update. Callers needing current state
/// must poll [`snapshot`](ProjectionSource::snapshot) /
/// [`rendered_snapshot`](Self::rendered_snapshot) instead of awaiting the
/// receiver.
pub struct InMemoryProjectionSource<P, R>
where
    P: Projection,
    R: Fn(&P) -> HashMap<String, PageEntry> + Send + Sync + 'static,
{
    projection: InMemoryProjection<P>,
    render: R,
    updates: broadcast::Sender<PageUpdate>,
}

impl<P, R> InMemoryProjectionSource<P, R>
where
    P: Projection,
    R: Fn(&P) -> HashMap<String, PageEntry> + Send + Sync + 'static,
{
    /// Create a source around an in-memory projection and renderer.
    #[must_use]
    pub fn new(projection: InMemoryProjection<P>, render: R) -> Self {
        let (updates, _) = broadcast::channel(DEFAULT_PAGE_UPDATE_CAPACITY);
        Self {
            projection,
            render,
            updates,
        }
    }

    /// Borrow the wrapped in-memory projection store.
    #[must_use]
    pub const fn projection(&self) -> &InMemoryProjection<P> {
        &self.projection
    }

    /// Render the current domain view into the web `PageEntry` map.
    #[must_use]
    pub fn rendered_snapshot(&self) -> Arc<HashMap<String, PageEntry>> {
        Arc::new((self.render)(self.projection.get()))
    }
}

impl<P, R> ProjectionSource for InMemoryProjectionSource<P, R>
where
    P: Projection,
    R: Fn(&P) -> HashMap<String, PageEntry> + Send + Sync + 'static,
{
    fn snapshot(&self) -> Option<Arc<HashMap<String, PageEntry>>> {
        Some(self.rendered_snapshot())
    }

    /// Returns a receiver on the internal broadcast channel.
    ///
    /// No live updates are currently pushed onto this channel; the returned
    /// receiver will not observe any `PageUpdate` until a producer calls
    /// `send` on the sender, which nothing in this crate does today. Poll
    /// [`snapshot`](ProjectionSource::snapshot) for current state instead of
    /// awaiting this receiver.
    fn subscribe(&self) -> broadcast::Receiver<PageUpdate> {
        self.updates.subscribe()
    }

    fn is_ready(&self) -> bool {
        true
    }
}
