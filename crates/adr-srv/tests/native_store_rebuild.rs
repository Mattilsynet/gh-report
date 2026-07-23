//! CHE-0098 M2 hard-cut rebuild proof (guardrail evidence).
//!
//! adr-srv's event log is a DERIVED PROJECTION of the `docs/adr/`
//! markdown corpus (AFM-0027) — the corpus, not the store, is the
//! real source of truth. This test demonstrates that starting from an
//! EMPTY native `.pgno` store, `AdrService::new_with_replay` (N-R5
//! resume) plus one `scrape_corpus` pass (the re-scan) reconstructs
//! the full aggregate set from the markdown corpus alone.

use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use adr_srv::scrape::scrape_corpus;
use adr_srv::{AdrCorpus, AdrId, AdrService, NativeAdrStore};
use tempfile::TempDir;

/// Same synthetic two-ADR corpus shape as `scrape_pipeline.rs`
/// (AFM-0001, AFM-0002 referencing AFM-0001/AFM-0003/AFM-0001).
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

Synthetic test ADR for the native store rebuild-from-corpus test.

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
preservation property.

## Decision

R1 [4]: Synthetic.

## Consequences

+ becomes easier: tested.
- becomes harder: nothing.
";
    fs::write(afm_dir.join("AFM-0002-second-test-adr.md"), afm0002).expect("write 0002");

    (marker_dir, tmp)
}

/// N-R5 evidence + epic hard-cut guardrail clearance: an EMPTY native
/// `.pgno` store, boot-replayed (0 streams) then re-scraped, ends up
/// with the same aggregate set a fresh corpus scrape produces — the
/// corpus re-scan alone reconstructs adr-srv state, independent of
/// any prior store contents.
#[tokio::test]
async fn rebuild_from_markdown_corpus_reconstructs_aggregates() {
    let (marker_dir, _guard) = build_synthetic_corpus();
    let store_dir = TempDir::new().expect("store tempdir");
    let store_path = store_dir.path().join("adr-srv.pgno");

    let store = Arc::new(create_empty_native_store(&store_path));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));

    let service = replay_zero_streams_from_empty_store(&store, &corpus).await;
    assert!(
        corpus.lock().expect("corpus mutex").is_empty(),
        "empty store must replay to an empty corpus"
    );

    let report = scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("corpus re-scan succeeds");
    assert_eq!(
        report.events_emitted, 2,
        "both synthetic ADR files must ingest as fresh events"
    );

    let afm0001 = AdrId::from_str("AFM-0001").expect("parses");
    let afm0002 = AdrId::from_str("AFM-0002").expect("parses");
    {
        let guard = corpus.lock().expect("corpus mutex");
        assert_eq!(guard.len(), 2, "both ADRs reconstructed into the corpus");
        assert!(guard.get(&afm0001).is_some(), "AFM-0001 reconstructed");
        let doc2 = guard.get(&afm0002).expect("AFM-0002 reconstructed");
        assert_eq!(
            doc2.references,
            vec![
                AdrId::from_str("AFM-0001").expect("parse"),
                AdrId::from_str("AFM-0003").expect("parse"),
                AdrId::from_str("AFM-0001").expect("parse"),
            ],
            "reference order/duplicates preserved through the native store round-trip"
        );
    }

    drop(service);
    reopen_and_replay_populated_store_reconstructs_without_rescrape(&store_path).await;
}

/// Nothing durable exists yet: a fresh, empty `.pgno` container.
fn create_empty_native_store(store_path: &std::path::Path) -> NativeAdrStore {
    NativeAdrStore::create_pgno(store_path).expect("create empty store")
}

async fn replay_zero_streams_from_empty_store(
    store: &Arc<NativeAdrStore>,
    corpus: &Arc<Mutex<AdrCorpus>>,
) -> AdrService<NativeAdrStore> {
    AdrService::new_with_replay(Arc::clone(store), corpus)
        .await
        .expect("replay of empty store succeeds")
}

/// Reopening the same on-disk store and replaying again (no
/// re-scrape) independently reconstructs the same aggregate set
/// purely from the persisted native events — the store itself, not
/// just the corpus re-scan, durably holds what N-R5 resume needs.
async fn reopen_and_replay_populated_store_reconstructs_without_rescrape(
    store_path: &std::path::Path,
) {
    let store2 = Arc::new(NativeAdrStore::open_pgno(store_path).expect("reopen store"));
    let corpus2: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    let _service2 = AdrService::new_with_replay(Arc::clone(&store2), &corpus2)
        .await
        .expect("replay of populated store succeeds");
    let guard2 = corpus2.lock().expect("corpus2 mutex");
    assert_eq!(
        guard2.len(),
        2,
        "reopening the .pgno store and replaying alone (no re-scrape) reconstructs both aggregates"
    );
}
