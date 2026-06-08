#![forbid(unsafe_code)]
use pardosa::store::HasEventSchemaSource;
use pardosa_schema::{EventString, GenomeSafe, NonEmptyEventString, Timestamp, Validate};
/// MAX-length bounds for bounded-string event payload fields.
///
/// Doctrine: every `String` in a GenomeSafe-derived event must be wrapped in
/// `EventString<MAX>` or `NonEmptyEventString<MAX>` (rule 16). The MAX is part
/// of the schema identity — changing it requires a schema migration.
pub mod limits {
    /// Repository slug shape: "owner/name" with generous headroom.
    pub const MAX_REPO_NAME: usize = 256;
    /// Sweep batch id, expected to be short/UUID-shaped.
    pub const MAX_BATCH_ID: usize = 64;
    /// Free-form error message text.
    pub const MAX_ERROR_MESSAGE: usize = 4096;
    /// Filesystem path on the host.
    pub const MAX_PATH: usize = 4096;
    /// GitHub-like organisation name.
    pub const MAX_ORG: usize = 128;
    /// Domain key (logical bucket identifier).
    pub const MAX_DOMAIN_KEY: usize = 128;
    /// Event source label (for example, "cli").
    pub const MAX_SOURCE: usize = 64;
    /// Snapshot signature opaque token.
    pub const MAX_SNAPSHOT_SIG: usize = 256;
    /// Per-repo evidence blurb (free-form, may be empty).
    pub const MAX_EVIDENCE: usize = 4096;
}
use limits::{
    MAX_BATCH_ID, MAX_DOMAIN_KEY, MAX_ERROR_MESSAGE, MAX_EVIDENCE, MAX_ORG, MAX_REPO_NAME,
    MAX_SNAPSHOT_SIG, MAX_SOURCE,
};
/// The CLI's payload type: domain events produced by a `pardosa-cli`
/// invocation, persisted to a `.pgno` via
/// [`pardosa::store::EventStore`].
///
/// Variant layout is fixed by the `#[repr(u8)]` discriminants below.
/// Adding a variant must use a fresh discriminant — re-using a retired
/// number breaks decoders of older bytes. `SweepFailed` is `= 7` (not
/// `= 4`) to leave a gap for variants that were removed in earlier
/// iterations; this is intentional, not a typo.
#[derive(Debug, GenomeSafe)]
#[repr(u8)]
pub enum DomainEvent {
    /// A sweep started over a target organisation. Records the org, the
    /// expected repository count, the batch id (used to correlate later
    /// `SweepCompleted` / `SweepFailed`), a wall-clock timestamp, and
    /// an optional opaque snapshot signature.
    SweepStarted {
        org: NonEmptyEventString<MAX_ORG>,
        repo_count: u64,
        batch_id: NonEmptyEventString<MAX_BATCH_ID>,
        timestamp: Timestamp,
        snapshot_signature: Option<EventString<MAX_SNAPSHOT_SIG>>,
    } = 0,
    /// A repository was evaluated as part of the sweep. Carries the
    /// domain key + repo name pair, the boolean outcome, the source
    /// (e.g. "cli"), the evaluation duration in milliseconds, and an
    /// optional free-form evidence blurb.
    RepoEvaluated {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        success: bool,
        source: NonEmptyEventString<MAX_SOURCE>,
        duration_ms: u64,
        timestamp: Timestamp,
        evidence: Option<EventString<MAX_EVIDENCE>>,
    } = 1,
    /// A repository was removed from the sweep target set. Records the
    /// domain key + repo name pair and the wall-clock timestamp.
    RepoRemoved {
        domain_key: NonEmptyEventString<MAX_DOMAIN_KEY>,
        repo_name: NonEmptyEventString<MAX_REPO_NAME>,
        timestamp: Timestamp,
    } = 2,
    /// A sweep completed successfully. Records the batch id (matches an
    /// earlier `SweepStarted`), total duration, the realised repo
    /// count, and the wall-clock timestamp.
    SweepCompleted {
        batch_id: NonEmptyEventString<MAX_BATCH_ID>,
        duration_ms: u64,
        repo_count: u64,
        timestamp: Timestamp,
    } = 3,
    /// A sweep failed before completion. Records the batch id, a
    /// free-form error message, the time spent before failure, and the
    /// wall-clock timestamp.
    SweepFailed {
        batch_id: NonEmptyEventString<MAX_BATCH_ID>,
        error: EventString<MAX_ERROR_MESSAGE>,
        duration_ms: u64,
        timestamp: Timestamp,
    } = 7,
}
impl DomainEvent {
    /// Stable display name for the variant.
    ///
    /// Returns the variant identifier as a `&'static str` (e.g.
    /// `"SweepStarted"`). Used in CLI output and test assertions; the
    /// returned string is part of the CLI's observable surface and
    /// must stay stable across versions.
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match self {
            DomainEvent::SweepStarted { .. } => "SweepStarted",
            DomainEvent::RepoEvaluated { .. } => "RepoEvaluated",
            DomainEvent::RepoRemoved { .. } => "RepoRemoved",
            DomainEvent::SweepCompleted { .. } => "SweepCompleted",
            DomainEvent::SweepFailed { .. } => "SweepFailed",
        }
    }
}
impl Validate for DomainEvent {
    type Error = std::convert::Infallible;
    fn validate(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}
impl HasEventSchemaSource for DomainEvent {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("pardosa-cli/DomainEvent");
}
#[cfg(test)]
mod tests {
    use super::*;
    use pardosa_schema::DomainError;
    fn ts(nanos: u64) -> Timestamp {
        Timestamp::from_nanos(nanos).expect("nonzero nanos")
    }
    fn nes<const MAX: usize>(s: &str) -> NonEmptyEventString<MAX> {
        NonEmptyEventString::try_new(s).expect("fits MAX, nonempty")
    }
    fn es<const MAX: usize>(s: &str) -> EventString<MAX> {
        EventString::try_from(s.to_string()).expect("fits MAX")
    }
    #[test]
    fn sweep_started_structural() {
        let e = DomainEvent::SweepStarted {
            org: nes("acme"),
            repo_count: 42,
            batch_id: nes("b-001"),
            timestamp: ts(1),
            snapshot_signature: Some(es("sig-abc")),
        };
        assert_eq!(e.event_type(), "SweepStarted");
        let DomainEvent::SweepStarted {
            org,
            repo_count,
            batch_id,
            timestamp,
            snapshot_signature,
        } = e
        else {
            panic!("variant mismatch");
        };
        assert_eq!(org.as_str(), "acme");
        assert_eq!(repo_count, 42);
        assert_eq!(batch_id.as_str(), "b-001");
        assert_eq!(timestamp.as_nanos(), 1);
        assert_eq!(
            snapshot_signature.as_ref().map(EventString::as_str),
            Some("sig-abc")
        );
    }
    #[test]
    fn repo_evaluated_structural() {
        let e = DomainEvent::RepoEvaluated {
            domain_key: nes("rust"),
            repo_name: nes("acme/widget"),
            success: true,
            source: nes("github"),
            duration_ms: 1234,
            timestamp: ts(2),
            evidence: Some(es("blurb")),
        };
        assert_eq!(e.event_type(), "RepoEvaluated");
        let DomainEvent::RepoEvaluated {
            domain_key,
            repo_name,
            success,
            source,
            duration_ms,
            timestamp,
            evidence,
        } = e
        else {
            panic!("variant mismatch");
        };
        assert_eq!(domain_key.as_str(), "rust");
        assert_eq!(repo_name.as_str(), "acme/widget");
        assert!(success);
        assert_eq!(source.as_str(), "github");
        assert_eq!(duration_ms, 1234);
        assert_eq!(timestamp.as_nanos(), 2);
        assert_eq!(evidence.as_ref().map(EventString::as_str), Some("blurb"));
    }
    #[test]
    fn repo_removed_structural() {
        let e = DomainEvent::RepoRemoved {
            domain_key: nes("rust"),
            repo_name: nes("acme/widget"),
            timestamp: ts(3),
        };
        assert_eq!(e.event_type(), "RepoRemoved");
        let DomainEvent::RepoRemoved {
            domain_key,
            repo_name,
            timestamp,
        } = e
        else {
            panic!("variant mismatch");
        };
        assert_eq!(domain_key.as_str(), "rust");
        assert_eq!(repo_name.as_str(), "acme/widget");
        assert_eq!(timestamp.as_nanos(), 3);
    }
    #[test]
    fn sweep_completed_structural() {
        let e = DomainEvent::SweepCompleted {
            batch_id: nes("b-001"),
            duration_ms: 9999,
            repo_count: 42,
            timestamp: ts(4),
        };
        assert_eq!(e.event_type(), "SweepCompleted");
        let DomainEvent::SweepCompleted {
            batch_id,
            duration_ms,
            repo_count,
            timestamp,
        } = e
        else {
            panic!("variant mismatch");
        };
        assert_eq!(batch_id.as_str(), "b-001");
        assert_eq!(duration_ms, 9999);
        assert_eq!(repo_count, 42);
        assert_eq!(timestamp.as_nanos(), 4);
    }
    #[test]
    fn sweep_failed_structural() {
        let e = DomainEvent::SweepFailed {
            batch_id: nes("b-001"),
            error: es("rate-limited"),
            duration_ms: 1500,
            timestamp: ts(8),
        };
        assert_eq!(e.event_type(), "SweepFailed");
        let DomainEvent::SweepFailed {
            batch_id,
            error,
            duration_ms,
            timestamp,
        } = e
        else {
            panic!("variant mismatch");
        };
        assert_eq!(batch_id.as_str(), "b-001");
        assert_eq!(error.as_str(), "rate-limited");
        assert_eq!(duration_ms, 1500);
        assert_eq!(timestamp.as_nanos(), 8);
    }
    #[test]
    fn oversized_string_rejected_at_construction() {
        let too_long = "x".repeat(MAX_BATCH_ID + 1);
        let err = NonEmptyEventString::<MAX_BATCH_ID>::try_new(&too_long)
            .expect_err("over-MAX must reject");
        assert!(matches!(err, DomainError::TooLong { .. }));
        let err2 = NonEmptyEventString::<MAX_BATCH_ID>::try_new("")
            .expect_err("empty must reject NonEmpty");
        assert!(matches!(err2, DomainError::Empty));
        let ok_empty: Result<EventString<MAX_ERROR_MESSAGE>, _> =
            EventString::try_from(String::new());
        assert!(ok_empty.is_ok());
        let too_long_msg = "x".repeat(MAX_ERROR_MESSAGE + 1);
        let err3 = EventString::<MAX_ERROR_MESSAGE>::try_from(too_long_msg)
            .expect_err("over-MAX message must reject");
        assert!(matches!(err3, DomainError::TooLong { .. }));
    }
}
