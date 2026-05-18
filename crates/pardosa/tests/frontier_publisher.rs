//! F2e integration test: Dragline frontier publisher.
//!
//! Verifies PAR-0021:R4 — frontier published to `pardosa.{stream}.frontier`
//! on every `anchor_interval` tick via the `FrontierPublisher` trait.
//!
//! Uses `InMemoryFrontierPublisher` (no real NATS broker; no `async-nats` dep).

use pardosa::Dragline;
use pardosa::frontier::InMemoryFrontierPublisher;

// ── helpers ──────────────────────────────────────────────────────────────────

fn build_dragline_with_mock(
    stream: &str,
    interval: u64,
) -> (Dragline<String>, InMemoryFrontierPublisher) {
    let publisher = InMemoryFrontierPublisher::new();
    let d = Dragline::<String>::with_publisher(stream.to_owned(), interval, publisher.clone());
    (d, publisher)
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Publish fires on every `anchor_interval`-th commit.
#[test]
fn frontier_published_on_anchor_interval_tick() {
    let interval = 3u64;
    let (mut d, mock) = build_dragline_with_mock("alpha", interval);

    // 3 events → 1 tick
    let r = d.create(1000, "a".to_owned()).unwrap();
    d.update(r.domain_id, 1001, "a2".to_owned()).unwrap();
    d.create(1002, "b".to_owned()).unwrap();

    let pubs = mock.published();
    assert_eq!(
        pubs.len(),
        1,
        "expected exactly 1 publish after {interval} events"
    );

    let (subject, payload) = &pubs[0];
    assert_eq!(subject, "pardosa.alpha.frontier");
    assert_eq!(payload.len(), 32, "frontier payload must be 32 bytes");
}

/// Payload matches `Dragline::frontier()` at time of publish.
#[test]
fn published_payload_matches_frontier_value() {
    let interval = 2u64;
    let (mut d, mock) = build_dragline_with_mock("beta", interval);

    let r = d.create(1, "x".to_owned()).unwrap();
    d.update(r.domain_id, 2, "y".to_owned()).unwrap();

    let pubs = mock.published();
    assert_eq!(pubs.len(), 1);
    let frontier = d.frontier();
    assert_ne!(
        frontier, [0u8; 32],
        "frontier must be non-zero after commits"
    );
    assert_eq!(pubs[0].1, frontier.to_vec());
}

/// No publish before `anchor_interval` is reached.
#[test]
fn no_publish_before_interval() {
    let interval = 5u64;
    let (mut d, mock) = build_dragline_with_mock("gamma", interval);

    d.create(1, "a".to_owned()).unwrap();
    d.create(2, "b".to_owned()).unwrap();

    assert!(
        mock.published().is_empty(),
        "must not publish before interval is reached"
    );
}

/// Multiple ticks accumulate multiple publishes.
#[test]
fn multiple_ticks_accumulate_publishes() {
    let interval = 2u64;
    let (mut d, mock) = build_dragline_with_mock("delta", interval);

    for i in 0u64..6 {
        d.create(i as i64, format!("e{i}")).unwrap();
    }

    let pubs = mock.published();
    assert_eq!(pubs.len(), 3, "6 events / interval 2 = 3 publishes");
    for (subject, _) in &pubs {
        assert_eq!(subject, "pardosa.delta.frontier");
    }
}

/// Dragline created with `new()` (no publisher) still commits without panic.
#[test]
fn dragline_new_commits_without_publisher() {
    let mut d = Dragline::<String>::new();
    d.create(0, "a".to_owned()).unwrap();
    // No panic; no publisher installed.
}
