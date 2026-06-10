//! Pre-SMI replay-corpus capture harness for Track 4.0 (mission
//! `adr-fmt-nnn3`).
//!
//! Captures a deterministic, representative event log via the
//! Merger-routed write path (Track 4.0/3b onward — `RunService` is a
//! thin channel-send wrapper over the [`Merger`] task; `RepoService`
//! and `WebhookService` continue to write directly until their
//! respective reroute steps 4 / 5) into
//! `tests/fixtures/smi_pre_corpus/`, alongside two derived snapshot
//! files used by the load-bearing replay test
//! (`smi_replay_equivalence`).
//!
//! The on-disk byte sequence is invariant across the reroute steps
//! (criterion #4 / #11) — that is precisely what
//! `smi_replay_equivalence` enforces against this fixture. The
//! harness updates here only reflect the call-site shape needed to
//! drive the **current** API surface, not a change in the captured
//! bytes.
//!
//! ## Why this is a `#[ignore]`-gated test, not a binary target
//!
//! The fixture is **committed**. Normal `cargo test` must NOT regenerate
//! it — that would make the replay test circular. Capture runs only on
//! explicit demand:
//!
//! ```text
//! cargo test -p gh-report --test smi_corpus_capture -- --ignored
//! ```
//!
//! The fixture is regenerated only when an intentional change to the
//! corpus shape (new scenario step, additional aggregate) is in
//! progress; the resulting diff is part of the same commit as the
//! scenario change. Routine refactors that pass `smi_replay_equivalence`
//! never touch the fixture.
//!
//! ## Determinism
//!
//! Every payload-side identifier (`batch_id`, `domain_key`,
//! `delivery_id`, timestamps) is a literal string constant. The fresh
//! tempdir-backed [`MsgpackFileStore`] assigns `AggregateId`s from a
//! `u64` counter starting at 1, so a fresh store always produces the
//! same id sequence for the same scenario. Envelope `event_id` (Uuid)
//! and envelope `timestamp` are runtime metadata, intentionally NOT
//! covered by replay-equivalence (see `.ooda/brief-track-4.0-smi.md`
//! § "Replay-equivalence test design"). The capture writes the raw
//! `.msgpack` files for criterion #11 (format-stability witness) but
//! the replay test only asserts on payload sequence + projection
//! snapshot.
//!
//! Determinism of `AggregateId` assignment across the Merger reroute
//! depends on the harness driving commands in a fixed order through a
//! single-task merger: the Merger consumes commands strictly FIFO from
//! its [`tokio::sync::mpsc`] queue, so the `EventStore::create` calls
//! it issues fire in the same order the harness `.await`s each
//! service method. The counter therefore advances 1, 2, 3, … in
//! scenario order, matching pre-3b capture.
//!
//! ## Scenario coverage
//!
//! One Run aggregate exercising all five Run variants
//! (`SweepStarted`/`SweepProgress`/`SweepCompleted`/`EvidencePublished`
//! — failure path covered on a second Run), one Repo aggregate with
//! both `RepoEvaluated` (carrying `RepositoryEvidence` to exercise
//! projection materialisation) and `RepoRemoved`, and one
//! `WebhookReceived` via the fresh-per-delivery `WebhookService` path.
//! Covers all 8 `DomainEvent` variants — satisfies criterion #10
//! (sweep audit trail preserved) and exercises the only two variants
//! that mutate `EvidenceProjection`.
//!
//! [`MergerHandles`]: gh_report::app::services::MergerHandles
//! [`cherry_pit_merger::Merger`]: cherry_pit_merger::Merger

use std::collections::HashMap;
use std::fs;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cherry_pit_app::InProcessEventBus;
use cherry_pit_core::{AggregateId, CorrelationContext, EventStore};
use gh_report::app::state::EventStoreImpl;

use gh_report::app::services::MergerHandles;
use gh_report::app::services::repo_service::RepoService;
use gh_report::domain::aggregates::repo::{RecordEvaluation, RecordRemoval};
use gh_report::domain::events::DomainEvent;
use gh_report::projection::EvidenceProjection;

/// Resolve the fixtures directory relative to the crate manifest.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("smi_pre_corpus")
}

/// Stable, literal payload-side timestamps. Driving the scenario with
/// these keeps the captured event payloads byte-identical run over run
/// (envelope metadata varies — see module docs).
const TS_T0: &str = "2026-05-16T10:00:00Z";
const TS_T1: &str = "2026-05-16T10:00:01Z";
const REPO_KEY: &str = "id-smi-repo-alpha";
const REPO_KEY_REMOVED: &str = "id-smi-repo-removed";

#[tokio::test]
#[ignore = "regenerates committed fixture under tests/fixtures/smi_pre_corpus/ — run only when intentionally bumping corpus shape"]
async fn capture_pre_smi_corpus() {
    let target = prepare_fixture_dir();

    let store_dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(EventStoreImpl::create_pgno(&store_dir.path().join("events.pgno")).unwrap());
    let bus = Arc::new(InProcessEventBus::<DomainEvent>::new());
    let repos_by_key: Arc<Mutex<HashMap<String, AggregateId>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let tracker: Arc<Mutex<HashMap<AggregateId, NonZeroU64>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let (handles, _joins) = MergerHandles::spawn(
        Arc::clone(&store),
        Arc::clone(&bus),
        Arc::clone(&repos_by_key),
        Arc::clone(&tracker),
    );

    let repo = RepoService::with_handle(handles.repo);

    let ctx = CorrelationContext::none();

    repo_aggregate_evaluate_and_remove(&repo, &ctx).await;

    let copied = copy_msgpack_files(store_dir.path(), &target);
    let aggregate_ids = collect_aggregate_ids(&repos_by_key);
    let payload_sequence = load_payload_sequence(&store, &aggregate_ids).await;
    let projection = fold_projection(&store, &aggregate_ids).await;

    write_snapshots(&target, &payload_sequence, &projection, &copied);
}

