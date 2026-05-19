//! Integration test for stale-lock evidence on [`PardosaFileEventStore`].
//!
//! Mirrors `crates/cherry-pit-gateway/tests/stale_lock_evidence.rs`.
//! Reuses [`cherry_pit_gateway::stale_lock_evidence`] verbatim — the
//! helper is `Path`-generic (it inspects `{dir}/.lock` filesystem
//! metadata) and is part of cherry-pit-gateway's public surface. The
//! `.lock` sentinel convention is identical between
//! `MsgpackFileStore` and `PardosaFileEventStore` per CHE-0043:R1,
//! so duplicating the helper would be unprincipled.
//!
//! Three states:
//!
//! 1. Empty directory — no `.lock` present yet ⇒ `Ok(None)`.
//! 2. After store construction — `.lock` exists (per CHE-0043:R1
//!    advisory `flock`) ⇒ `Ok(Some(_))` with matching filesystem
//!    metadata.
//! 3. Non-existent directory — treated as absence ⇒ `Ok(None)`.

use cherry_pit_core::DomainEvent;
use cherry_pit_gateway::stale_lock_evidence;
use cherry_pit_pardosa::PardosaFileEventStore;
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

impl pardosa_encoding::Decode for TestEvent {
    fn decode(d: &mut pardosa_encoding::Decoder<'_>) -> Result<Self, pardosa_encoding::EventError> {
        let tag = <u8 as pardosa_encoding::Decode>::decode(d)?;
        match tag {
            0 => Ok(Self::Tick),
            _ => Err(pardosa_encoding::EventError::InvalidInput),
        }
    }
}

#[test]
fn stale_lock_evidence_reports_filesystem_metadata() {
    let dir = tempfile::tempdir().unwrap();

    // ── 1. Fresh tempdir: no .lock yet ──────────────────────────────
    let none = stale_lock_evidence(dir.path()).expect("metadata probe succeeds");
    assert!(none.is_none(), "no .lock present before store construction");

    // ── 2. Trigger lock acquisition via PardosaFileEventStore::open ─
    let _store = PardosaFileEventStore::<TestEvent>::open(dir.path())
        .expect("open succeeds on fresh tempdir");

    // Sanity: .lock now exists per CHE-0043:R1.
    let lock_path = dir.path().join(".lock");
    assert!(
        lock_path.exists(),
        ".lock sentinel must exist after open (CHE-0043:R1)"
    );

    // ── 3. Helper now reports evidence ──────────────────────────────
    let evidence = stale_lock_evidence(dir.path())
        .expect("metadata probe succeeds")
        .expect("lock should be present after open");

    assert!(
        evidence.lock_path.ends_with(".lock"),
        "lock_path must end in .lock, got {:?}",
        evidence.lock_path
    );
    assert_eq!(
        evidence.lock_path, lock_path,
        "lock_path must be {{store_dir}}/.lock"
    );

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

/// CHE-0043:R3 — a second `open` while the first store still holds
/// the flock must surface `StoreError::StoreLocked`.
#[test]
fn second_open_returns_store_locked() {
    let dir = tempfile::tempdir().unwrap();

    let _first = PardosaFileEventStore::<TestEvent>::open(dir.path())
        .expect("first open succeeds on fresh tempdir");

    let second = PardosaFileEventStore::<TestEvent>::open(dir.path());
    match second {
        Err(cherry_pit_core::StoreError::StoreLocked { path }) => {
            assert_eq!(
                path,
                dir.path().to_path_buf(),
                "StoreLocked must carry the store dir"
            );
        }
        Err(other) => panic!("expected StoreLocked, got {other:?}"),
        Ok(_) => panic!("second open must fail while first holds the flock"),
    }
}
