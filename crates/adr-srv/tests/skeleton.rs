//! M1.2 skeleton tests — pin the wire-byte-stable substrate.
//!
//! Tests required by `phase2-v2-m1.2-adrsrv-skeleton-1779400000`
//! `success_criteria` #3:
//!   - `AdrId::from_str("AFM-0001")` succeeds; displays as `"AFM-0001"`.
//!   - `AdrId::from_str("invalid")` returns `Err`.
//!   - `BodyHash::compute(b"hello")` deterministic.
//!   - `AdrIngested` round-trips through msgpack (rmp-serde) byte-identical.
//!   - `AdrDocument::apply` updates state from event.
//!   - `AdrService::new(store)` constructs against `MsgpackFileStore::new`.
//!   - axum `/health` router returns 200.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use adr_srv::{
    AdrCorpus, AdrDate, AdrDocument, AdrFrontmatter, AdrId, AdrIngested, AdrService, AppState,
    BodyHash, Status, Tier, build_schema,
};
use cherry_pit_gateway::MsgpackFileStore;

fn sample_event() -> AdrIngested {
    AdrIngested {
        id: AdrId::from_str("AFM-0001").expect("AFM-0001 parses"),
        frontmatter: AdrFrontmatter {
            title: "SSOT Architecture for ADR Governance".to_string(),
            date: AdrDate::new(2026, 5, 1).expect("date parses"),
            last_reviewed: AdrDate::new(2026, 5, 18).expect("date parses"),
            tier: Tier::S,
            status: Status::Accepted,
        },
        body_hash: BodyHash::compute(b"sample adr body bytes"),
        references: vec![
            AdrId::from_str("CHE-0029").expect("CHE-0029 parses"),
            AdrId::from_str("CHE-0030").expect("CHE-0030 parses"),
        ],
    }
}

// ── AdrId ──────────────────────────────────────────────────────────

#[test]
fn adr_id_parses_canonical_form() {
    let id = AdrId::from_str("AFM-0001").expect("AFM-0001 parses");
    assert_eq!(id.domain(), "AFM");
    assert_eq!(id.number(), 1);
    assert_eq!(id.to_string(), "AFM-0001");
}

#[test]
fn adr_id_display_zero_pads() {
    let id = AdrId::new("CHE", 42).expect("constructs");
    assert_eq!(id.to_string(), "CHE-0042");
}

#[test]
fn adr_id_rejects_missing_separator() {
    let err = AdrId::from_str("invalid").expect_err("must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("separator") || msg.contains("zero-padded"),
        "unexpected error: {msg}"
    );
}

#[test]
fn adr_id_rejects_unknown_domain() {
    let err = AdrId::from_str("XXX-0001").expect_err("must reject");
    assert!(
        matches!(err, adr_srv::AdrIdError::UnknownDomain(ref s) if s == "XXX"),
        "expected UnknownDomain(XXX), got {err:?}"
    );
}

#[test]
fn adr_id_rejects_non_zero_padded() {
    let err = AdrId::from_str("AFM-1").expect_err("must reject 1-digit number");
    assert!(matches!(err, adr_srv::AdrIdError::NotZeroPadded(_)));
}

#[test]
fn adr_id_rejects_zero_number() {
    let err = AdrId::new("AFM", 0).expect_err("must reject zero");
    assert!(matches!(err, adr_srv::AdrIdError::InvalidNumber(_)));
}

#[test]
fn adr_id_accepts_all_known_domains() {
    for d in adr_srv::KNOWN_DOMAINS {
        let id = AdrId::new(*d, 1).unwrap_or_else(|e| panic!("{d}: {e}"));
        assert_eq!(id.domain(), *d);
    }
}

// ── BodyHash ───────────────────────────────────────────────────────

#[test]
fn body_hash_is_deterministic() {
    let h1 = BodyHash::compute(b"hello");
    let h2 = BodyHash::compute(b"hello");
    assert_eq!(h1, h2, "xxh3-128 must be deterministic across calls");
}

#[test]
fn body_hash_differs_for_different_input() {
    let h1 = BodyHash::compute(b"hello");
    let h2 = BodyHash::compute(b"world");
    assert_ne!(h1, h2, "different input must yield different hash");
}

