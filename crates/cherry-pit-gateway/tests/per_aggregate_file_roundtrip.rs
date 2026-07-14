//! Per-aggregate-file roundtrip integration test for `MsgpackFileStore`.
//!
//! Carry from B7'b Inc 1 linus review (bead adr-fmt-8yde). Exercises
//! create→append→reopen→load on-disk, asserting: sequence order
//! preserved (1, 2, 3); payload equality across the persistence
//! boundary; exactly one `*.msgpack` file per aggregate (CHE-0036
//! file-per-stream, CHE-0048 per-aggregate-file).
//!
//! Integration test, `tempfile`-isolated (CHE-0038:R5); `EventStore`
//! is async (CHE-0025 RPITIT), so `#[tokio::test]` drives it —
//! CHE-0038:R5's "sync" framing means no spawned services/processes,
//! not literal synchronous code.
//!
//! # Assumptions (CHE-0030 — public API only)
//!
//! Three envelopes come via `create` (1 event) then `append` (2
//! events), staying on the public `EventStore` surface. File-count
//! filters on `.msgpack` to exclude `.lock` (CHE-0043:R1); filename
//! format is documented but treated as an implementation detail —
//! asserting count, not filename, keeps the test resilient to
//! internal renames.

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

#[tokio::test]
async fn per_aggregate_file_roundtrip_preserves_sequence_and_payload() {
    let dir = tempfile::tempdir().unwrap();

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
    };

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

    assert_eq!(loaded[0].sequence().get(), 1);
    assert_eq!(loaded[1].sequence().get(), 2);
    assert_eq!(loaded[2].sequence().get(), 3);

    assert_eq!(loaded[0].aggregate_id(), id);
    assert_eq!(loaded[1].aggregate_id(), id);
    assert_eq!(loaded[2].aggregate_id(), id);

    assert_eq!(
        *loaded[0].payload(),
        TestEvent::Created {
            name: "alpha".into()
        }
    );
    assert_eq!(*loaded[1].payload(), TestEvent::Updated { value: 10 });
    assert_eq!(*loaded[2].payload(), TestEvent::Updated { value: 20 });

    assert_eq!(loaded[0].event_id(), created[0].event_id());
    assert_eq!(loaded[0].timestamp(), created[0].timestamp());

    let mut msgpack_count = 0;
    let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".msgpack") && !name.ends_with(".msgpack.tmp") {
            msgpack_count += 1;
        }
    }
    assert_eq!(
        msgpack_count, 1,
        "CHE-0048: exactly one .msgpack file per aggregate, got {msgpack_count}"
    );
}
