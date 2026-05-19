#![forbid(unsafe_code)]

//! adr-srv binary. M1.4 boot pipeline:
//!
//! 1. discover `adr-fmt.toml` (`surface_probe`) — hard exit on failure
//! 2. open `PardosaFileEventStore<AdrIngested>` at `ADR_SRV_STORE`
//!    (default `./.adr-srv/store`)
//! 3. `AdrService::new_with_replay(store, &corpus)` rebuilds the
//!    `adrs_by_id` / `latest_body_hash` indices and the projection
//!    from the event log per CHE-0065 (replay-on-boot election)
//! 4. `scrape_corpus(...)` scans the markdown corpus and appends
//!    `AdrIngested` events for any frontmatter drift; projection is
//!    kept in lock-step on every append
//! 5. mount axum router with `/health` (M1.1) and `/graphql` (M1.4)
//!
//! Production posture (systemd, bind address, TLS) stays Phase 3 per
//! the oracle bead G3 gap note on M1 scope.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_graphql_axum::GraphQL;
use axum::{Router, routing::get, routing::post_service};

use adr_srv::scrape::scrape_corpus;
use adr_srv::{AdrCorpus, AdrIngested, AdrService, build_schema};
use cherry_pit_pardosa::PardosaFileEventStore;

#[tokio::main]
async fn main() {
    println!("adr-srv M1.4");

    // (1) discover adr-fmt.toml — hard exit so a misconfigured server
    // surfaces the problem rather than masking it.
    let cwd: PathBuf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let _corpus_root = match adr_srv::surface_probe(&cwd) {
        Ok(root) => {
            println!("corpus root: {}", root.display());
            root
        }
        Err(e) => {
            eprintln!("surface_probe failed: {e}");
            std::process::exit(1);
        }
    };

    // (2) open event store. Default location is per-cwd; override via
    // ADR_SRV_STORE for ops. Store directory is created on first run.
    let store_path = std::env::var("ADR_SRV_STORE")
        .map_or_else(|_| cwd.join(".adr-srv").join("store"), PathBuf::from);
    if let Err(e) = std::fs::create_dir_all(&store_path) {
        eprintln!("create store dir {}: {e}", store_path.display());
        std::process::exit(1);
    }
    let store: PardosaFileEventStore<AdrIngested> = match PardosaFileEventStore::open(&store_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "PardosaFileEventStore::open({}) failed: {e}",
                store_path.display()
            );
            std::process::exit(1);
        }
    };
    let store = Arc::new(store);

    // (3) replay → service + projection. CHE-0065: AdrCorpus is
    // rebuilt deterministically from the event log on every boot.
    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    let service = match AdrService::new_with_replay(Arc::clone(&store), &corpus).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("replay failed: {e}");
            std::process::exit(1);
        }
    };

    // (4) one boot-time scrape. Per-record errors land in the report's
    // diagnostics; only infra errors (resolve corpus, read dir, store
    // append) bubble up here and abort boot.
    match scrape_corpus(&service, &cwd, &corpus).await {
        Ok(report) => println!(
            "boot scrape: {} records seen, {} events emitted, {} diagnostics",
            report.records_seen,
            report.events_emitted,
            report.diagnostics.len()
        ),
        Err(e) => {
            eprintln!("boot scrape failed: {e}");
            std::process::exit(1);
        }
    }

    // (5) build the GraphQL schema once. async-graphql `Schema` is
    // cheaply Clone (internal Arc), so axum can clone it per request.
    let schema = build_schema(Arc::clone(&corpus));

    let bind = std::env::var("ADR_SRV_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let app: Router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/graphql", post_service(GraphQL::new(schema)));

    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind {bind}: {e}");
            std::process::exit(1);
        }
    };
    println!("adr-srv listening on {bind} (POST /graphql, GET /health)");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("axum::serve exited: {e}");
        std::process::exit(1);
    }
}
