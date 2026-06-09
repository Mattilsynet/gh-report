//! M1.3 scrape pipeline — integration test.
//!
//! Mission contract `phase2-v2-m1.3-adrsrv-scrape-1779400000`
//! `success_criteria` + CLOSURE.md § 4 C1 (#3 idempotent ingest,
//! #4 ≥1 event per ADR file).
//!
//! Properties pinned:
//!   1. First scrape against the live workspace corpus emits
//!      `events_emitted >= 1` per ADR file actually parsed.
//!   2. A second scrape against the same store, with the same corpus,
//!      emits zero new events (idempotent via `body_hash`,
//!      AFM-0027:R4).
//!   3. `Vec<AdrId>` reference order is preserved verbatim, including
//!      duplicates — projected straight from `AdrRecord.relationships`
//!      with `verb == References` filter, no dedup, no sort.
//!   4. `new_with_replay` rebuilds `adrs_by_id` from store so a fresh
//!      `AdrService` over an existing store sees existing aggregates
//!      and emits zero events on re-scrape.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use adr_srv::scrape::{ScrapeReport, scrape_corpus};
use adr_srv::{AdrCorpus, AdrId, AdrIngested, AdrService};
use cherry_pit_core::EventStore;
use cherry_pit_gateway::MsgpackFileStore as PardosaFileEventStore;
use tempfile::TempDir;

/// Build a synthetic ADR corpus + `adr-fmt.toml` marker in a tempdir.
/// Returns `(marker_dir, _temp_guard)` where `marker_dir` is the
/// directory containing the marker file (the discovery root).
///
/// Corpus layout:
///   `marker_dir/adr-fmt.toml`         — points `corpus.root="adr"`
///   `marker_dir/adr/afm/AFM-0001-...md`
///   `marker_dir/adr/afm/AFM-0002-...md`  (References AFM-0001, AFM-0003, AFM-0001 → duplicate-preservation case)
///   `marker_dir/adr/stale/`            — empty stale archive
fn build_synthetic_corpus() -> (PathBuf, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let marker_dir = tmp.path().to_path_buf();

    let toml = r#"
[corpus]
root = "adr"

[stale]
directory = "stale"

[[domains]]
prefix = "AFM"
name = "Test Domain"
directory = "afm"
description = "synthetic test domain"
crates = []
foundation = false
"#;
    fs::write(marker_dir.join("adr-fmt.toml"), toml).expect("write toml");

    let afm_dir = marker_dir.join("adr").join("afm");
    fs::create_dir_all(&afm_dir).expect("mkdir afm");
    fs::create_dir_all(marker_dir.join("adr").join("stale")).expect("mkdir stale");

    let afm0001 = r"# AFM-0001. First Test ADR

Date: 2026-05-19
Last-reviewed: 2026-05-19
Tier: S
Status: Accepted

## Related

Root: AFM-0001

## Context

Synthetic test ADR for the scrape-pipeline integration test.

## Decision

R1 [5]: Synthetic decision rule for the test.

## Consequences

+ becomes easier: testing.
- becomes harder: nothing.
";
    fs::write(afm_dir.join("AFM-0001-first-test-adr.md"), afm0001).expect("write 0001");

    let afm0002 = r"# AFM-0002. Second Test ADR

Date: 2026-05-19
Last-reviewed: 2026-05-19
Tier: A
Status: Accepted

## Related

References: AFM-0001, AFM-0003, AFM-0001

## Context

Synthetic ADR exercising the reference-order-and-duplicate
preservation property in the scrape projection.

## Decision

R1 [4]: Synthetic.

## Consequences

+ becomes easier: tested.
- becomes harder: nothing.
";
    fs::write(afm_dir.join("AFM-0002-second-test-adr.md"), afm0002).expect("write 0002");

    (marker_dir, tmp)
}

#[tokio::test]
async fn first_scrape_emits_one_event_per_adr_file() {
    let (marker_dir, _guard) = build_synthetic_corpus();
    let store_dir = TempDir::new().expect("store tempdir");
    let store: PardosaFileEventStore<AdrIngested> = PardosaFileEventStore::new(store_dir.path());
    let service = AdrService::new(Arc::new(store));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));

    let report: ScrapeReport = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape ok");

    assert!(
        report.events_emitted >= 2,
        "expected >= 2 events, got {} (records_seen={})",
        report.events_emitted,
        report.records_seen,
    );
    assert_eq!(
        report.records_seen, 2,
        "synthetic corpus has exactly 2 ADR files",
    );
}

