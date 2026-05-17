//! ADR template and link-integrity validator — library surface.
//!
//! This crate ships as both a binary (`adr-fmt`) and a library (`adr_fmt`).
//! The binary is a thin wrapper over the library so future consumers
//! (e.g. `adr-srv`, Phase 2 v2 C1) can re-use the parsing, linting,
//! and navigation surface without spawning a subprocess.
//!
//! The CLI surface is frozen for v0.1 per AFM-0001. The library API
//! is exposed via `pub use` / `pub mod` re-exports per CHE-0030.

#![forbid(unsafe_code)]
