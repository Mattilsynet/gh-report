//! Per-aggregate-file roundtrip integration test for `MsgpackFileStore`.
//!
//! Carry from B7'b Inc 1 linus review (bead adr-fmt-8yde). Exercises the
//! full create→append→reopen→load path against a real on-disk file under
//! `tempfile::TempDir`, asserting:
//!
//! - sequence order preserved (1, 2, 3)
//! - payload equality across the persistence boundary
//! - exactly one `*.msgpack` file exists for the aggregate under the
//!   store directory (CHE-0036 file-per-stream, CHE-0048 per-aggregate-file
//!   invariant)
//!
//! Test category per CHE-0038: integration test with `tempfile` isolation
//! (CHE-0038:R5). The `EventStore` API is async (CHE-0025 RPITIT), so the
//! test uses `#[tokio::test]` — the "sync" framing in CHE-0038:R5 means
//! "no spawned services / processes", not literally synchronous code.
//!
//! # Assumptions (CHE-0030 — public API only)
//!
//! - Three envelopes for one aggregate are produced via `create` (1 event)
//!   then `append` (2 events) rather than hand-constructing `EventEnvelope`
//!   values. This stays on the public `EventStore` trait surface and is the
//!   most reversible interpretation of the brief's "N=3 envelopes for one
//!   `AggregateId`". The brief's `expected_sequence=0` shorthand corresponds
//!   to `nz(1)` here because `create` always lands sequence 1, so the next
//!   `append` expects `actual_sequence == 1`.
//! - File-count assertion filters on the `.msgpack` extension to exclude
//!   the `.lock` sentinel file (CHE-0043:R1 process fencing). Without the
//!   filter the directory would contain 2 entries — `.lock` plus the
//!   aggregate file — and the invariant under test (CHE-0048) is about
//!   aggregate files specifically, not all directory entries.
//! - Filename format (`{id}.msgpack`) is observable from the public
//!   docstring on `MsgpackFileStore` ("File layout" section) but treated
//!   as an implementation detail here — we assert on count, not on the
//!   exact filename. This keeps the test resilient to internal renames
//!   while still pinning the one-file-per-aggregate invariant.

use std::num::NonZeroU64;

use cherry_pit_core::{CorrelationContext, DomainEvent, EventStore};
use cherry_pit_gateway::MsgpackFileStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    Created { name: String },
    Updated { value: u32 },
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Created { .. } => "test.created",
            Self::Updated { .. } => "test.updated",
        }
    }
}

impl pardosa_encoding::Encode for TestEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Created { name } => {
                out.push(0u8);
                pardosa_encoding::Encode::encode(name, out);
            }
            Self::Updated { value } => {
                out.push(1u8);
                pardosa_encoding::Encode::encode(value, out);
            }
        }
    }
}

#[tokio::test]
async fn per_aggregate_file_roundtrip_preserves_sequence_and_payload() {
    let dir = tempfile::tempdir().unwrap();

    // ── Phase 1: write via first store instance ─────────────────────
    let (id, created) = {
        let store = MsgpackFileStore::<TestEvent>::new(dir.path());

        let (id, created) = store
            .create(
                vec![TestEvent::Created {
                    name: "alpha".into(),
                }],
                CorrelationContext::none(),
            )
            .await
            .expect("create succeeds on fresh tempdir");

        // Append two more events at expected_sequence=1 (the sequence
        // landed by `create`).
        let appended = store
            .append(
                id,
                NonZeroU64::new(1).unwrap(),
                vec![
                    TestEvent::Updated { value: 10 },
                    TestEvent::Updated { value: 20 },
                ],
                CorrelationContext::none(),
            )
            .await
            .expect("append succeeds after create");

        assert_eq!(created.len(), 1, "create returns one envelope");
        assert_eq!(appended.len(), 2, "append returns two envelopes");

        (id, created)
        // store dropped here → no in-memory cache survives.
    };

    // ── Phase 2: reconstruct at same path, force on-disk read ───────
    let store = MsgpackFileStore::<TestEvent>::new(dir.path());
    let loaded = store
        .load(id)
        .await
        .expect("load from fresh store instance succeeds");

    assert_eq!(
        loaded.len(),
        3,
        "all three envelopes survive the persistence boundary"
    );

    // Sequence order preserved (1, 2, 3) per CHE-0036 file-per-stream.
    assert_eq!(loaded[0].sequence().get(), 1);
    assert_eq!(loaded[1].sequence().get(), 2);
    assert_eq!(loaded[2].sequence().get(), 3);

    // All envelopes belong to the same aggregate.
    assert_eq!(loaded[0].aggregate_id(), id);
    assert_eq!(loaded[1].aggregate_id(), id);
    assert_eq!(loaded[2].aggregate_id(), id);

    // Payload equality across the persistence boundary.
    assert_eq!(
        *loaded[0].payload(),
        TestEvent::Created {
            name: "alpha".into()
        }
    );
    assert_eq!(*loaded[1].payload(), TestEvent::Updated { value: 10 });
    assert_eq!(*loaded[2].payload(), TestEvent::Updated { value: 20 });

    // First envelope on disk equals the one returned by `create` —
    // event_id, timestamp, correlation/causation all survive.
    assert_eq!(loaded[0].event_id(), created[0].event_id());
    assert_eq!(loaded[0].timestamp(), created[0].timestamp());

    // ── Phase 3: per-aggregate-file invariant (CHE-0048) ────────────
    // Count *.msgpack files only — `.lock` sentinel (CHE-0043:R1) is
    // not an aggregate file. The invariant is: one aggregate ⇒ one
    // `.msgpack` file.
    let mut msgpack_count = 0;
    let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Exclude `.msgpack.tmp` orphans (none expected, but be precise).
        if name.ends_with(".msgpack") && !name.ends_with(".msgpack.tmp") {
            msgpack_count += 1;
        }
    }
    assert_eq!(
        msgpack_count, 1,
        "CHE-0048: exactly one .msgpack file per aggregate, got {msgpack_count}"
    );
}
