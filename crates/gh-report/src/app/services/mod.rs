//! Per-aggregate ApplicationService skeletons (CHE-0054:R4).
//!
//! Each service owns the load → handle → append → publish triad
//! (CHE-0008:R1 + CHE-0024:R3) for one aggregate type:
//!
//! - [`run_service::RunService`] — Run aggregate use cases.
//! - [`repo_service::RepoService`] — Repo aggregate use cases.
//! - [`webhook_service::WebhookService`] — WebhookDelivery use case.
//!
//! ## Method body status (Inc B7'a-5)
//!
//! Method bodies are `unimplemented!()` skeletons. The
//! load→handle→append→publish wiring lands in **B7'b**; the existing
//! 14 production publish sites (`collect.rs`/`daemon.rs`/
//! `webhook/mod.rs`) migrate to these service calls in **B7'c**.
//!
//! ## Signature convention (resolves brief Inc-5 requirement)
//!
//! All methods take `ctx: &CorrelationContext` by reference. Rationale:
//!
//! - `CorrelationContext` is `Clone` (not `Copy`) — `&` avoids the
//!   per-call clone.
//! - The Inc 2/3/4 threading sites
//!   (`collect.rs`/`daemon.rs`/`webhook/mod.rs`) all take `&CorrelationContext`.
//!   B7'c call sites can pass through their already-threaded
//!   `&corr_ctx` with zero churn — no `.clone()` required.
//! - Diverges from the moltke mid-checkpoint suggestion of by-value
//!   (chosen here for Q7-corpus alignment); flagged in the Inc B7'a-5
//!   commit.
//!
//! ## Open γ — EventStore stack shape
//!
//! Per moltke instruction: hopper picks at Inc B7'a-5/6 wiring.
//! Resolution: **per-aggregate concrete EventStore instances** (three
//! `Arc<MsgpackFileStore<DomainEvent>>`, one per service). CHE-0054:R8
//! permits either; per-aggregate keeps each service self-contained
//! and matches the per-aggregate write-coordination granularity
//! justified by R4. AppState wiring (Inc B7'a-6) materialises the
//! three instances.
//!
//! ## Open ε — `Option<Arc<...Service>>` smell
//!
//! Resolved at Inc B7'a-6 wiring time, not here. The skeletons make
//! no assumption either way — `AppState::new()` may construct services
//! eagerly (no Option) or lazily (Option) depending on whether an
//! in-memory EventStore exists in the workspace yet (oracle Gap-β).

pub mod repo_service;
pub mod run_service;
mod shared;
pub mod webhook_service;
