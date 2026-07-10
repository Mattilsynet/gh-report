//! `gh-report` — GitHub organization governance collector and reporter.
//!
//! This crate provides the core library for collecting GitHub governance
//! data, evaluating repository-level security controls, aggregating metrics,
//! and serving HTML reports from an in-memory cache.
//!
//! See [`infra::logging`] for the logging convention and GCP Cloud Logging
//! formatter.

#![cfg_attr(not(test), forbid(unsafe_code))]
#![cfg_attr(test, deny(unsafe_code))]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod aggregate;
pub mod api_outcome;
pub mod app;
pub mod collector;
pub mod config;
pub mod domain;
pub mod error;
pub mod event;
pub mod github;
pub mod infra;
pub mod pattern_match;
pub mod projection;
pub mod report;
pub mod server;
pub mod store;
pub mod webhook;

#[cfg(test)]
pub mod test_fixtures;
