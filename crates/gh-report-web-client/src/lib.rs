//! Leptos CSR client for gh-report's progressive-enhancement sortable
//! tables (CHE-0087). Compiled to `wasm32-unknown-unknown`.
//!
//! `leptos`/`web-sys`/`wasm-bindgen`/`js-sys` are declared as
//! `wasm32`-only target dependencies (see `Cargo.toml`), so a plain
//! host `cargo build --workspace` never fetches or compiles them for
//! this crate; only [`sort`] (pure, dependency-free) compiles on host.

#![deny(unsafe_code)]

pub mod sort;

#[cfg(target_arch = "wasm32")]
mod dom;
