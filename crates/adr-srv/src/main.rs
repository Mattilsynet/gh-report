#![forbid(unsafe_code)]

//! adr-srv binary stub. M1.2 skeleton — boots an axum server with a
//! single `/health` route. GraphQL endpoint and the full scrape →
//! ingest pipeline land in M1.3 / M1.4.

use std::path::PathBuf;

use axum::{Router, routing::get};

#[tokio::main]
async fn main() {
    println!("adr-srv skeleton");

    // Preserve the M1.1 surface probe so the binary still validates
    // `adr-fmt.toml` discovery at boot. Failure here is a hard exit;
    // the server starting against a broken config would mask the
    // problem.
    let cwd: PathBuf = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match adr_srv::surface_probe(&cwd) {
        Ok(root) => println!("corpus root: {}", root.display()),
        Err(e) => {
            eprintln!("surface_probe failed: {e}");
            std::process::exit(1);
        }
    }

    // Bind on the loopback by default. Production posture (systemd /
    // container / bind address knob) is deferred to Phase 3 per the
    // oracle bead G3 gap note on M1 scope.
    let bind = std::env::var("ADR_SRV_BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let app: Router = Router::new().route("/health", get(|| async { "ok" }));

    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("failed to bind {bind}: {e}");
            std::process::exit(1);
        }
    };
    println!("adr-srv listening on {bind}");
    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("axum::serve exited: {e}");
        std::process::exit(1);
    }
}
