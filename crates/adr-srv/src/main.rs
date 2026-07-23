#![forbid(unsafe_code)]

//! adr-srv binary. Boot pipeline (CHE-0098 native pardosa store port):
//!
//! 1. discover `adr-fmt.toml` (`surface_probe`) — hard exit on failure
//! 2. open (or create) the native pardosa `.pgno` store at
//!    `ADR_SRV_STORE`, resuming every already-`Defined` fiber (N-R5)
//! 3. `AdrService::new_with_replay(store, &corpus)` rebuilds the
//!    per-`AdrId` indices and the `AdrCorpus` projection from the
//!    persisted native events
//! 4. `scrape_corpus(...)` re-scans the markdown corpus and appends
//!    `AdrIngested` events for any frontmatter drift; this is the
//!    sanctioned rebuild-from-corpus path (CHE-0098 guardrail) — a
//!    hard cut abandons any prior `.msgpack` data, recovery is this
//!    re-scrape, not replay of the abandoned store
//! 5. mount axum router with `/health` (M1.1) and `/graphql` (M1.4)
//!
//! Production posture (systemd, bind address, TLS) stays Phase 3 per
//! the oracle bead G3 gap note on M1 scope.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_graphql_axum::GraphQL;
use axum::{Router, routing::get, routing::post_service};

use adr_srv::scrape::scrape_corpus;
use adr_srv::{AdrCorpus, AdrService, NativeAdrStore, build_schema};

#[tokio::main]
async fn main() {
    println!("adr-srv M1.4");

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

    let store_path = std::env::var("ADR_SRV_STORE")
        .map_or_else(|_| cwd.join(".adr-srv").join("store.pgno"), PathBuf::from);
    if let Some(parent) = store_path.parent()
        && let Err(e) = tokio::fs::create_dir_all(parent).await
    {
        eprintln!("create store dir {}: {e}", parent.display());
        std::process::exit(1);
    }
    let store = if store_path.exists() {
        NativeAdrStore::open_pgno(&store_path)
    } else {
        NativeAdrStore::create_pgno(&store_path)
    };
    let store = match store {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("open/create native store {}: {e}", store_path.display());
            std::process::exit(1);
        }
    };

    let corpus: Arc<Mutex<AdrCorpus>> = Arc::new(Mutex::new(AdrCorpus::default()));
    let service = match AdrService::new_with_replay(Arc::clone(&store), &corpus).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("replay failed: {e}");
            std::process::exit(1);
        }
    };

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