#[tokio::test]
async fn second_scrape_emits_zero_events_unchanged_corpus() {
    let (marker_dir, _guard) = build_synthetic_corpus();
    let store_dir = TempDir::new().expect("store tempdir");
    let store: PardosaFileEventStore<AdrIngested> = PardosaFileEventStore::new(store_dir.path());
    let service = AdrService::new(Arc::new(store));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));

    let first = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape 1");
    assert!(first.events_emitted >= 2, "first scrape emits");

    let second = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape 2");
    assert_eq!(
        second.events_emitted, 0,
        "second scrape on unchanged corpus must emit 0 events (AFM-0027:R4); got {}",
        second.events_emitted,
    );
    assert_eq!(
        second.records_seen, 2,
        "records_seen still counts files walked even when no events emitted",
    );
}

#[tokio::test]
async fn references_preserve_order_and_duplicates() {
    let (marker_dir, _guard) = build_synthetic_corpus();
    let store_dir = TempDir::new().expect("store tempdir");
    let store: Arc<PardosaFileEventStore<AdrIngested>> =
        Arc::new(PardosaFileEventStore::new(store_dir.path()));
    let service = AdrService::new(Arc::clone(&store));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));

    let _ = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape");

    let afm0002 = AdrId::from_str("AFM-0002").expect("AFM-0002 parses");
    let agg_id = service
        .lookup(&afm0002)
        .expect("AFM-0002 must be indexed after scrape");
    let envelopes = store.load(agg_id).await.expect("load aggregate");
    assert_eq!(
        envelopes.len(),
        1,
        "AFM-0002 has exactly one ingested event"
    );
    let event = envelopes[0].payload();

    let expected = vec![
        AdrId::from_str("AFM-0001").expect("parse"),
        AdrId::from_str("AFM-0003").expect("parse"),
        AdrId::from_str("AFM-0001").expect("parse"),
    ];
    assert_eq!(
        event.references, expected,
        "references must preserve source order including duplicates (Q3 case)",
    );
}

#[tokio::test]
async fn replay_on_boot_rebuilds_index_so_re_scrape_is_idempotent() {
    let (marker_dir, _guard) = build_synthetic_corpus();
    let store_dir = TempDir::new().expect("store tempdir");

    {
        let store: PardosaFileEventStore<AdrIngested> =
            PardosaFileEventStore::new(store_dir.path());
        let service = AdrService::new(Arc::new(store));
        let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
        let r = scrape_corpus(&service, &marker_dir, &corpus)
            .await
            .expect("scrape 1");
        assert!(r.events_emitted >= 2);
    }

    let store2: PardosaFileEventStore<AdrIngested> = PardosaFileEventStore::new(store_dir.path());
    let corpus2: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    let service2 = AdrService::new_with_replay(Arc::new(store2), &corpus2)
        .await
        .expect("new_with_replay ok");

    let r2 = scrape_corpus(&service2, &marker_dir, &corpus2)
        .await
        .expect("scrape 2");
    assert_eq!(
        r2.events_emitted, 0,
        "after replay-on-boot, re-scrape must be idempotent; got {}",
        r2.events_emitted,
    );
}

#[tokio::test]
async fn changed_body_emits_new_event() {
    let (marker_dir, _guard) = build_synthetic_corpus();
    let store_dir = TempDir::new().expect("store tempdir");
    let store: PardosaFileEventStore<AdrIngested> = PardosaFileEventStore::new(store_dir.path());
    let service = AdrService::new(Arc::new(store));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));

    let first = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape 1");
    assert!(first.events_emitted >= 2);

    let p = marker_dir
        .join("adr")
        .join("afm")
        .join("AFM-0001-first-test-adr.md");
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&p)
        .expect("open for append");
    writeln!(f, "\nAppended sentence to change body hash.").expect("append");
    drop(f);

    let second = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape 2");
    assert_eq!(
        second.events_emitted, 1,
        "exactly one ADR changed → exactly one new event; got {}",
        second.events_emitted,
    );
}
