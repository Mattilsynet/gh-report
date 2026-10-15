use gh_report::app::write_policy::{WritePolicyCategory, write_with_policy_sync};
use gh_report::error::PersistenceError;
use gh_report::event::DomainEvent;
use gh_report::store::{NativeStore, StoreError};
use pardosa::prelude::*;

#[test]
fn native_candidate_records_reopens_and_replays() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.pgno");
    let event = DomainEvent::RepositoryDeleted {
        domain_key: NonEmptyEventString::new("repo-1").unwrap(),
        repo_name: NonEmptyEventString::new("repo-1").unwrap(),
        detected_at: Timestamp::new(42).unwrap(),
    };
    let store = NativeStore::create_pgno(&path).unwrap();
    store.record("repo-1", event.clone()).unwrap();
    drop(store);
    let reopened = NativeStore::open_pgno(&path).unwrap();
    assert_eq!(reopened.events().unwrap(), vec![(false, event)]);
    assert_eq!(gh_report::config::EVIDENCE_SCHEMA_VERSION, "25.0");
}

#[test]
fn uncertainty_stops_after_one_attempt_and_preserves_typed_source() {
    let mut calls = 0;
    let failure = write_with_policy_sync(|| {
        calls += 1;
        Err(PersistenceError::Indeterminate(Box::new(
            StoreError::Indeterminate { carried_epoch: 17 },
        )))
    })
    .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(
        failure.category,
        WritePolicyCategory::ReconciliationRequired
    );
    let PersistenceError::Indeterminate(error) = failure.error else {
        panic!("typed source lost");
    };
    assert!(matches!(
        error.downcast_ref::<StoreError>(),
        Some(StoreError::Indeterminate { carried_epoch: 17 })
    ));
}

#[test]
fn native_operation_preserves_ownership_condition() {
    let failure = OperationFailure::new(FailureCondition::OwnershipUnestablished, "no authority");
    let StoreError::Operation(failure) = StoreError::from(failure) else {
        panic!("typed ownership condition lost");
    };
    assert_eq!(
        failure.condition(),
        &FailureCondition::OwnershipUnestablished
    );
}
