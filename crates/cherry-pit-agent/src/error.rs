//! Agent-level error type.
//!
//! Per CHE-0051:R4 + CHE-0051:R8: the dispatch closure and `App::run` both
//! return `Result<(), AgentError>`. Variants are filled in as composition
//! surfaces land. `#[non_exhaustive]` per CHE-0021:R1 so future variants
//! do not break consumers.

use std::error::Error;
use std::fmt;

use cherry_pit_core::{BusError, ErrorCategory, StoreError};

/// Errors surfaced by the agent composition layer.
///
/// Per CHE-0051:R4 + R8.
///
/// Variant set covers the failure surfaces the dispatch loop can hit:
/// policy-output-dispatch failures (caller closure returned Err),
/// dead-letter-sink write failures, and store/bus port failures
/// bubbled up through composition. `#[non_exhaustive]` provides
/// forward compatibility — a catch-all `Other` variant is deliberately
/// omitted until a real call site demands it.
#[derive(Debug)]
#[non_exhaustive]
pub enum AgentError {
    /// A registered policy's dispatch closure returned an error.
    /// Carries the boxed underlying error from the user closure.
    Policy(Box<dyn Error + Send + Sync>),

    /// A `DeadLetterSink::record` call failed.
    DeadLetter(Box<dyn Error + Send + Sync>),

    /// An `EventStore` operation failed.
    Store(StoreError),

    /// An `EventBus::publish` call failed.
    Bus(BusError),
}

impl AgentError {
    /// Classify the agent failure for retry guidance per CHE-0021:R3
    /// + CHE-0046:R1–R2.
    ///
    /// Policy-closure and dead-letter failures are treated as
    /// `Terminal` (the agent does not retry user closures); store and
    /// bus failures inherit their port category.
    ///
    /// # Example
    ///
    /// ```
    /// use cherry_pit_agent::AgentError;
    /// use cherry_pit_core::{BusError, ErrorCategory};
    ///
    /// let policy_err = AgentError::Policy("user closure failed".into());
    /// assert_eq!(policy_err.category(), ErrorCategory::Terminal);
    ///
    /// let bus_err = AgentError::Bus(BusError::new("connection reset"));
    /// assert_eq!(bus_err.category(), ErrorCategory::Retryable);
    /// ```
    #[must_use]
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::Policy(_) | Self::DeadLetter(_) => ErrorCategory::Terminal,
            Self::Store(e) => e.category(),
            Self::Bus(e) => e.category(),
        }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(e) => write!(f, "policy dispatch error: {e}"),
            Self::DeadLetter(e) => write!(f, "dead-letter sink error: {e}"),
            Self::Store(e) => write!(f, "store error: {e}"),
            Self::Bus(e) => write!(f, "bus error: {e}"),
        }
    }
}

impl Error for AgentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Policy(e) | Self::DeadLetter(e) => Some(e.as_ref()),
            Self::Store(e) => Some(e),
            Self::Bus(e) => Some(e),
        }
    }
}

impl From<StoreError> for AgentError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<BusError> for AgentError {
    fn from(e: BusError) -> Self {
        Self::Bus(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_variant_is_terminal() {
        let err = AgentError::Policy("nope".into());
        assert_eq!(err.category(), ErrorCategory::Terminal);
    }

    #[test]
    fn dead_letter_variant_is_terminal() {
        let err = AgentError::DeadLetter("disk full".into());
        assert_eq!(err.category(), ErrorCategory::Terminal);
    }

    #[test]
    fn bus_variant_inherits_retryable() {
        let err = AgentError::Bus(BusError::new("network"));
        assert_eq!(err.category(), ErrorCategory::Retryable);
    }

    #[test]
    fn store_variant_inherits_terminal_for_corrupt_data() {
        let err = AgentError::Store(StoreError::CorruptData("bad".into()));
        assert_eq!(err.category(), ErrorCategory::Terminal);
    }

    #[test]
    fn display_includes_inner() {
        let err = AgentError::Policy("inner thing".into());
        assert!(err.to_string().contains("inner thing"));
    }
}
