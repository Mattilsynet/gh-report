//! Baseline rendering: JSON-encode the resident projection.
//!
//! δ.3c-ii: replaces the pre-pivot `infra::baseline::dump_baseline` which
//! read `<store>/baseline.msgpack`. Extracted from `state.rs` (K9,
//! adr-fmt-b98n1) as a pure structural move — no behavioural change.

use super::AppState;

impl AppState {
    /// Render the current in-memory projection as a JSON-encoded
    /// [`crate::infra::baseline::Baseline`] suitable for stdout dump.
    ///
    /// δ.3c-ii: replaces the pre-pivot `infra::baseline::dump_baseline`
    /// which read `<store>/baseline.msgpack`. Callers
    /// (`--dump-baseline`) must run [`Self::snapshot_fast_path_init`]
    /// first so the projection reflects the event log.
    ///
    /// Held internally so the `lock_projection` `MutexGuard` does not
    /// escape `pub(crate)` visibility. Output shape is byte-equivalent
    /// to the pre-pivot dump (same `Baseline { schema_version,
    /// entries }`, same `serde_json::to_string_pretty` formatter).
    ///
    /// # Errors
    /// Surfaces `serde_json` serialization failure (extremely unlikely
    /// for owned, well-formed `Baseline` data).
    pub fn dump_baseline_json(&self) -> Result<String, serde_json::Error> {
        let repos: Vec<crate::domain::evidence::RepositoryEvidence> = self
            .lock_projection()
            .repositories
            .values()
            .cloned()
            .collect();
        let baseline = crate::infra::baseline::build_baseline(&repos);
        serde_json::to_string_pretty(&baseline)
    }
}
