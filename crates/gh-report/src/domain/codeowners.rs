//! CODEOWNERS domain types.
//!
//! Defines the parsed representation of a CODEOWNERS file.  These types
//! live in the domain layer because they appear in [`super::checks::CodeownersResult`]
//! and in evidence serialization, making them part of the core domain model.

use serde::{Deserialize, Serialize};

/// Reason a CODEOWNERS file was found but not parsed.
///
/// Surfaced on [`super::checks::CodeownersResult::truncation`] so a downstream
/// consumer (dashboard, audit, alerting) can distinguish "file exists, owners
/// extracted" from "file exists, parse skipped" without re-fetching the API
/// response.
///
/// All variants imply the file was located via a content-API call that
/// returned a `file` payload — the file presence component of the status
/// (`Conforming` / `NonConforming`) is unaffected. Only the parsed-owners
/// component is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeownersTruncationReason {
    /// Content API response carried a non-`base64` encoding (or no encoding).
    NotBase64Encoded,
    /// Encoded content exceeded the size cap before decoding.
    OversizedBase64,
    /// `base64` field was missing or null in the API response.
    ContentMissing,
    /// Decoded bytes failed base64 decoding (e.g. illegal characters).
    DecodeFailed,
    /// Decoded bytes were not valid UTF-8.
    InvalidUtf8,
}

/// Parsed CODEOWNERS file content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedCodeowners {
    /// Individual CODEOWNERS entries (pattern + owners).
    pub entries: Vec<CodeownersEntry>,
    /// Deduplicated list of all owners found across all entries.
    pub unique_owners: Vec<String>,
    /// Count of lines skipped during parsing because they exceeded
    /// `MAX_LINE_LENGTH` (10 KB). Comment lines and blank lines are NOT
    /// counted here — only over-length lines that were dropped without
    /// being parsed. Surfaced for observability so silent data loss is
    /// detectable from evidence alone.
    #[serde(default)]
    pub skipped_lines: u32,
}

/// A single CODEOWNERS entry: a file pattern and its owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeownersEntry {
    /// File pattern (e.g., `*.js`, `src/`, `/docs/`).
    pub pattern: String,
    /// Owner references (e.g., `@org/team`, `@user`).
    pub owners: Vec<String>,
}
