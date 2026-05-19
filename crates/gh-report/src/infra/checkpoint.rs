//! Snapshot-signature utility re-export.
//!
//! δ.3c-ii retired the on-disk sweep-level checkpoint surface
//! (`<run>-checkpoint.msgpack`). Same-day resume is now driven by
//! event-log replay through the projection runtime per CHE-0051:R5
//! and CHE-0048:R2. The projection sub-checkpoint surface
//! (`<aggregate_id>-evidence.checkpoint.msgpack`, CHE-0048:R1) is
//! unaffected and lives under the projection store, not this module.
//!
//! All on-disk checkpoint functions previously exposed here
//! (`Checkpoint`, `load_checkpoint`, `save_checkpoint`,
//! `try_resume`, `remove_checkpoint`, `checkpoint_path`,
//! `restamp_evidence`, `result_has_expected_checks`,
//! `rotate_corrupt_checkpoint`, `MAX_CHECKPOINT_FILE_BYTES`) have
//! been removed. The single survivor is the
//! [`build_snapshot_signature`] re-export, which threads through
//! `StartSweep` (see [`crate::domain::aggregates::run::StartSweep`])
//! and is still authored against the SHA-256 alert-summary scheme
//! documented in `cherry_pit_storage`.

// Re-export signature utilities (sole δ.3c-ii survivor).
pub use cherry_pit_storage::build_snapshot_signature;
