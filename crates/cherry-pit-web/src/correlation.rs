//! Correlation propagation surface per CHE-0049:R5 + R6.
//!
//! - [`correlation_layer`] — axum middleware that extracts the active
//!   [`cherry_pit_core::CorrelationContext`] from inbound headers
//!   (W3C `traceparent` primary, `X-Correlation-ID` fallback), stashes
//!   it in request extensions, and echoes the correlation id on every
//!   response.
//! - [`correlation_from_extensions`] — handler-side accessor returning
//!   the stashed context, or [`cherry_pit_core::CorrelationContext::none`]
//!   when the layer is not active (CHE-0039:R2 — never synthesise).
//! - [`extract_correlation`], [`extract_idempotency_key`],
//!   [`IdempotencyKey`] — lower-level header extractors used inside the
//!   middleware layer and by consumer routers that need direct access.
//!
//! Routed out of the private `mod middleware` (CHE-0030:R2) into this
//! dedicated public submodule per CHE-0049:R14.

pub use crate::middleware::{
    IdempotencyKey, correlation_from_extensions, correlation_layer, extract_correlation,
    extract_idempotency_key,
};
