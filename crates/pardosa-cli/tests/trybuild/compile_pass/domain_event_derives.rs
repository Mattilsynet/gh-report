use pardosa_cli::DomainEvent;
use pardosa_cli::event::limits::{
    MAX_BATCH_ID, MAX_DOMAIN_KEY, MAX_ERROR_MESSAGE, MAX_ORG, MAX_REPO_NAME, MAX_SOURCE,
};
use pardosa_schema::{NonEmptyEventString, Timestamp};
fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
    NonEmptyEventString::try_new(s).expect("nonempty fits MAX")
}
fn ts() -> Timestamp {
    Timestamp::from_nanos(1).expect("nonzero")
}
fn main() {
    let evs: [DomainEvent; 5] = [
        DomainEvent::SweepStarted {
            org: nes::<MAX_ORG>("acme"),
            repo_count: 1,
            batch_id: nes::<MAX_BATCH_ID>("b"),
            timestamp: ts(),
            snapshot_signature: None,
        },
        DomainEvent::RepoEvaluated {
            domain_key: nes::<MAX_DOMAIN_KEY>("k"),
            repo_name: nes::<MAX_REPO_NAME>("r"),
            success: true,
            source: nes::<MAX_SOURCE>("s"),
            duration_ms: 0,
            timestamp: ts(),
            evidence: None,
        },
        DomainEvent::RepoRemoved {
            domain_key: nes::<MAX_DOMAIN_KEY>("k"),
            repo_name: nes::<MAX_REPO_NAME>("r"),
            timestamp: ts(),
        },
        DomainEvent::SweepCompleted {
            batch_id: nes::<MAX_BATCH_ID>("b"),
            duration_ms: 0,
            repo_count: 1,
            timestamp: ts(),
        },
        DomainEvent::SweepFailed {
            batch_id: nes::<MAX_BATCH_ID>("b"),
            error: pardosa_schema::EventString::<MAX_ERROR_MESSAGE>::try_from(String::from("e"))
                .expect("fits MAX"),
            duration_ms: 0,
            timestamp: ts(),
        },
    ];
    for ev in &evs {
        let _name: &'static str = ev.event_type();
    }
}
