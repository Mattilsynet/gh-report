//! Evidence data store and publication infrastructure.
//!
//! Extracted from [`AppState`] as part of the Phase 2 decomposition.
//! Groups the five fields related to evidence storage, HTML caching,
//! WebSocket broadcasting, org-level alert summaries, and batch tracking.
//!
//! Also hosts the [`ProjectionSource`] impl that exposes the in-memory
//! HTML cache + WS delta stream to cherry-pit-web's projection adapter
//! (CHE-0049 R11, CHE-0055 §G1). Phase 2 stub wiring: gh-report holds
//! the impl, but no main path mounts `build_projection_router` yet —
//! the trait satisfies the type-only `cherry-pit-web` dep edge so the
//! adapter's generic parameter can resolve when the consumer wires it
//! at a future commit. MC-2.7 removes both this impl and the dep edge
//! when the shim is deleted.
//!
//! [`AppState`]: super::state::AppState
//! [`ProjectionSource`]: cherry_pit_web::ProjectionSource

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use cherry_pit_core::CorrelationContext;
use cherry_pit_web::{PageEntry, PageUpdate, ProjectionSource};
use tokio::sync::broadcast;

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
    pub html_cache: ArcSwap<Option<HashMap<String, CachedPage>>>,

    /// Broadcast channel for notifying connected WebSocket clients of page
    /// updates. Each WebSocket handler subscribes via `.subscribe()`.
    ///
    /// Capacity bounds per-subscriber buffer: if a browser lags behind 64
    /// updates, its receiver gets `RecvError::Lagged` and the handler sends
    /// a full-reload signal. The `_rx` is immediately dropped — new
    /// receivers are created per WebSocket connection.
    pub ws_broadcast: tokio::sync::broadcast::Sender<PageUpdateEvent>,

    /// Org-level alert summary (secret scanning). Updated by the sweep,
    /// read by webhook-triggered evaluations via eventual consistency (AD6).
    pub org_summary: Arc<ArcSwap<Option<Arc<OrgAlertSummary>>>>,

    /// Active batch tracker for the current sweep. The delivery task calls
    /// `complete_one()` for each `ScheduledBatch` outcome. Set by the sweep,
    /// cleared when the batch completes.
    pub batch_tracker: ArcSwap<Option<Arc<BatchTracker>>>,
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

// ── ProjectionSource impl (CHE-0049 R11, CHE-0055 §G1) ──────────────
//
// Bridges the legacy in-memory cache + WS broadcast types
// (`CachedPage`, `PageUpdateEvent`) to the cherry-side migration twins
// (`PageEntry`, `PageUpdate`). Both pairs are field-for-field
// equivalent; `PageEntry`/`PageUpdate` only add a `correlation:
// CorrelationContext` envelope on the WS delta. Phase 2 is a typed
// stub — no main path consumes the resulting router today; M5 Phase 3c
// (cherry side) and a later gh-report commit will mount it.

/// Convert a legacy `CachedPage` into the cherry-side `PageEntry`.
///
/// `PageEntry` is `#[non_exhaustive]`; the only public constructor is
/// `PageEntry::new(filename, body)` which re-computes ETag + zstd. The
/// resulting entry is semantically equivalent to the input but the
/// pre-compression cost is paid again on conversion. Acceptable here
/// because `ProjectionSource::snapshot` is invoked on WS reconnect
/// (CHE-0049 R11 HTTP-snapshot-then-WS-deltas), not per HTTP request
/// on the hot path. If a higher-frequency caller emerges, cache the
/// converted map alongside the source `ArcSwap` keyed by `Arc::ptr_eq`.
fn cached_page_to_page_entry(filename: &str, page: &CachedPage) -> PageEntry {
    PageEntry::new(filename, page.body.to_vec())
}

impl ProjectionSource for EvidenceState {
    fn snapshot(&self) -> Option<Arc<HashMap<String, PageEntry>>> {
        let guard = self.html_cache.load();
        let pages = guard.as_ref().as_ref()?;
        let converted: HashMap<String, PageEntry> = pages
            .iter()
            .map(|(k, v)| (k.clone(), cached_page_to_page_entry(k, v)))
            .collect();
        Some(Arc::new(converted))
    }

