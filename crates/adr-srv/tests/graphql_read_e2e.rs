//! M1.4 end-to-end: GraphQL Query read-side over `AdrCorpus`.
//!
//! Mission contract `phase2-v2-m1.4-adrsrv-graphql-1779400000`
//! `success_criteria` #3-4 (≥5 new tests, full suite green) +
//! CLOSURE.md § 4 C1 #2 (canonical query returns the projected ADR).
//!
//! Tests exercise the `Query` resolver path via `schema.execute()`
//! — no HTTP layer. The `/graphql` HTTP wiring is a separate smoke
//! check (manual `curl` per brief verify_commands). Per the brief's
//! TDD-discipline ask, each test was first observed failing on a
//! known-bad resolver mutation before being committed; mutations
//! and observations are recorded in the M1.4 mission journal.

#![allow(clippy::doc_markdown, clippy::needless_raw_string_hashes)]

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use adr_srv::scrape::scrape_corpus;
use adr_srv::{AdrCorpus, AdrIngested, AdrService, build_schema};
use async_graphql::{Request, Value, from_value};
use cherry_pit_gateway::MsgpackFileStore as PardosaFileEventStore;
use serde::Deserialize;
use tempfile::TempDir;

/// Build a synthetic corpus with AFM-0001 (References AFM-0002,
/// AFM-0003 in that order) plus AFM-0002 and AFM-0003 as plain
/// ADRs. Returns the marker dir and the tempdir guard.
fn build_corpus() -> (PathBuf, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let marker_dir = tmp.path().to_path_buf();
    let toml = r#"
[corpus]
root = "adr"

[stale]
directory = "stale"

[[domains]]
prefix = "AFM"
name = "Test"
directory = "afm"
description = "synthetic"
crates = []
foundation = false
"#;
    fs::write(marker_dir.join("adr-fmt.toml"), toml).expect("toml");
    let afm = marker_dir.join("adr").join("afm");
    fs::create_dir_all(&afm).expect("afm dir");
    fs::create_dir_all(marker_dir.join("adr").join("stale")).expect("stale dir");

    fs::write(
        afm.join("AFM-0001-root.md"),
        "# AFM-0001. Root ADR\n\nDate: 2026-05-19\nLast-reviewed: 2026-05-19\nTier: S\nStatus: Accepted\n\n## Related\n\nReferences: AFM-0002, AFM-0003\n\n## Context\n\nRoot.\n\n## Decision\n\nR1 [5]: x.\n\n## Consequences\n\n+ a.\n- b.\n",
    )
    .expect("write 0001");
    fs::write(
        afm.join("AFM-0002-second.md"),
        "# AFM-0002. Second ADR\n\nDate: 2026-05-19\nLast-reviewed: 2026-05-19\nTier: A\nStatus: Accepted\n\n## Related\n\nRoot: AFM-0001\n\n## Context\n\nLeaf.\n\n## Decision\n\nR1 [4]: y.\n\n## Consequences\n\n+ a.\n- b.\n",
    )
    .expect("write 0002");
    fs::write(
        afm.join("AFM-0003-third.md"),
        "# AFM-0003. Third ADR\n\nDate: 2026-05-19\nLast-reviewed: 2026-05-19\nTier: B\nStatus: Proposed\n\n## Related\n\nRoot: AFM-0001\n\n## Context\n\nLeaf.\n\n## Decision\n\nR1 [3]: z.\n\n## Consequences\n\n+ a.\n- b.\n",
    )
    .expect("write 0003");

    (marker_dir, tmp)
}

