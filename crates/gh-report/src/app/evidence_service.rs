//! Evidence data store and publication infrastructure.
//!
//! Extracted from [`AppState`] as part of the Phase 2 decomposition.
//! Groups the five fields related to evidence storage, HTML caching,
//! WebSocket broadcasting, org-level alert summaries, and batch tracking.
//!
//! [`AppState`]: super::state::AppState

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::app::work_queue::BatchTracker;
use crate::domain::metrics::OrgAlertSummary;
use crate::infra::server::state::{CachedPage, PageUpdateEvent};

/// Evidence service sub-aggregate.
///
/// Holds the in-memory HTML page cache (swapped atomically after each
/// collection), the WebSocket broadcast channel, the org-level alert
/// summary, and the batch tracker.
pub struct EvidenceState {
    /// In-memory HTML page cache, swapped atomically after each collection.
    ///
    /// `None` → no collection has completed yet (server returns 503).
    /// `Some(map)` → cache key is the relative path (e.g. `"index.html"`,
    /// `"report.html"`).
    pub(crate) html_cache: ArcSwap<Option<HashMap<String, CachedPage>>>,

    /// Broadcast channel for notifying connected WebSocket clients of page
    /// updates. Each WebSocket handler subscribes via `.subscribe()`.
    ///
    /// Capacity bounds per-subscriber buffer: if a browser lags behind 64
    /// updates, its receiver gets `RecvError::Lagged` and the handler sends
    /// a full-reload signal. The `_rx` is immediately dropped — new
    /// receivers are created per WebSocket connection.
    pub(crate) ws_broadcast: tokio::sync::broadcast::Sender<PageUpdateEvent>,

    /// Org-level alert summary (secret scanning). Updated by the sweep,
    /// read by webhook-triggered evaluations via eventual consistency (AD6).
    pub(crate) org_summary: Arc<ArcSwap<Option<Arc<OrgAlertSummary>>>>,

    /// Active batch tracker for the current sweep. The delivery task calls
    /// `complete_one()` for each `ScheduledBatch` outcome. Set by the sweep,
    /// cleared when the batch completes.
    pub(crate) batch_tracker: ArcSwap<Option<Arc<BatchTracker>>>,
}

impl EvidenceState {
    /// Create a production `EvidenceState`.
    pub(crate) fn new() -> Self {
        let (ws_broadcast, _) = tokio::sync::broadcast::channel::<PageUpdateEvent>(64);
        Self {
            html_cache: ArcSwap::from_pointee(None),
            ws_broadcast,
            org_summary: Arc::new(ArcSwap::from_pointee(None)),
            batch_tracker: ArcSwap::from_pointee(None),
        }
    }
}
