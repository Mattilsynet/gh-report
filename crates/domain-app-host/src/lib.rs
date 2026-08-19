#![forbid(unsafe_code)]

//! Production host for the `domain-app` WASM plugin boundary
//! (CHE-0105/0106/0107, SEC-0014).
//!
//! Loads a component compiled once at [`HostRuntime::new`], then
//! instantiates a fresh [`wasmtime::Store`] per call (G-C): the world is
//! pure (state passed by value), so per-call instantiation costs little
//! and guarantees zero cross-call state leakage (SEC-0014:R4). The
//! [`wasmtime::component::Linker`] built here grants nothing beyond the
//! mandatory `wasi:io/*` baseline `wasm32-wasip2`'s libstd always pulls
//! in (genuinely linked) plus eleven always-trapping stub interfaces
//! (`wasi:clocks/monotonic-clock` and the ten `wasi:cli/*` interfaces,
//! ADR-fmt-4ksfn AMENDMENT 1) that resolve every declared import but
//! convey zero capability — the security property under test is zero
//! *granted* capability, asserted against the host
//! [`wasmtime::component::Linker`], not the guest's declared imports.

mod error;
mod runtime;

pub use error::HostError;
pub use runtime::{CALL_FUEL_LIMIT, HostRuntime};

wasmtime::component::bindgen!({
    world: "domain-app",
    path: "wit/domain-app",
    additional_derives: [PartialEq],
});