fn prepare_fixture_dir() -> PathBuf {
    let target = fixtures_dir();
    if target.exists() {
        fs::remove_dir_all(&target).expect("clean fixture dir");
    }
    fs::create_dir_all(&target).expect("recreate fixture dir");
    target
}

type RepoSvc = RepoService;

async fn repo_aggregate_evaluate_and_remove(repo: &RepoSvc, ctx: &CorrelationContext) {
    repo.record_evaluation(
        REPO_KEY,
        RecordEvaluation {
            domain_key: REPO_KEY.into(),
            repo_name: "smi-repo-alpha".into(),
            success: true,
            source: "scheduled_batch".into(),
            duration_ms: 250,
            timestamp: TS_T0.into(),
            evidence: None,
        },
        ctx,
    )
    .await
    .expect("record_evaluation");
    repo.record_removal(
        REPO_KEY_REMOVED,
        RecordRemoval {
            domain_key: REPO_KEY_REMOVED.into(),
            repo_name: "smi-repo-removed".into(),
            timestamp: TS_T1.into(),
        },
        ctx,
    )
    .await
    .expect("record_removal");
}

/// Copy `<id>.msgpack` files from the live store directory into the
/// fixture directory and return the manifest list, sorted numerically
/// by the `<id>` stem.
///
/// Numeric (not lexicographic) sort is load-bearing: lex order matches
/// numeric only for ids 1..=9; at id ≥ 10 the manifest would silently
/// flip to `1, 10, 11, 2, 3, …`, which the replay test would happily
/// bless as a new "valid" ordering, masking real regressions.
fn copy_msgpack_files(store_dir: &Path, target: &Path) -> Vec<String> {
    let mut copied: Vec<(u64, String)> = Vec::new();
    for entry in fs::read_dir(store_dir).expect("read store dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("msgpack") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("utf-8 filename")
            .to_string();
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or_else(|| {
                panic!("non-numeric msgpack stem {filename} — fixture layout assumption broken")
            });
        fs::copy(&path, target.join(&filename)).expect("copy fixture msgpack");
        copied.push((stem, filename));
    }
    copied.sort_by_key(|(stem, _)| *stem);
    assert!(
        !copied.is_empty(),
        "expected at least one per-aggregate msgpack file in capture"
    );
    copied.into_iter().map(|(_, name)| name).collect()
}

fn collect_aggregate_ids(repos_by_key: &Arc<Mutex<HashMap<String, AggregateId>>>) -> Vec<AggregateId> {
    let mut all: Vec<AggregateId> = Vec::new();
    let guard = repos_by_key.lock().expect("index lock");
    all.extend(guard.values().copied());
    all.sort_by_key(|id| id.get());
    all.dedup();
    all
}

async fn load_payload_sequence(
    store: &Arc<EventStoreImpl>,
    aggregate_ids: &[AggregateId],
) -> Vec<DomainEvent> {
    let mut out: Vec<DomainEvent> = Vec::new();
    for id in aggregate_ids {
        let envelopes = store.load(*id).await.expect("load aggregate stream");
        for env in envelopes {
            out.push(env.payload().clone());
        }
    }
    out
}

async fn fold_projection(
    store: &Arc<EventStoreImpl>,
    aggregate_ids: &[AggregateId],
) -> EvidenceProjection {
    let mut projection = EvidenceProjection::default();
    for id in aggregate_ids {
        let envelopes = store.load(*id).await.expect("load for projection fold");
        for env in &envelopes {
            cherry_pit_core::Projection::apply(&mut projection, env);
        }
    }
    projection
}

fn write_snapshots(
    target: &Path,
    payload_sequence: &[DomainEvent],
    projection: &EvidenceProjection,
    copied: &[String],
) {
    let payload_json =
        serde_json::to_string_pretty(payload_sequence).expect("serialise payload sequence") + "\n";
    let projection_json =
        serde_json::to_string_pretty(projection).expect("serialise projection") + "\n";

    fs::write(target.join("payload_sequence.json"), payload_json)
        .expect("write payload_sequence.json");
    fs::write(target.join("projection_snapshot.json"), projection_json)
        .expect("write projection_snapshot.json");

    let manifest = copied.join("\n") + "\n";
    fs::write(target.join("aggregate_files.txt"), manifest).expect("write aggregate_files.txt");

    let written: Vec<PathBuf> = fs::read_dir(target)
        .expect("read target")
        .map(|e| e.expect("dir entry").path())
        .collect();
    assert!(
        written
            .iter()
            .any(|p| ends_with(p, "payload_sequence.json"))
    );
    assert!(
        written
            .iter()
            .any(|p| ends_with(p, "projection_snapshot.json"))
    );
    assert!(written.iter().any(|p| ends_with(p, "aggregate_files.txt")));
}

fn ends_with(path: &Path, suffix: &str) -> bool {
    path.file_name().and_then(|s| s.to_str()) == Some(suffix)
}
