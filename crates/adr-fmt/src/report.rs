//! Diagnostic reporting for ADR validation.
//!
//! Per AFM-0003, all rule findings are warnings; the tool exits 0 on
//! lint completion regardless of warning count. Exit 1 is reserved for
//! infrastructure failures (missing config, unreadable files, invalid
//! configuration) which are signalled directly via stderr + `process::exit`
//! in `main`, not through this diagnostic channel.

use std::fmt;
use std::path::Path;

/// Diagnostic severity. Only `Warning` is emitted today per AFM-0003
/// advisory-only semantics. The enum is kept as a single-variant type
/// to leave room for a future `--error-on-warning` mode without breaking
/// the diagnostic API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Warning => write!(f, "warning"),
        }
    }
}

/// A single diagnostic message attached to a file and optional line.
#[derive(Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub rule: &'static str,
    pub file: String,
    pub line: usize,
    pub message: String,
    /// Internal diagnostics are not shown to users.
    pub internal: bool,
}

impl Diagnostic {
    #[must_use]
    pub fn warning(rule: &'static str, file: &Path, line: usize, message: String) -> Self {
        Self {
            severity: Severity::Warning,
            rule,
            file: file.display().to_string(),
            line,
            message,
            internal: false,
        }
    }
}
