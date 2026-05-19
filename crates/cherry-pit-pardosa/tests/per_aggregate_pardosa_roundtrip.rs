//! Per-aggregate-file roundtrip integration test for
//! [`PardosaFileEventStore`].
//!
//! Mirrors `crates/cherry-pit-gateway/tests/per_aggregate_file_roundtrip.rs`
//! (the gateway's `MsgpackFileStore` analogue). Exercises the full
//! create → append → drop store → reopen at same path → load path
//! against a real on-disk file under `tempfile::TempDir`, asserting:
//!
//! - sequence order preserved (1, 2, 3) across the persistence
//!   boundary (CHE-0042:R1 — envelopes survive replay byte-identical).
//! - payload equality across the persistence boundary.
//! - event-id and timestamp of the first envelope survive (envelope
//!   identity is not re-fabricated on replay; the substrate stores
//!   what the store produced — CHE-0042:R1).
//! - exactly one `*.pardosa` file exists for the aggregate under the
//!   store directory (CHE-0036 file-per-stream + CHE-0048
//!   per-aggregate-file invariant). `.lock` (CHE-0043:R1) is
//!   excluded from the count — it is not an aggregate file.

use std::num::NonZeroU64;

use cherry_pit_core::{CorrelationContext, DomainEvent, EventStore};
use cherry_pit_pardosa::PardosaFileEventStore;
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

impl pardosa_encoding::Decode for TestEvent {
    fn decode(
        d: &mut pardosa_encoding::Decoder<'_>,
    ) -> Result<Self, pardosa_encoding::EventError> {
        let tag = <u8 as pardosa_encoding::Decode>::decode(d)?;
        match tag {
            0 => {
                let name = <String as pardosa_encoding::Decode>::decode(d)?;
                Ok(Self::Created { name })
            }
            1 => {
                let value = <u32 as pardosa_encoding::Decode>::decode(d)?;
                Ok(Self::Updated { value })
            }
            _ => Err(pardosa_encoding::EventError::InvalidInput),
        }
    }
}

#[tokio::test]
async fn per_aggregate_pardosa_roundtrip_preserves_sequence_and_payload() {
    let dir = tempfile::tempdir().unwrap();

    // ── Phase 1: write via first store instance ─────────────────────
    let (id, created) = {
        let store = PardosaFileEventStore::<TestEvent>::open(dir.path()).expect("open succeeds");

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
        // Store dropped here → flock released → no in-memory cache survives.
    };

    // ── Phase 2: reconstruct at same path, force on-disk read ───────
    let store = PardosaFileEventStore::<TestEvent>::open(dir.path())
        .expect("reopen succeeds after first store dropped");
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

    // CHE-0042:R1 — envelope identity (event_id, timestamp) survives
    // byte-identical across replay; the file store stores what the
    // inner store produced, no re-fabrication.
    assert_eq!(loaded[0].event_id(), created[0].event_id());
    assert_eq!(loaded[0].timestamp(), created[0].timestamp());

    // ── Phase 3: per-aggregate-file invariant (CHE-0048) ────────────
    // Count `*.pardosa` files only — `.lock` sentinel (CHE-0043:R1) is
    // not an aggregate file.
    let mut pardosa_count = 0;
    let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".pardosa") {
            pardosa_count += 1;
        }
    }
    assert_eq!(
        pardosa_count, 1,
        "CHE-0048: exactly one .pardosa file per aggregate, got {pardosa_count}"
    );
}
