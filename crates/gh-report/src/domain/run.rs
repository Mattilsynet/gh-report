//! Run metadata and lifecycle types.

use std::fmt::Write;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Metadata for a single collection run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMetadata {
    /// Unique identifier for this run.
    pub run_id: String,
    /// UTC timestamp when the run started.
    pub started_at: Timestamp,
    /// UTC timestamp when the run completed (if finished).
    pub completed_at: Option<Timestamp>,
    /// Target organization.
    pub organization: String,
    /// Evidence schema version used.
    pub schema_version: String,
    /// Outcome of the run.
    pub status: RunStatus,
}

/// Run lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Run is currently in progress.
    InProgress,
    /// Run completed successfully.
    Completed,
}

impl RunMetadata {
    /// Create new run metadata for a run starting now.
    #[must_use]
    pub fn new(organization: String, schema_version: String) -> Self {
        Self {
            run_id: generate_run_id(),
            started_at: Timestamp::now(),
            completed_at: None,
            organization,
            schema_version,
            status: RunStatus::InProgress,
        }
    }

    /// Mark the run as completed.
    pub fn complete(&mut self) {
        self.completed_at = Some(Timestamp::now());
        self.status = RunStatus::Completed;
    }

    /// The date portion of the run timestamp (YYYY-MM-DD).
    #[must_use]
    pub fn date(&self) -> String {
        self.started_at.strftime("%Y-%m-%d").to_string()
    }

    /// ISO 8601 formatted run timestamp.
    #[must_use]
    pub fn timestamp(&self) -> String {
        self.started_at
            .strftime("%Y-%m-%dT%H:%M:%S+00:00")
            .to_string()
    }

    /// Project this run into a `CorrelationContext` for cycle-rooted
    /// event correlation (CHE-0039:R1).
    ///
    /// ## Projection
    ///
    /// [`run_id`](Self::run_id) is 32 lowercase hex characters by
    /// construction (see [`generate_run_id`]) — 16 bytes:
    ///
    /// ```text
    /// run_id (32 hex chars) → 16 bytes → Uuid::from_bytes
    /// ```
    ///
    /// **Determinism**: same `run_id` → same UUID across process
    /// boundaries (no clock, PRNG, or salt), so replay with the same
    /// persisted `run_id` yields the same `correlation_id` per cycle
    /// phase (CHE-0048:R3 + CHE-0042:R3).
    ///
    /// **UUID variant bits**: carries no valid v4/v7 variant bits —
    /// `run_id` is `fastrand` output, not a structured UUID.
    /// Acceptable: `CorrelationContext` accepts any `Uuid` (CHE-0039)
    /// as an opaque correlation key.
    ///
    /// **Cycle-root**: returns
    /// [`CorrelationContext::correlated`] (`correlation_id` only) — a
    /// collection cycle roots its own chain (CHE-0039:R3).
    ///
    /// # Panics
    ///
    /// Panics if `run_id` is not 32 lowercase hex characters —
    /// guaranteed by [`generate_run_id`]; other construction violates
    /// the invariant.
    #[must_use]
    pub fn correlation_context(&self) -> cherry_pit_core::CorrelationContext {
        let mut bytes = [0u8; 16];
        let hex = self.run_id.as_bytes();
        assert!(
            hex.len() == 32,
            "run_id must be 32 hex chars, got {}",
            hex.len()
        );
        for (i, byte) in bytes.iter_mut().enumerate() {
            let hi = decode_hex_nibble(hex[i * 2]);
            let lo = decode_hex_nibble(hex[i * 2 + 1]);
            *byte = (hi << 4) | lo;
        }
        cherry_pit_core::CorrelationContext::correlated(uuid::Uuid::from_bytes(bytes))
    }
}

/// Decode one ASCII hex character (`0-9a-f`) into its nibble value.
///
/// Lowercase-only by `generate_run_id` invariant; uppercase or
/// non-hex panics.
fn decode_hex_nibble(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => panic!("run_id contains non-lowercase-hex byte: {c:#x}"),
    }
}

/// Generate a random 32-character lowercase hex run ID.
///
/// Uses `fastrand` (non-cryptographic PRNG) — run IDs are used for log
/// correlation and checkpoint naming, not security purposes.
fn generate_run_id() -> String {
    let mut bytes = [0u8; 16];
    fastrand::fill(&mut bytes);
    bytes.iter().fold(String::with_capacity(32), |mut s, b| {
        write!(s, "{b:02x}").expect("hex write to String is infallible");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_run_is_in_progress() {
        let run = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        assert_eq!(run.status, RunStatus::InProgress);
        assert!(run.completed_at.is_none());
        assert_eq!(run.organization, "TestOrg");
    }

    #[test]
    fn complete_sets_status_and_timestamp() {
        let mut run = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        run.complete();
        assert_eq!(run.status, RunStatus::Completed);
        assert!(run.completed_at.is_some());
    }

    #[test]
    fn date_returns_yyyy_mm_dd() {
        let run = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        let date = run.date();
        assert_eq!(date.len(), 10);
        assert!(date.starts_with("20"));
    }

    #[test]
    fn run_id_is_32_char_lowercase_hex() {
        let run = RunMetadata::new(
            "TestOrg".to_string(),
            crate::config::EVIDENCE_SCHEMA_VERSION.to_string(),
        );
        assert_eq!(run.run_id.len(), 32);
        assert!(
            run.run_id
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "run_id should be lowercase hex: {}",
            run.run_id,
        );
    }

    #[test]
    fn run_ids_are_unique() {
        let a = RunMetadata::new("Org".to_string(), "1.0".to_string());
        let b = RunMetadata::new("Org".to_string(), "1.0".to_string());
        assert_ne!(a.run_id, b.run_id);
    }

    #[test]
    fn correlation_context_is_deterministic_per_run_id() {
        let original = RunMetadata::new("Org".to_string(), "1.0".to_string());
        let restarted = RunMetadata {
            run_id: original.run_id.clone(),
            started_at: jiff::Timestamp::now(),
            completed_at: None,
            organization: original.organization.clone(),
            schema_version: original.schema_version.clone(),
            status: RunStatus::InProgress,
        };
        assert_eq!(
            original.correlation_context(),
            restarted.correlation_context(),
            "correlation_context must depend only on run_id (gap α determinism)",
        );
    }

    #[test]
    fn correlation_context_differs_across_run_ids() {
        let a = RunMetadata::new("Org".to_string(), "1.0".to_string());
        let b = RunMetadata::new("Org".to_string(), "1.0".to_string());
        assert_ne!(a.run_id, b.run_id);
        assert_ne!(a.correlation_context(), b.correlation_context());
    }

    #[test]
    fn correlation_context_is_cycle_rooted() {
        let run = RunMetadata::new("Org".to_string(), "1.0".to_string());
        let ctx = run.correlation_context();
        assert!(ctx.correlation_id().is_some());
        assert!(ctx.causation_id().is_none());
    }

    #[test]
    fn correlation_context_projects_run_id_bytes() {
        let run = RunMetadata {
            run_id: "00112233445566778899aabbccddeeff".to_string(),
            started_at: jiff::Timestamp::now(),
            completed_at: None,
            organization: "Org".to_string(),
            schema_version: "1.0".to_string(),
            status: RunStatus::InProgress,
        };
        let expected = uuid::Uuid::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ]);
        assert_eq!(run.correlation_context().correlation_id(), Some(expected));
    }
}
