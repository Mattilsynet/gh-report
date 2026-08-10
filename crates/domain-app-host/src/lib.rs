#![forbid(unsafe_code)]

//! Production host for the `domain-app` WASM plugin boundary
//! (CHE-0105/0106/0107, SEC-0013).
//!
//! Loads a component compiled once at [`HostRuntime::new`], then
//! instantiates a fresh [`wasmtime::Store`] per call (G-C): the world is
//! pure (state passed by value), so per-call instantiation costs little
//! and guarantees zero cross-call state leakage (SEC-0013:R4). The
//! [`wasmtime::component::Linker`] built here grants nothing beyond the
//! mandatory `wasi:io/poll` baseline `wasm32-wasip2`'s libstd always
//! pulls in (SEC-0013:R1 footnote) — the security property under test is
//! zero *granted* capability, asserted against the host
//! [`wasmtime::component::Linker`], not the guest's declared imports.

mod error;
mod runtime;

pub use error::HostError;
pub use runtime::{CALL_FUEL_LIMIT, HostRuntime};

wasmtime::component::bindgen!({
    world: "domain-app",
    path: "wit/domain-app",
});