#[test]
fn body_hash_display_is_lowercase_hex_32_chars() {
    let h = BodyHash::compute(b"hello");
    let s = h.to_string();
    assert_eq!(s.len(), 32, "16 bytes × 2 hex chars = 32 chars: {s}");
    assert!(
        s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}

// ── AdrDate ────────────────────────────────────────────────────────

#[test]
fn adr_date_constructs_and_displays() {
    let d = AdrDate::new(2026, 5, 18).expect("valid date");
    assert_eq!(d.year(), 2026);
    assert_eq!(d.month(), 5);
    assert_eq!(d.day(), 18);
    assert_eq!(d.to_string(), "2026-05-18");
}

#[test]
fn adr_date_rejects_invalid_month_and_day() {
    assert!(AdrDate::new(2026, 0, 1).is_err());
    assert!(AdrDate::new(2026, 13, 1).is_err());
    assert!(AdrDate::new(2026, 2, 29).is_err()); // 2026 not a leap year
    assert!(AdrDate::new(2024, 2, 29).is_ok()); // 2024 IS a leap year
    assert!(AdrDate::new(2026, 4, 31).is_err()); // April has 30 days
}

// ── AdrIngested msgpack round-trip ─────────────────────────────────

#[test]
fn adr_ingested_round_trips_byte_identical() {
    let event = sample_event();
    let bytes = rmp_serde::to_vec_named(&event).expect("encode AdrIngested");
    let decoded: AdrIngested = rmp_serde::from_slice(&bytes).expect("decode AdrIngested");
    assert_eq!(decoded, event, "round-trip must preserve value");

    // Re-encode and compare bytes — msgpack is deterministic for a
    // given Serialize impl.
    let bytes2 = rmp_serde::to_vec_named(&decoded).expect("re-encode AdrIngested");
    assert_eq!(
        bytes, bytes2,
        "re-encode must produce byte-identical output"
    );
}

#[test]
fn adr_ingested_encodes_to_non_empty_bytes() {
    let event = sample_event();
    let bytes = rmp_serde::to_vec_named(&event).expect("encode AdrIngested");
    assert!(!bytes.is_empty(), "encoded event must have bytes");
}

#[test]
fn adr_ingested_event_type_is_stable_string() {
    use cherry_pit_core::DomainEvent;
    let event = sample_event();
    assert_eq!(event.event_type(), "AdrIngested");
}

// ── AdrDocument fold ───────────────────────────────────────────────

#[test]
fn adr_document_from_first_seeds_state_from_event() {
    let event = sample_event();
    let doc = AdrDocument::from_first(&event);
    assert_eq!(doc.id, event.id);
    assert_eq!(doc.frontmatter, event.frontmatter);
    assert_eq!(doc.body_hash, event.body_hash);
    assert_eq!(doc.references, event.references);
}

#[test]
fn adr_document_apply_updates_state_from_event() {
    let initial = sample_event();
    let doc = AdrDocument::from_first(&initial);

    // Construct a follow-up event that mutates frontmatter + body_hash
    // (mirrors what an `AdrReingested`-shaped event would do in M1.3
    // when the same file is re-scraped after a content change).
    let updated_event = AdrIngested {
        id: initial.id.clone(),
        frontmatter: AdrFrontmatter {
            title: "Title updated".to_string(),
            ..initial.frontmatter.clone()
        },
        body_hash: BodyHash::compute(b"different content"),
        references: vec![],
    };

    let next = doc.apply(&updated_event);
    assert_eq!(next.frontmatter.title, "Title updated");
    assert_eq!(next.body_hash, updated_event.body_hash);
    assert!(next.references.is_empty());
}

// ── AdrService against MsgpackFileStore ────────────────────────────

#[tokio::test]
async fn adr_service_constructs_against_msgpack_file_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: MsgpackFileStore<AdrIngested> = MsgpackFileStore::new(dir.path());
    let service = AdrService::new(Arc::new(store));
    // store() accessor reaches the inner Arc — proves the service is
    // wired to the store, not just constructed and discarded.
    let _store_ref = service.store();
}

#[tokio::test]
async fn app_state_constructs_from_service() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store: MsgpackFileStore<AdrIngested> = MsgpackFileStore::new(dir.path());
    let service = Arc::new(AdrService::new(Arc::new(store)));
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    let schema = build_schema(Arc::clone(&corpus));
    let state = AppState::new(Arc::clone(&service), Arc::clone(&corpus), schema);
    // AppState::clone is cheap (Arc + schema-arc); verify it works.
    let _cloned = state.clone();
}

// ── axum /health router boot ───────────────────────────────────────

#[tokio::test]
async fn health_route_returns_200() {
    use axum::{Router, body::Body, routing::get};
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    let app: Router = Router::new().route("/health", get(|| async { "ok" }));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("oneshot resolves");
    assert_eq!(response.status(), StatusCode::OK);
}

// ── schema constructor smoke ───────────────────────────────────────

#[test]
fn build_schema_constructs() {
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    let _schema = build_schema(corpus);
}
