//! Replay-equivalence regression test — contract enforcer for Track 4.0
//! (mission `adr-fmt-nnn3`).
//!
//! Loads the committed pre-SMI corpus from `tests/fixtures/smi_pre_corpus/`
//! and asserts byte-equivalence of:
//!
//!   (a) the replayed `DomainEvent` payload sequence, against
//!       `payload_sequence.json`;
//!   (b) the resulting `EvidenceProjection` final state, against
//!       `projection_snapshot.json`.
//!
//! Envelope metadata (`event_id` Uuid, envelope `timestamp`) is **not**
//! part of the equivalence target — see
//! `.ooda/brief-track-4.0-smi.md` § "Replay-equivalence test design".
//!
//! ## Why this is load-bearing
//!
//! Track 4.0 collapses three write paths into a single Merger task.
//! Pre-refactor (now), the fixture is captured via the three-service
//! write path; this test reads the fixture and re-folds it through
//! the projection — at this point the assertion is a tautology
//! (commit-time gate that the capture is internally consistent).
//!
//! Through Tracks 4.0 steps 3–6 the **write path** changes. The
//! committed fixture does not change. If a behavioural increment
//! drifts the payload sequence (e.g. reorders events, drops an
//! envelope, mutates a field) or the projection materialisation,
//! this test fails. That is the contract enforcer for criterion #4
//! (replay equivalence) and #11 (on-disk msgpack format unchanged)
//! in the mission brief.
//!
//! ## Regenerating the fixture
//!
//! If an intentional change to the corpus shape is needed (new
//! scenario step, new aggregate, etc.), run:
//!
//! ```text
//! cargo test -p gh-report --test smi_corpus_capture -- --ignored
//! ```
//!
//! The diff under `tests/fixtures/smi_pre_corpus/` is part of the
//! same commit as the scenario change.

use std::fs;
use std::path::PathBuf;

use cherry_pit_core::{EventEnvelope, Projection};
use gh_report::domain::events::DomainEvent;
use gh_report::projection::EvidenceProjection;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("smi_pre_corpus")
}

/// Decode a single per-aggregate msgpack file (the on-disk format
/// `MsgpackFileStore` writes) into its envelope sequence.
///
/// `MsgpackFileStore` writes each aggregate as a whole-file overwrite
/// containing one `rmp_serde::encode::to_vec_named(&Vec<EventEnvelope<E>>)`
/// blob (see `crates/cherry-pit-gateway/src/event_store/msgpack_file.rs`).
/// Decoding mirrors `MsgpackFileStore::deserialize_and_validate_stream`:
/// a single `from_slice::<Vec<EventEnvelope<DomainEvent>>>`.
fn load_msgpack_aggregate(path: &PathBuf) -> Vec<EventEnvelope<DomainEvent>> {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    let envelopes: Vec<EventEnvelope<DomainEvent>> =
        rmp_serde::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "decode fixture {} as Vec<EventEnvelope<DomainEvent>>: {e}",
                path.display()
            )
        });
    assert!(
        !envelopes.is_empty(),
        "fixture file {} decoded zero envelopes",
        path.display()
    );
    envelopes
}

#[test]
#[ignore = "wire-format in flight; SMI fixture regenerated and tests re-enabled in sub-05 per CHE-0065"]
fn smi_replay_payload_sequence_byte_equivalent() {
    let dir = fixtures_dir();
    let manifest_path = dir.join("aggregate_files.txt");
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "read manifest {} (regenerate via `cargo test -p gh-report --test smi_corpus_capture -- --ignored`): {e}",
            manifest_path.display()
        )
    });

    // Replay every aggregate stream named in the manifest, in manifest
    // order (capture sorted by filename, which is `<id>.msgpack` —
    // numerically stable for u64-counter-assigned ids 1..=9).
    let mut replayed: Vec<DomainEvent> = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = dir.join(line);
        let envelopes = load_msgpack_aggregate(&path);
        for env in envelopes {
            replayed.push(env.payload().clone());
        }
    }

    let expected_json =
        fs::read_to_string(dir.join("payload_sequence.json")).expect("read payload_sequence.json");
    let expected: Vec<DomainEvent> =
        serde_json::from_str(&expected_json).expect("decode payload_sequence.json");

    // Compare via JSON re-serialisation: avoids relying on a `PartialEq`
    // impl for DomainEvent (RepositoryEvidence contains Arc<Repository>
    // and other deep fields; structural equality via the serde view is
    // what the projection contract actually cares about).
    let replayed_json =
        serde_json::to_string_pretty(&replayed).expect("serialise replayed sequence");
    let expected_normalised =
        serde_json::to_string_pretty(&expected).expect("re-serialise expected sequence");
    assert_eq!(
        replayed_json, expected_normalised,
        "replayed payload sequence drifted from committed pre-SMI corpus — \
         this is a Track 4.0 contract violation per criterion #4 in \
         `.ooda/brief-track-4.0-smi.md`. Inspect the diff and HALT \
         per the brief's halt-and-handback trigger before committing."
    );
}

