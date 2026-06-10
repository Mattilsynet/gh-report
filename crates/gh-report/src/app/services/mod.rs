//! Per-aggregate `ApplicationService` skeletons (CHE-0054:R4).
//!
//! Each service owns the load → handle → append → publish triad
//! (CHE-0008:R1 + CHE-0024:R3) for one aggregate type:
//!
//! - [`run_service::RunService`] — Run aggregate use cases.
//! - [`repo_service::RepoService`] — Repo aggregate use cases.
//! - [`webhook_service::WebhookService`] — `WebhookDelivery` use case.
//!
//! ## Post-Mission-H shape (`adr-fmt-cq7vb.11`)
//!
//! The pre-mission single 8-variant `MergerCommand` + 700-LoC
//! in-crate `Merger` is replaced by three per-aggregate
//! [`MergerArm`](cherry_pit_merger::MergerArm) impls (see [`arms`])
//! plus three [`MergerHandle`](cherry_pit_merger::MergerHandle) clones
//! bundled into [`merger::MergerHandles`]. The lifted
//! [`cherry_pit_merger::Merger`] primitive (CHE-0069) owns the
//! `load → handle → create-or-append → publish` triad and the I1
//! TOCTOU resolution per aggregate. Service-method signatures are
//! byte-identical at the call-site boundary per CHE-0054:R10.
//!
//! ## Signature convention (resolves brief Inc-5 requirement)
//!
//! All methods take `ctx: &CorrelationContext` by reference. Rationale:
//!
//! - `CorrelationContext` is `Clone` (not `Copy`) — `&` avoids the
//!   per-call clone.
//! - The Inc 2/3/4 threading sites
//!   (`collect.rs`/`daemon.rs`/`webhook/mod.rs`) all take `&CorrelationContext`.
//!   Call sites pass through their already-threaded `&corr_ctx` with
//!   zero churn.

pub mod arms;
pub mod merger;
pub mod repo_service;
pub mod run_service;
pub mod webhook_service;

pub use arms::{RepoArm, RepoCmd, RunArm, RunCmd, WebhookArm, WebhookCmd};
pub use merger::{MergerHandles, MergerJoinHandles};
