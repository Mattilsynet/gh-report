//! Standalone, self-contained, client-side animated discrete-event
//! simulation of gh-report's queue network (adr-fmt-223sd), making
//! queue-network theory (arrivals, bounded buffer with drop + dedup
//! loss, M/M/c service station, batch join-barrier, departure fold →
//! render → publish) legible in a browser. No dependency on a running
//! gh-report; no trace capture.
//!
//! [`sim`] is pure and host-testable; [`view`] (wasm32-only) drives it
//! frame-by-frame and renders it via raw `web-sys` DOM calls driven by
//! leptos reactive primitives (no `view!` macro — mirrors
//! `gh-report-web-client`'s `dom.rs` approach).
//!
//! Run in a browser:
//! ```text
//! cd crates/cherry-pit-sd-viz
//! wasm-pack build --target web --out-dir pkg --dev
//! python3 -m http.server 8787
//! # open http://localhost:8787/index.html
//! ```
//! See `README.md` for the exact commands (wasm-pack and
//! wasm-bindgen-cli variants).

#![forbid(unsafe_code)]

pub mod binding;
pub mod layout;
pub mod scene;
pub mod sd;
pub mod sim;
pub mod sparkline;

#[cfg(target_arch = "wasm32")]
pub mod components;
#[cfg(target_arch = "wasm32")]
pub mod overlay;
#[cfg(target_arch = "wasm32")]
mod view;
