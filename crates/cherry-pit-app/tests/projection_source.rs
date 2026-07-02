use std::collections::HashMap;

use cherry_pit_app::InMemoryProjectionSource;
use cherry_pit_core::{DomainEvent, EventEnvelope, Projection};
use cherry_pit_projection::InMemoryProjection;
use cherry_pit_web::{PageEntry, ProjectionSource};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast::error::TryRecvError;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PageEvent;

impl DomainEvent for PageEvent {
    fn event_type(&self) -> &'static str {
        "page.changed"
    }
}

#[derive(Default)]
struct PageView {
    pages: Vec<RenderedPage>,
}

struct RenderedPage {
    key: String,
    filename: String,
    body: Vec<u8>,
}

impl Projection for PageView {
    type Event = PageEvent;

    fn apply(&mut self, _event: &EventEnvelope<Self::Event>) {}
}

fn render_pages(view: &PageView) -> HashMap<String, PageEntry> {
    view.pages
        .iter()
        .map(|page| {
            (
                page.key.clone(),
                PageEntry::new(&page.filename, page.body.clone()),
            )
        })
        .collect()
}

#[test]
fn memory_projection_source_renders_domain_view_into_page_entries() {
    let mut store = InMemoryProjection::<PageView>::new();
    store.replace(PageView {
        pages: vec![
            RenderedPage {
                key: "index.html".to_owned(),
                filename: "index.html".to_owned(),
                body: b"<h1>Hello</h1>".to_vec(),
            },
            RenderedPage {
                key: "api/data.json".to_owned(),
                filename: "data.json".to_owned(),
                body: br#"{"ok":true}"#.to_vec(),
            },
        ],
    });

    let source = InMemoryProjectionSource::new(store, render_pages);

    assert!(
        source.is_ready(),
        "populated projection source must be ready"
    );
    let snapshot = source
        .snapshot()
        .expect("populated projection source must expose a snapshot");

    let index = snapshot
        .get("index.html")
        .expect("index page must render into the PageEntry map");
    assert_eq!(index.body.as_ref(), b"<h1>Hello</h1>");
    assert_eq!(
        index.content_type.to_str().unwrap(),
        "text/html; charset=utf-8"
    );

    let data = snapshot
        .get("api/data.json")
        .expect("json page must render into the PageEntry map");
    assert_eq!(data.body.as_ref(), br#"{"ok":true}"#);
    assert_eq!(
        data.content_type.to_str().unwrap(),
        "application/json; charset=utf-8"
    );

    let mut receiver = source.subscribe();
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}
