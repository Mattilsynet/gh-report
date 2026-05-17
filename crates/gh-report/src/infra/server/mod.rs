//! In-memory HTTP server: config, state types, request pipeline.
//!
//! Absorbed under mission `absorb-server-1778695800` (P1-A.5.2).
//! Provides a generic SERVE pipeline (security headers, ETag/304,
//! zstd negotiation, WebSocket live updates, path normalization) —
//! zero domain knowledge. Domain-specific state implements
//! [`state::ServerState`].

pub mod config;
pub mod error;
#[expect(
    clippy::module_inception,
    reason = "absorbed under mission `absorb-server-1778695800` (P1-A.5.2) as a byte-for-byte donor port preserving the donor's `server::server` shape; upstream crate/commit provenance not preserved in this codebase"
)]
pub mod server;
pub mod state;
