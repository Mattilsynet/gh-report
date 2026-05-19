//! Historic-invariant guard: `baseline.msgpack` write-path stays retired.
//!
//! ## Background
//!
//! Commit `63236ac` ("gh-report: retire baseline.msgpack + checkpoint
//! persistence; replay-as-rebuild (δ.3c-ii)") removed the on-disk
//! `baseline.msgpack` snapshot and the per-run `*.checkpoint`
//! persistence surface from `crates/gh-report/src/infra/baseline.rs`
//! and `crates/gh-report/src/infra/checkpoint.rs`. The doctrine is
//! recorded in CHE-0048 line 24 (gh-report replay-as-rebuild
//! exemption) and CHE-0022:R6 (no derived state in event payloads).
//!
//! Post-retirement, the event log is the **only** durable boot
//! source; aggregate state is rebuilt on every `AppState`
//! construction via [`AppState::snapshot_fast_path_init`] +
//! `bootstrap_replay_state` (landed as `bootstrap_replay_indices`
//! in M3 of `phase2-v2-completion-1779400000`; renamed +
//! scope-expanded to also fold `projection_state` in mission
//! `cpp-r-b-r-c` per bd `adr-fmt-5rwbu`).
//!
//! ## What this test asserts
//!
//! Any future commit re-introducing the retired write-path symbols
//! would silently invalidate the Memory-Image bootstrap invariant
//! (baseline file outliving the event log would shadow replay
//! results). The grep-based assertions below pin the retired names
//! as **absent** from the live `crates/gh-report/src/infra/` tree.
//!
//! ## How this would catch a regression
//!
//! Temporarily inserting `pub fn save_baseline() {}` into
//! `infra/baseline.rs` makes [`save_baseline_is_retired`] fail with a
//! message naming the offending file and line. This was confirmed by
//! running the test against a synthetic re-introduction during M3
//! TDD validation (see mission verify report).

use std::path::PathBuf;

fn infra_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("infra")
}

/// Returns matches of `pattern` across all `.rs` files under
/// `crates/gh-report/src/infra/`, as `(file_name, line_number, line)`.
///
/// Naive line scan — sufficient for fixed-string symbol assertions.
fn grep_infra(pattern: &str) -> Vec<(String, usize, String)> {
    let dir = infra_dir();
    let mut hits = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read_dir({}) failed: {e}", dir.display()));
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read({}) failed: {e}", path.display()));
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("<unnamed>")
            .to_string();
        for (i, line) in content.lines().enumerate() {
            if line.contains(pattern) {
                hits.push((name.clone(), i + 1, line.to_string()));
            }
        }
    }
    hits
}

/// `fn save_baseline` was the on-disk write entry-point retired by
/// `63236ac`. It must not return — `dump_baseline_json` (on
/// `AppState`) is the only sanctioned baseline render and it sources
/// from `projection_state`, not a sibling file.
#[test]
fn save_baseline_is_retired() {
    let hits = grep_infra("fn save_baseline");
    assert!(
        hits.is_empty(),
        "save_baseline write-path was retired by commit 63236ac \
         (CHE-0048 line-24, CHE-0022:R6); a re-introduction would \
         shadow event-log replay. Offending lines:\n{}",
        hits.iter()
            .map(|(f, n, l)| format!("  {f}:{n} — {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `fn load_baseline` was the on-disk read entry-point retired by
/// `63236ac`. Aggregate state is reconstructed via event-log replay
/// (`bootstrap_replay_state`), not by reading a sibling msgpack file.
///
/// Note: a `load_baseline` *method* exists on `EvidenceProjection`
/// (`src/projection.rs`) — that method ingests in-memory
/// `Vec<RepositoryEvidence>` for warm-start and is **not** the
/// retired surface. This test scopes its search to `src/infra/`
/// (where the free-fn write-path lived), so the projection method
/// does not false-positive here.
#[test]
fn load_baseline_free_fn_in_infra_is_retired() {
    let hits = grep_infra("pub fn load_baseline");
    assert!(
        hits.is_empty(),
        "load_baseline free function in infra/ was retired by commit \
         63236ac; baseline read-path is now event-log replay. \
         Offending lines:\n{}",
        hits.iter()
            .map(|(f, n, l)| format!("  {f}:{n} — {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `fn baseline_path` was the path-builder for the now-retired
/// `baseline.msgpack` sibling file. Its existence implies a write or
/// read site still expects an on-disk baseline.
#[test]
fn baseline_path_helper_is_retired() {
    let hits = grep_infra("fn baseline_path");
    assert!(
        hits.is_empty(),
        "baseline_path helper was retired by commit 63236ac; no \
         on-disk baseline file is written by gh-report. Offending \
         lines:\n{}",
        hits.iter()
            .map(|(f, n, l)| format!("  {f}:{n} — {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `fn save_checkpoint` was the per-run on-disk write retired by
/// `63236ac`. Projection-checkpoint persistence on the gh-report side
/// is exempt per CHE-0048 line 24; the only checkpoint state is the
/// in-memory `projection_checkpoint_seq: AtomicU64` on `AppState`.
#[test]
fn save_checkpoint_is_retired() {
    let hits = grep_infra("fn save_checkpoint");
    assert!(
        hits.is_empty(),
        "save_checkpoint write-path was retired by commit 63236ac \
         (CHE-0048 line-24 exemption); checkpoint state is in-memory \
         only. Offending lines:\n{}",
        hits.iter()
            .map(|(f, n, l)| format!("  {f}:{n} — {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// `fn load_checkpoint` was the per-run on-disk read retired by
/// `63236ac`. Resume is now a pure phase transition (`step_resume`)
/// driven by in-memory projection state.
#[test]
fn load_checkpoint_is_retired() {
    let hits = grep_infra("fn load_checkpoint");
    assert!(
        hits.is_empty(),
        "load_checkpoint read-path was retired by commit 63236ac; \
         resume is a pure phase transition over in-memory state. \
         Offending lines:\n{}",
        hits.iter()
            .map(|(f, n, l)| format!("  {f}:{n} — {}", l.trim()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