/// Spin up service + corpus, scrape once. Returns the corpus mutex
/// and the temp guards so the caller can build a schema and
/// .execute() against it.
async fn scrape_and_corpus() -> (Arc<Mutex<AdrCorpus>>, TempDir, TempDir) {
    let (marker_dir, tmp_corpus) = build_corpus();
    let store_tmp = TempDir::new().expect("store tempdir");
    let store: PardosaFileEventStore<AdrIngested> = PardosaFileEventStore::new(store_tmp.path());
    let service = AdrService::new(Arc::new(store));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape ok");
    (corpus, tmp_corpus, store_tmp)
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AdrShape {
    id: String,
    title: String,
    date: String,
    last_reviewed: String,
    tier: String,
    status: String,
    body_hash: String,
    references: Vec<RefShape>,
}

#[derive(Deserialize, Debug)]
struct RefShape {
    id: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AdrByIdData {
    adr_by_id: Option<AdrShape>,
}

#[derive(Deserialize, Debug)]
struct AdrIdTitle {
    id: String,
    #[allow(dead_code)]
    title: String,
}

#[derive(Deserialize, Debug)]
struct AdrIdOnly {
    id: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AllAdrsData {
    all_adrs: Vec<AdrIdTitle>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct AdrsByDomainData {
    adrs_by_domain: Vec<AdrIdOnly>,
}

/// CLOSURE.md C1 #2 canonical query: `{ adrByID(id: "AFM-0001") {
/// id title references { id } } }`. Asserts shape on a freshly
/// scraped corpus.
#[tokio::test]
async fn adr_by_id_returns_scraped_adr_with_references_in_source_order() {
    let (corpus, _g1, _g2) = scrape_and_corpus().await;
    let schema = build_schema(Arc::clone(&corpus));

    let q = r#"{ adrById(id: "AFM-0001") { id title date lastReviewed tier status bodyHash references { id } } }"#;
    let resp = schema.execute(Request::new(q)).await;
    assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);

    let data: AdrByIdData = from_value(Value::from_json(resp.data.into_json().unwrap()).unwrap())
        .expect("deserialise data");
    let adr = data.adr_by_id.expect("AFM-0001 must be present");

    assert_eq!(adr.id, "AFM-0001");
    assert_eq!(adr.title, "Root ADR");
    assert_eq!(adr.date, "2026-05-19");
    assert_eq!(adr.last_reviewed, "2026-05-19");
    assert_eq!(adr.tier, "S");
    assert_eq!(adr.status, "Accepted");
    assert_eq!(
        adr.body_hash.len(),
        32,
        "body_hash is 32-char lowercase hex (xxh3-128)"
    );
    assert!(adr.body_hash.chars().all(|c| c.is_ascii_hexdigit()));

    let ref_ids: Vec<&str> = adr.references.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        ref_ids,
        vec!["AFM-0002", "AFM-0003"],
        "references in source order"
    );
}

/// Unknown id resolves to GraphQL null without error.
#[tokio::test]
async fn adr_by_id_returns_null_for_unknown_adr() {
    let (corpus, _g1, _g2) = scrape_and_corpus().await;
    let schema = build_schema(Arc::clone(&corpus));

    let resp = schema
        .execute(Request::new(r#"{ adrById(id: "AFM-9999") { id } }"#))
        .await;
    assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);

    let data: AdrByIdData =
        from_value(Value::from_json(resp.data.into_json().unwrap()).unwrap()).expect("deserialise");
    assert!(data.adr_by_id.is_none(), "unknown id must resolve to null");
}

/// Unparseable id (not in canonical form) also resolves to null, no
/// error (resolver swallows the parse failure into None).
#[tokio::test]
async fn adr_by_id_returns_null_for_unparseable_id() {
    let (corpus, _g1, _g2) = scrape_and_corpus().await;
    let schema = build_schema(Arc::clone(&corpus));

    let resp = schema
        .execute(Request::new(r#"{ adrById(id: "not an adr id") { id } }"#))
        .await;
    assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
    let data: AdrByIdData =
        from_value(Value::from_json(resp.data.into_json().unwrap()).unwrap()).expect("deserialise");
    assert!(data.adr_by_id.is_none());
}

/// `allAdrs` returns every ADR in BTreeMap key order (domain then
/// numeric) regardless of scrape order.
#[tokio::test]
async fn all_adrs_lists_every_adr_in_btreemap_order() {
    let (corpus, _g1, _g2) = scrape_and_corpus().await;
    let schema = build_schema(Arc::clone(&corpus));

    let resp = schema
        .execute(Request::new(r#"{ allAdrs { id title } }"#))
        .await;
    assert!(resp.errors.is_empty(), "errors: {:?}", resp.errors);
    let data: AllAdrsData =
        from_value(Value::from_json(resp.data.into_json().unwrap()).unwrap()).expect("deserialise");

    let ids: Vec<&str> = data.all_adrs.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["AFM-0001", "AFM-0002", "AFM-0003"],
        "allAdrs returns BTreeMap-ordered ids"
    );
}

/// `adrsByDomain("AFM")` returns the same set as `allAdrs` for this
/// single-domain corpus; `adrsByDomain("CHE")` returns empty.
#[tokio::test]
async fn adrs_by_domain_filters_by_prefix() {
    let (corpus, _g1, _g2) = scrape_and_corpus().await;
    let schema = build_schema(Arc::clone(&corpus));

    let resp_afm = schema
        .execute(Request::new(r#"{ adrsByDomain(domain: "AFM") { id } }"#))
        .await;
    assert!(resp_afm.errors.is_empty(), "errors: {:?}", resp_afm.errors);
    let data_afm: AdrsByDomainData =
        from_value(Value::from_json(resp_afm.data.into_json().unwrap()).unwrap())
            .expect("deserialise AFM");
    let ids: Vec<&str> = data_afm
        .adrs_by_domain
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(ids, vec!["AFM-0001", "AFM-0002", "AFM-0003"]);

    let resp_che = schema
        .execute(Request::new(r#"{ adrsByDomain(domain: "CHE") { id } }"#))
        .await;
    assert!(resp_che.errors.is_empty(), "errors: {:?}", resp_che.errors);
    let data_che: AdrsByDomainData =
        from_value(Value::from_json(resp_che.data.into_json().unwrap()).unwrap())
            .expect("deserialise CHE");
    assert!(
        data_che.adrs_by_domain.is_empty(),
        "no CHE ADRs in this corpus"
    );
}

/// Mutating an ADR file and re-scraping flows through the projection:
/// the second `adrById` returns a different body_hash than the first.
/// Proves AdrCorpus tracks the latest event, not a stale boot snapshot.
#[tokio::test]
async fn projection_reflects_body_mutation_on_rescrape() {
    let (marker_dir, _tmp_corpus) = build_corpus();
    let store_tmp = TempDir::new().expect("store tempdir");
    let store: PardosaFileEventStore<AdrIngested> = PardosaFileEventStore::new(store_tmp.path());
    let service = AdrService::new(Arc::new(store));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape 1");

    let schema = build_schema(Arc::clone(&corpus));
    let resp1 = schema
        .execute(Request::new(r#"{ adrById(id: "AFM-0001") { bodyHash } }"#))
        .await;
    assert!(resp1.errors.is_empty());
    let h1: serde_json::Value = resp1.data.into_json().unwrap();
    let hash1 = h1["adrById"]["bodyHash"].as_str().unwrap().to_string();

    let p = marker_dir.join("adr").join("afm").join("AFM-0001-root.md");
    let mut body = fs::read_to_string(&p).expect("read");
    body.push_str("\nMutation appended.\n");
    fs::write(&p, body).expect("rewrite");

    scrape_corpus(&service, &marker_dir, &corpus)
        .await
        .expect("scrape 2");

    let resp2 = schema
        .execute(Request::new(r#"{ adrById(id: "AFM-0001") { bodyHash } }"#))
        .await;
    assert!(resp2.errors.is_empty());
    let h2: serde_json::Value = resp2.data.into_json().unwrap();
    let hash2 = h2["adrById"]["bodyHash"].as_str().unwrap().to_string();

    assert_ne!(
        hash1, hash2,
        "body_hash must change after corpus mutation; projection tracks latest event"
    );
}
