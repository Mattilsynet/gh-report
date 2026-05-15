//! Aggregation logic: metrics computation and collection statistics.
//!
//! Pure functions that transform collected repository evidence into
//! aggregated metrics, statistics, and observability summaries.
//!
//! ## Migration
//!
//! Canonical home for functions previously in `collector::metrics`.
//! The old module re-exports everything from here for backward compatibility.

pub mod metrics;