    fn subscribe(&self) -> broadcast::Receiver<PageUpdate> {
        // Construct a side broadcast channel on-demand and spawn a
        // transform task that re-broadcasts each `PageUpdateEvent`
        // (legacy) as a `PageUpdate` (cherry) with
        // `CorrelationContext::none()` — the legacy event carries no
        // correlation. The task lives until either:
        //  - the source sender is dropped (no more pages), or
        //  - the returned receiver is dropped (no more subscribers).
        // Per-call channel capacity matches the source (64) so a lagged
        // upstream `RecvError::Lagged` is the bound, not the bridge.
        let mut source_rx = self.ws_broadcast.subscribe();
        let (bridge_tx, bridge_rx) = broadcast::channel::<PageUpdate>(64);
        tokio::spawn(async move {
            loop {
                match source_rx.recv().await {
                    Ok(evt) => {
                        let pages: Vec<String> =
                            evt.pages.iter().map(|s| s.as_ref().to_owned()).collect();
                        let update = PageUpdate::new(
                            pages,
                            evt.repo.as_ref().to_owned(),
                            evt.timestamp.as_ref().to_owned(),
                            CorrelationContext::none(),
                        );
                        if bridge_tx.send(update).is_err() {
                            // No subscribers left — exit transform task.
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Skip the missed batch; downstream sees lag
                        // separately if it lags this bridge channel.
                    }
                }
            }
        });
        bridge_rx
    }

    fn is_ready(&self) -> bool {
        self.html_cache.load().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_source_not_ready_when_cache_empty() {
        let state = EvidenceState::new();
        assert!(!ProjectionSource::is_ready(&state));
        assert!(ProjectionSource::snapshot(&state).is_none());
    }

    #[test]
    fn projection_source_ready_when_cache_populated() {
        let state = EvidenceState::new();
        let mut pages = HashMap::new();
        pages.insert(
            "index.html".to_string(),
            CachedPage::new("index.html", b"<html>hi</html>".to_vec()),
        );
        state.html_cache.store(Arc::new(Some(pages)));
        assert!(ProjectionSource::is_ready(&state));

        let snap = ProjectionSource::snapshot(&state).expect("snapshot should be Some");
        assert_eq!(snap.len(), 1);
        assert!(snap.contains_key("index.html"));
    }

    #[test]
    fn projection_source_snapshot_preserves_keys() {
        let state = EvidenceState::new();
        let mut pages = HashMap::new();
        for name in ["index.html", "report.html", "style.css"] {
            pages.insert(
                name.to_string(),
                CachedPage::new(name, format!("<{name}/>").into_bytes()),
            );
        }
        state.html_cache.store(Arc::new(Some(pages)));
        let snap = ProjectionSource::snapshot(&state).expect("snapshot should be Some");
        assert_eq!(snap.len(), 3);
        for name in ["index.html", "report.html", "style.css"] {
            assert!(snap.contains_key(name), "missing key {name}");
        }
    }

    #[tokio::test]
    async fn projection_source_subscribe_bridges_page_update_events() {
        let state = EvidenceState::new();
        let mut rx = ProjectionSource::subscribe(&state);

        // Send a legacy event on the source channel.
        state
            .ws_broadcast
            .send(PageUpdateEvent::new(
                vec!["index.html".into()],
                "my-repo".into(),
                "2026-04-15T12:00:00Z".into(),
            ))
            .expect("at least one subscriber via bridge");

        // The transform task should forward it as a cherry-side PageUpdate.
        let update = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("recv should not time out")
            .expect("bridge should deliver update");

        assert_eq!(update.repo.as_ref(), "my-repo");
        assert_eq!(update.timestamp.as_ref(), "2026-04-15T12:00:00Z");
        assert_eq!(update.pages.len(), 1);
        assert_eq!(update.pages[0].as_ref(), "index.html");
        // Legacy events carry no correlation; bridge fills with none().
        assert!(update.correlation.correlation_id().is_none());
        assert!(update.correlation.causation_id().is_none());
    }
}
