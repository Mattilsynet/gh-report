//! HTTP error envelope + status mapping per CHE-0049:R4 + R10.
//!
//! Single source of truth for translating gateway / store / bus errors
//! into HTTP responses. Consumer routers call [`map_dispatch_error`],
//! [`map_store_error`], [`map_bus_error`], and
//! [`post_persist_cancellation_response`] rather than constructing
//! envelopes ad-hoc — this preserves CHE-0049:R10 mapping (Retryable →
//! 503+Retry-After, `Terminal::AggregateNotFound` → 404,
//! `StoreLocked` → 503, cancellation-after-persist → 202).
//!
//! The public JSON shape is [`ErrorBody`]; the triple form
//! ([`ErrorEnvelope`]) carries status + headers alongside the body so
//! callers can echo correlation and `Retry-After` consistently.
//!
//! Routed out of `mod middleware` (private; CHE-0030:R2) into this
//! dedicated public submodule per CHE-0049:R14.

pub use crate::middleware::{
    ErrorBody, ErrorEnvelope, map_bus_error, map_dispatch_error, map_store_error,
    post_persist_cancellation_response,
};
