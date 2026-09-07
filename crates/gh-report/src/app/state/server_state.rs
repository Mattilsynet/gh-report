//! `ServerState` trait wiring for `AppState`, consumed by
//! `cherry_pit_web::serve`. Extracted from `state.rs` (K9, ghr-ff7dd746)
//! as a pure structural move — no behavioural change.

use std::collections::HashMap;

use arc_swap::ArcSwap;

use super::{AppState, CachedPage, PageUpdateEvent};

impl cherry_pit_web::serve::ServerState for AppState {
    fn html_cache(&self) -> &ArcSwap<Option<HashMap<String, CachedPage>>> {
        &self.evidence.html_cache
    }

    fn ws_broadcast(&self) -> &tokio::sync::broadcast::Sender<PageUpdateEvent> {
        &self.evidence.ws_broadcast
    }

    fn is_ready(&self) -> bool {
        self.event_store.backend_reachable()
            && self.org_event_store.backend_reachable()
            && self.team_event_store.backend_reachable()
            && self
                .evidence
                .html_cache
                .load()
                .as_ref()
                .as_ref()
                .is_some_and(|pages| pages.contains_key("index.html"))
    }
}