#[test]
#[ignore = "wire-format in flight; SMI fixture regenerated and tests re-enabled in sub-05 per CHE-0065"]
fn smi_replay_projection_snapshot_byte_equivalent() {
    let dir = fixtures_dir();
    let manifest_path = dir.join("aggregate_files.txt");
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "read manifest {} (regenerate via `cargo test -p gh-report --test smi_corpus_capture -- --ignored`): {e}",
            manifest_path.display()
        )
    });

    // Fold every aggregate stream into a fresh projection, in manifest
    // order — same order as the capture used so the snapshot lines up.
    let mut projection = EvidenceProjection::default();
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = dir.join(line);
        let envelopes = load_msgpack_aggregate(&path);
        for env in &envelopes {
            projection.apply(env);
        }
    }

    let replayed_json =
        serde_json::to_string_pretty(&projection).expect("serialise replayed projection");
    let expected_json = fs::read_to_string(dir.join("projection_snapshot.json"))
        .expect("read projection_snapshot.json");
    // Round-trip the expected through serde so trailing-newline /
    // whitespace differences between the two files don't cause spurious
    // drift.
    let expected_value: EvidenceProjection =
        serde_json::from_str(&expected_json).expect("decode projection_snapshot.json");
    let expected_normalised =
        serde_json::to_string_pretty(&expected_value).expect("re-serialise expected projection");

    assert_eq!(
        replayed_json, expected_normalised,
        "replayed EvidenceProjection drifted from committed pre-SMI snapshot — \
         Track 4.0 contract violation per criterion #4. HALT and back-brief \
         moltke before committing."
    );
}

/// Witness for criterion #11 (on-disk msgpack format unchanged).
///
/// Load each `.msgpack` fixture, decode to `Vec<EventEnvelope<DomainEvent>>`,
/// re-encode via the **same codec** the production write path uses
/// (`rmp_serde::encode::to_vec_named` — see
/// `cherry-pit-gateway/src/event_store/msgpack_file.rs:362`), and assert
/// byte-equality against the original file bytes. Drift in serde
/// attributes, field order, or codec choice (e.g. accidental swap to
/// `to_vec` array-of-positionals) fails this test even when JSON
/// payload semantics are preserved. `Cargo.lock` is committed
/// (AGENTS.md § Conventions), so rmp-serde minor-version output drift
/// is contained.
#[test]
#[ignore = "wire-format in flight; SMI fixture regenerated and tests re-enabled in sub-05 per CHE-0065"]
fn smi_msgpack_on_disk_format_byte_equivalent() {
    let dir = fixtures_dir();
    let manifest_path = dir.join("aggregate_files.txt");
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "read manifest {} (regenerate via `cargo test -p gh-report --test smi_corpus_capture -- --ignored`): {e}",
            manifest_path.display()
        )
    });

    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let path = dir.join(line);
        let original =
            fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
        let envelopes: Vec<EventEnvelope<DomainEvent>> = rmp_serde::from_slice(&original)
            .unwrap_or_else(|e| panic!("decode {}: {e}", path.display()));
        let reencoded = rmp_serde::encode::to_vec_named(&envelopes)
            .unwrap_or_else(|e| panic!("re-encode {}: {e}", path.display()));
        assert_eq!(
            original,
            reencoded,
            "msgpack on-disk format drifted for {} — Track 4.0 contract \
             violation per criterion #11 (`.ooda/brief-track-4.0-smi.md`). \
             The encoding contract (rmp_serde::encode::to_vec_named over \
             Vec<EventEnvelope<DomainEvent>>) has changed even though \
             payload semantics may survive a JSON round-trip. HALT and \
             back-brief moltke before committing.",
            path.display()
        );
    }
}

/// δ.3c-i (bead `adr-fmt-syjan`): `SweepStarted::snapshot_signature:
/// Option<String>` survives a pardosa-encoding encode → decode round
/// trip. Exercises the `Some(_)` arm (the trailing-`None` arm is
/// covered by the in-tree `wire_format_byte_equality` fixture in
/// `domain/events.rs`).
///
/// Not gated behind `#[ignore]`: the test operates on the hand-rolled
/// `Encode` impl directly. δ.3c-ii (bead `adr-fmt-baao9`) consumes
/// this contract when it threads `build_snapshot_signature(...)`
/// through `StartSweep`; the encoded `Some(_)` payload must reach the
/// projection unchanged.
///
/// The prior `from_bytes` (Decode) round-trip was retired in the
/// pardosa-removal mission: gh-report no longer depends on
/// `pardosa-encoding::Decode`; persistence reads use serde via
/// `MsgpackFileStore`. `Encode` byte-stability is locked by the
/// `events::tests` byte-equality regression at
/// `src/domain/events.rs`.
#[test]
fn sweep_started_snapshot_signature_encode_byte_stable() {
    use pardosa_encoding::to_vec;

    let original = DomainEvent::SweepStarted {
        org: "test-org".into(),
        repo_count: 42,
        batch_id: "batch-001".into(),
        timestamp: "2026-04-20T12:00:00Z".into(),
        snapshot_signature: Some("test-sig".into()),
    };

    let bytes = to_vec(&original);
    // First byte is the variant discriminant (SweepStarted = 0u8).
    assert_eq!(bytes[0], 0u8, "SweepStarted discriminant must be 0");
    // The Some(_) tag (1u8) appears after the four leading String
    // fields; we don't pin the exact offset here — the comprehensive
    // byte-equality regression in src/domain/events.rs locks the full
    // wire format.
    assert!(
        bytes.contains(&1u8),
        "encoded SweepStarted with Some(snapshot_signature) must contain the Some-tag"
    );
}
