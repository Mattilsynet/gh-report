//! Q.4 — cross-process persistence smoke.
//!
//! Spawns the `gh-report` binary as a child process to validate the
//! durability claim that an `EventEnvelope<DomainEvent>` written and
//! fsync'd in run N is recoverable in run N+1 across process
//! boundaries (CHE-0024 persist-then-publish; pardosa-eventstore
//! Path B frame layer).
//!
//! In-process integration tests (`tests/bootstrap_replay.rs`) cover
//! the projection-replay logic but cannot falsify OS-level fsync /
//! page-cache ordering bugs because both seed and read happen inside
//! the same process. This smoke seeds via direct `pardosa-eventstore`
//! API in one Tokio runtime, drops the store handle (releases
//! `RunLock`), then `assert_cmd::Command::cargo_bin` spawns a fresh
//! process running `gh-report --dump-baseline`, which executes the
//! full `AppState::with_stores` → `snapshot_fast_path_init` →
//! `dump_baseline_json` chain. The kernel's writeback cache may have
//! flushed (or not) between the two; the test exercises whichever
//! path wins on the host.
//!
//! Assertion shape is intentionally coarse: exit 0 + parseable
//! `Baseline` JSON. The point is to falsify "binary boots cleanly
//! against a previously-written store" — not to reverify projection
//! semantics, which `bootstrap_replay` already covers in-process.

use std::sync::Arc;

use cherry_pit_core::{CorrelationContext, EventStore};
use cherry_pit_gateway::MsgpackFileStore;
use gh_report::domain::events::DomainEvent;

use assert_cmd::Command;

/// The bin reads `events/<org>/` per `bin/gh-report.rs` L96. Tests
/// must seed at the same nested path.
const ORG: &str = "test-org-q4";

/// Empty-store boot: spawn the bin against a fresh tempdir; assert
/// exit 0 + a JSON baseline with empty `entries`. Falsifies "binary
/// crashes when no events have been written yet".
#[test]
fn dump_baseline_against_empty_store_exits_zero_with_empty_entries() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Bin auto-creates events/<org>/ on first open — no pre-mkdir needed.

    let output = Command::cargo_bin("gh-report")
        .expect("locate gh-report binary")
        .args([
            "--dump-baseline",
            "--org",
            ORG,
            "--store-dir",
            tmp.path().to_str().expect("tempdir is utf-8"),
        ])
        .output()
        .expect("spawn gh-report");

    assert!(
        output.status.success(),
        "gh-report --dump-baseline (empty store) exited {:?}; stderr=\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let baseline: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout is valid JSON Baseline");

    assert!(
        baseline.get("schema_version").is_some(),
        "Baseline missing schema_version; raw stdout:\n{stdout}"
    );
    let entries = baseline
        .get("entries")
        .and_then(|v| v.as_object())
        .expect("Baseline.entries is an object");
    assert!(
        entries.is_empty(),
        "empty store must yield empty entries; got {} entries",
        entries.len()
    );
}

/// Seeded-store boot: write one `SweepStarted` envelope through
/// `pardosa-eventstore`, fsync, drop the handle (releases
/// `RunLock`), then spawn the bin. Assert exit 0 + valid JSON.
///
/// `SweepStarted` is intentionally a no-op for `Baseline.entries`
/// (the Run aggregate is folded but contributes no `RepositoryEvidence`).
/// Q.4's claim under test is *not* that entries are populated — that
/// is covered in-process by `bootstrap_replay::restart_rehydrates_projection_state`.
/// The claim under test here is that the bin opens a non-empty store
/// from a prior process, walks the frame, replays through the
/// projection runtime, and exits cleanly. Anything stronger requires
/// a `RepoEvaluated` fixture with non-`None` `updated_at` and
/// non-Unknown checks; that's the cost-vs-coverage tradeoff
/// articulated in Q.4's brief.
#[tokio::test]
async fn dump_baseline_against_seeded_store_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let events_dir = tmp.path().join("events").join(ORG);
    std::fs::create_dir_all(&events_dir).expect("mk events dir");

    {
        let store = Arc::new(MsgpackFileStore::<DomainEvent>::new(&events_dir));
        let ctx = CorrelationContext::none();
        let event = DomainEvent::SweepStarted {
            org: ORG.into(),
            repo_count: 0,
            batch_id: "batch-q4-smoke".into(),
            timestamp: "2026-05-20T00:00:00Z".into(),
            snapshot_signature: None,
        };
        store
            .create(vec![event], ctx)
            .await
            .expect("create Run aggregate");
        // store dropped at scope end — RunLock released, fsync
        // already complete per CHE-0024 (`create` returns Ok only
        // after `write_all + fsync`).
    }

    let output = Command::cargo_bin("gh-report")
        .expect("locate gh-report binary")
        .args([
            "--dump-baseline",
            "--org",
            ORG,
            "--store-dir",
            tmp.path().to_str().expect("tempdir is utf-8"),
        ])
        .output()
        .expect("spawn gh-report");

    assert!(
        output.status.success(),
        "gh-report --dump-baseline (seeded store) exited {:?}; stderr=\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let baseline: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout is valid JSON Baseline");

    assert!(
        baseline.get("schema_version").is_some(),
        "Baseline missing schema_version; raw stdout:\n{stdout}"
    );
    // entries may or may not be empty depending on which DomainEvent
    // arm was seeded; SweepStarted leaves it empty by design. The
    // load-bearing assertion is the bin-exit-zero above.
    assert!(
        baseline.get("entries").is_some(),
        "Baseline missing entries object; raw stdout:\n{stdout}"
    );
}
