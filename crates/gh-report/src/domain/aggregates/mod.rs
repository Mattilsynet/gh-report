//! Command-side aggregates for `gh-report` (CHE-0054).
//!
//! Each aggregate owns one disjoint write-coordination domain:
//!
//! - [`run::Run`] — sweep lifecycle, owns 5 sweep variants (CHE-0054:R1).
//! - [`repo::Repo`] — repository evaluation lifecycle (CHE-0054:R2).
//! - [`webhook::WebhookDelivery`] — degenerate per-delivery aggregate
//!   (CHE-0054:R3).
//!
//! Aggregates implement [`cherry_pit_core::Aggregate`] + per-command
//! [`cherry_pit_core::HandleCommand`] impls per CHE-0008:R1 (pure
//! handle, no I/O). They reuse the existing
//! [`crate::domain::events::DomainEvent`] enum as their `Event` type;
//! per-aggregate event-enum partitioning is intentionally deferred
//! (CHE-0054 mandates *ownership* of variants, not type partition).

pub mod repo;
pub mod run;
pub mod webhook;
