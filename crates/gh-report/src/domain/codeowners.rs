//! CODEOWNERS domain types.
//!
//! Defines the parsed representation of a CODEOWNERS file.  These types
//! live in the domain layer because they appear in [`super::checks::CodeownersResult`]
//! and in evidence serialization, making them part of the core domain model.

use cherry_pit_core::pardosa_encoding::Encode;
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
///
/// # Wire format
///
/// Variant discriminant is `u8` of declaration position (`NotBase64Encoded=0`,
/// `OversizedBase64=1`, `ContentMissing=2`, `DecodeFailed=3`, `InvalidUtf8=4`).
/// Reorder or insert is a wire-format break (CHE-0064:R2 + PAR-0024:R5); new
/// variants must append.
///
/// ```
/// use gh_report::domain::codeowners::CodeownersTruncationReason;
/// use cherry_pit_core::pardosa_encoding::Encode;
/// let mut out = Vec::new();
/// CodeownersTruncationReason::OversizedBase64.encode(&mut out);
/// assert_eq!(out, vec![1u8]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum CodeownersTruncationReason {
    /// Content API response carried a non-`base64` encoding (or no encoding).
    NotBase64Encoded = 0,
    /// Encoded content exceeded the size cap before decoding.
    OversizedBase64 = 1,
    /// `base64` field was missing or null in the API response.
    ContentMissing = 2,
    /// Decoded bytes failed base64 decoding (e.g. illegal characters).
    DecodeFailed = 3,
    /// Decoded bytes were not valid UTF-8.
    InvalidUtf8 = 4,
}

impl Encode for CodeownersTruncationReason {
    fn encode(&self, out: &mut Vec<u8>) {
        out.push(*self as u8);
    }
}

/// Parsed CODEOWNERS file content.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `entries`,
/// `unique_owners`, `skipped_lines`. Field reorder is a wire-format break
/// (CHE-0064:R2 + PAR-0024:R5); new fields must append.
///
/// ```
/// use gh_report::domain::codeowners::ParsedCodeowners;
/// use cherry_pit_core::pardosa_encoding::Encode;
/// let parsed = ParsedCodeowners {
///     entries: Vec::new(),
///     unique_owners: Vec::new(),
///     skipped_lines: 0,
/// };
/// let mut out = Vec::new();
/// parsed.encode(&mut out);
/// assert!(!out.is_empty());
/// ```
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
    pub skipped_lines: u32,
}

impl Encode for ParsedCodeowners {
    fn encode(&self, out: &mut Vec<u8>) {
        self.entries.encode(out);
        self.unique_owners.encode(out);
        self.skipped_lines.encode(out);
    }
}

/// A single CODEOWNERS entry: a file pattern and its owners.
///
/// # Wire format
///
/// Fields encode in declaration order via `Encode::encode`: `pattern`,
/// `owners`. Field reorder is a wire-format break (CHE-0064:R2 + PAR-0024:R5);
/// new fields must append.
///
/// ```
/// use gh_report::domain::codeowners::CodeownersEntry;
/// use cherry_pit_core::pardosa_encoding::Encode;
/// let entry = CodeownersEntry {
///     pattern: "*.rs".to_string(),
///     owners: vec!["@team".to_string()],
/// };
/// let mut out = Vec::new();
/// entry.encode(&mut out);
/// assert!(!out.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeownersEntry {
    /// File pattern (e.g., `*.js`, `src/`, `/docs/`).
    pub pattern: String,
    /// Owner references (e.g., `@org/team`, `@user`).
    pub owners: Vec<String>,
}

impl Encode for CodeownersEntry {
    fn encode(&self, out: &mut Vec<u8>) {
        self.pattern.encode(out);
        self.owners.encode(out);
    }
}
