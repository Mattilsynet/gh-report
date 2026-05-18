//! Integration test for [`cherry_pit_gateway::stale_lock_evidence`].
//!
//! Covers CHE-0047:R5 — when a process observes
//! [`cherry_pit_core::error::StoreError::StoreLocked`], the gateway
//! exposes a helper that produces filesystem-metadata evidence of the
//! `.lock` sentinel suitable for inclusion in incident records.
//!
//! The test exercises three states:
//!
//! 1. Empty directory — no `.lock` present yet ⇒ `Ok(None)`.
//! 2. After first store write — `.lock` exists (per CHE-0043:R1
//!    advisory `flock`) ⇒ `Ok(Some(_))` with matching filesystem
//!    metadata.
//! 3. Non-existent directory — treated as absence ⇒ `Ok(None)`.

use cherry_pit_core::{CorrelationContext, DomainEvent, EventStore};
use cherry_pit_gateway::{MsgpackFileStore, stale_lock_evidence};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum TestEvent {
    Tick,
}

impl DomainEvent for TestEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Tick => "test.tick",
        }
    }
}

impl pardosa_encoding::Encode for TestEvent {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Tick => out.push(0u8),
        }
    }
}

#[tokio::test]
async fn stale_lock_evidence_reports_filesystem_metadata() {
    let dir = tempfile::tempdir().unwrap();

    // ── 1. Fresh tempdir: no .lock yet ──────────────────────────────
    let none = stale_lock_evidence(dir.path()).expect("metadata probe succeeds");
    assert!(none.is_none(), "no .lock present before first write");

    // ── 2. Trigger lock acquisition via a real store write ──────────
    let store = MsgpackFileStore::<TestEvent>::new(dir.path());
    store
        .create(vec![TestEvent::Tick], CorrelationContext::none())
        .await
        .expect("create succeeds on fresh tempdir");

    // Sanity: .lock now exists per CHE-0043:R1.
    let lock_path = dir.path().join(".lock");
    assert!(
        lock_path.exists(),
        ".lock sentinel must exist after first write (CHE-0043:R1)"
    );

    // ── 3. Helper now reports evidence ──────────────────────────────
    let evidence = stale_lock_evidence(dir.path())
        .expect("metadata probe succeeds")
        .expect("lock should be present after first write");

    assert!(
        evidence.lock_path.ends_with(".lock"),
        "lock_path must end in .lock, got {:?}",
        evidence.lock_path
    );
    assert_eq!(
        evidence.lock_path, lock_path,
        "lock_path must be {{store_dir}}/.lock"
    );

    // Size must match filesystem (sentinel is typically zero-length;
    // we only assert agreement with the actual file metadata, not a
    // specific value, since the implementation reserves the right to
    // write a marker).
    let fs_meta = std::fs::metadata(&lock_path).expect("stat .lock");
    assert_eq!(
        evidence.lock_size,
        fs_meta.len(),
        "lock_size must match filesystem metadata"
    );
    assert_eq!(
        evidence.lock_mtime,
        fs_meta.modified().unwrap(),
        "lock_mtime must match filesystem metadata"
    );

    // ── 4. Absent-dir case ──────────────────────────────────────────
    let absent = dir.path().join("does-not-exist");
    let none2 = stale_lock_evidence(&absent).expect("absent dir is not an error");
    assert!(none2.is_none(), "absent dir reports None, not Err");
}
