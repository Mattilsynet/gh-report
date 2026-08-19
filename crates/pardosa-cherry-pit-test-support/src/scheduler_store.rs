use std::num::NonZeroU64;
use std::path::Path;

use cherry_pit_core::{
    AggregateId, CorrelationContext, EventEnvelope, EventStore, StoreCreateResult, StoreError,
};
use pardosa_schema::{EventBytes, EventString, GenomeSafe};

use crate::PgnoEventStore;

/// Upper bound on `caller_event_type` byte length carried through the
/// bridge-local [`SchedulerEventDto`]. Chosen generously above any
/// realistic domain event-type identifier (same bounded-newtype
/// convention as [`crate::fixture::RecordedEvent`] and the
/// `PgnoEventStore` unit-test `TestEvent`).
const CALLER_EVENT_TYPE_MAX: usize = 512;

/// Upper bound on the opaque `payload` transport carried through the
/// bridge-local [`SchedulerEventDto`]. 64 KiB comfortably covers the
/// serialized caller-event payloads this test-support bridge exercises.
const PAYLOAD_MAX: usize = 65_536;

type CallerEventTypeDto = EventString<CALLER_EVENT_TYPE_MAX>;
type PayloadDto = EventBytes<PAYLOAD_MAX>;

/// Bridge-crate-local, `GenomeSafe` DTO mirroring
/// `cherry_pit_core::SchedulerEvent` field-for-field.
///
/// Lives entirely in `pardosa-cherry-pit-test-support` per the
/// CHE-0029:R4/R6 severance ruling (oracle ghr-ad3cb725 Q3):
/// `cherry_pit_core::SchedulerEvent` itself never derives `GenomeSafe`,
/// and `cherry-pit-core` gains no `pardosa` dependency edge. Every
/// `SchedulerEvent` field has a corresponding DTO field of the same
/// semantic content; `fire_at` is carried as `i128` nanoseconds (not
/// the lossy `i64` used elsewhere in this crate for "now" timestamps)
/// so the conversion never truncates a caller-supplied instant.
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
#[repr(u8)]
pub enum SchedulerEventDto {
    /// Mirrors `SchedulerEvent::Armed`.
    Armed(ScheduleArmedDto) = 0,
    /// Mirrors `SchedulerEvent::Fired`.
    Fired(ScheduleFiredDto) = 1,
    /// Mirrors `SchedulerEvent::Cancelled`.
    Cancelled(ScheduleCancelledDto) = 2,
}

/// DTO mirror of `cherry_pit_core::ScheduleArmed`.
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct ScheduleArmedDto {
    schedule_id: uuid::Uuid,
    fire_at_nanos: i128,
    target_aggregate: u64,
    caller_event_id: uuid::Uuid,
    caller_event_type: CallerEventTypeDto,
    payload: PayloadDto,
    correlation_id: Option<uuid::Uuid>,
    causation_id: Option<uuid::Uuid>,
}

/// DTO mirror of `cherry_pit_core::ScheduleFired`.
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct ScheduleFiredDto {
    schedule_id: uuid::Uuid,
    target_aggregate: u64,
    caller_event_id: uuid::Uuid,
    caller_event_type: CallerEventTypeDto,
    payload: PayloadDto,
    correlation_id: Option<uuid::Uuid>,
    causation_id: Option<uuid::Uuid>,
}

/// DTO mirror of `cherry_pit_core::ScheduleCancelled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, GenomeSafe)]
pub struct ScheduleCancelledDto {
    schedule_id: uuid::Uuid,
}

impl cherry_pit_core::DomainEvent for SchedulerEventDto {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Armed(_) => "scheduler.schedule_armed",
            Self::Fired(_) => "scheduler.schedule_fired",
            Self::Cancelled(_) => "scheduler.schedule_cancelled",
        }
    }
}

impl serde::Serialize for SchedulerEventDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut bytes = Vec::new();
        pardosa_schema::Encode::encode(self, &mut bytes);
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de> serde::Deserialize<'de> for SchedulerEventDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        pardosa_schema::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// Error converting a `cherry_pit_core::SchedulerEvent` into its
/// bridge-local `GenomeSafe` DTO.
///
/// The mapping is total and field-preserving for every
/// `SchedulerEvent` value whose transport fields fit the DTO's bounded
/// newtypes ([`CALLER_EVENT_TYPE_MAX`], [`PAYLOAD_MAX`]) — the same
/// bounded-newtype convention already used by this crate's other
/// `GenomeSafe` fixtures. A field exceeding those bounds is the only
/// unmappable case; it surfaces here rather than corrupting data
/// silently.
#[derive(Debug)]
pub enum SchedulerEventConversionError {
    /// `caller_event_type` exceeded [`CALLER_EVENT_TYPE_MAX`] bytes.
    CallerEventTypeTooLong { actual: usize },
    /// `payload` exceeded [`PAYLOAD_MAX`] bytes.
    PayloadTooLong { actual: usize },
}

impl std::fmt::Display for SchedulerEventConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CallerEventTypeTooLong { actual } => write!(
                f,
                "caller_event_type is {actual} bytes, exceeds bound {CALLER_EVENT_TYPE_MAX}"
            ),
            Self::PayloadTooLong { actual } => {
                write!(f, "payload is {actual} bytes, exceeds bound {PAYLOAD_MAX}")
            }
        }
    }
}

impl std::error::Error for SchedulerEventConversionError {}

fn caller_event_type_dto(value: &str) -> Result<CallerEventTypeDto, SchedulerEventConversionError> {
    CallerEventTypeDto::try_from(value.to_string()).map_err(|_| {
        SchedulerEventConversionError::CallerEventTypeTooLong {
            actual: value.len(),
        }
    })
}

fn payload_dto(value: &[u8]) -> Result<PayloadDto, SchedulerEventConversionError> {
    PayloadDto::try_from(value.to_vec()).map_err(|_| {
        SchedulerEventConversionError::PayloadTooLong {
            actual: value.len(),
        }
    })
}

/// Convert a `SchedulerEvent` into its bridge-local DTO.
///
/// # Errors
///
/// Returns [`SchedulerEventConversionError`] when `caller_event_type`
/// or `payload` exceeds the DTO's bounded newtype capacity.
pub fn to_dto(
    event: &cherry_pit_core::SchedulerEvent,
) -> Result<SchedulerEventDto, SchedulerEventConversionError> {
    use cherry_pit_core::SchedulerEvent;
    Ok(match event {
        SchedulerEvent::Armed(armed) => SchedulerEventDto::Armed(ScheduleArmedDto {
            schedule_id: armed.schedule_id().as_uuid(),
            fire_at_nanos: armed.fire_at().as_nanosecond(),
            target_aggregate: armed.target_aggregate().get(),
            caller_event_id: armed.caller_event_id(),
            caller_event_type: caller_event_type_dto(armed.caller_event_type())?,
            payload: payload_dto(armed.payload())?,
            correlation_id: armed.correlation().correlation_id(),
            causation_id: armed.correlation().causation_id(),
        }),
        SchedulerEvent::Fired(fired) => SchedulerEventDto::Fired(ScheduleFiredDto {
            schedule_id: fired.schedule_id().as_uuid(),
            target_aggregate: fired.target_aggregate().get(),
            caller_event_id: fired.caller_event_id(),
            caller_event_type: caller_event_type_dto(fired.caller_event_type())?,
            payload: payload_dto(fired.payload())?,
            correlation_id: fired.correlation().correlation_id(),
            causation_id: fired.correlation().causation_id(),
        }),
        SchedulerEvent::Cancelled(cancelled) => {
            SchedulerEventDto::Cancelled(ScheduleCancelledDto {
                schedule_id: cancelled.schedule_id().as_uuid(),
            })
        }
    })
}

fn target_aggregate_from(raw: u64) -> Result<AggregateId, StoreError> {
    NonZeroU64::new(raw).map(AggregateId::new).ok_or_else(|| {
        StoreError::CorruptData(Box::<dyn std::error::Error + Send + Sync>::from(
            "scheduler DTO target_aggregate must be non-zero",
        ))
    })
}

/// Convert a bridge-local DTO back into a `SchedulerEvent`.
///
/// Total and infallible: every DTO field has exactly one corresponding
/// `SchedulerEvent` field and no bound can be violated on the way back
/// (the DTO's bounded newtypes already enforce their own bound).
///
/// # Errors
///
/// Returns [`StoreError::CorruptData`] only if a persisted
/// `target_aggregate` or `fire_at_nanos` value is structurally
/// impossible (zero aggregate id; out-of-range timestamp) — defense in
/// depth against corrupted `.pgno` bytes, not a normal conversion path.
pub fn from_dto(dto: SchedulerEventDto) -> Result<cherry_pit_core::SchedulerEvent, StoreError> {
    use cherry_pit_core::SchedulerEvent;
    use cherry_pit_core::{ScheduleArmed, ScheduleCancelled, ScheduleFired};

    Ok(match dto {
        SchedulerEventDto::Armed(armed) => {
            let target_aggregate = target_aggregate_from(armed.target_aggregate)?;
            let fire_at = jiff::Timestamp::from_nanosecond(armed.fire_at_nanos)
                .map_err(|e| StoreError::CorruptData(Box::new(e)))?;
            SchedulerEvent::Armed(ScheduleArmed::new(
                cherry_pit_core::ScheduleId::from_uuid(armed.schedule_id),
                fire_at,
                target_aggregate,
                armed.caller_event_id,
                armed.caller_event_type.as_str().to_string(),
                armed.payload.as_slice().to_vec(),
                correlation_from(armed.correlation_id, armed.causation_id),
            ))
        }
        SchedulerEventDto::Fired(fired) => {
            let target_aggregate = target_aggregate_from(fired.target_aggregate)?;
            SchedulerEvent::Fired(ScheduleFired::from_armed(&ScheduleArmed::new(
                cherry_pit_core::ScheduleId::from_uuid(fired.schedule_id),
                jiff::Timestamp::UNIX_EPOCH,
                target_aggregate,
                fired.caller_event_id,
                fired.caller_event_type.as_str().to_string(),
                fired.payload.as_slice().to_vec(),
                correlation_from(fired.correlation_id, fired.causation_id),
            )))
        }
        SchedulerEventDto::Cancelled(cancelled) => {
            SchedulerEvent::Cancelled(ScheduleCancelled::new(
                cherry_pit_core::ScheduleId::from_uuid(cancelled.schedule_id),
            ))
        }
    })
}

fn correlation_from(
    correlation_id: Option<uuid::Uuid>,
    causation_id: Option<uuid::Uuid>,
) -> CorrelationContext {
    match (correlation_id, causation_id) {
        (Some(c), Some(k)) => CorrelationContext::new(c, k),
        (Some(c), None) => CorrelationContext::correlated(c),
        (None, _) => CorrelationContext::none(),
    }
}

fn conversion_error(error: SchedulerEventConversionError) -> StoreError {
    StoreError::Infrastructure(Box::new(error))
}

fn remap_envelope(
    envelope: &EventEnvelope<SchedulerEventDto>,
) -> Result<EventEnvelope<cherry_pit_core::SchedulerEvent>, StoreError> {
    let event_id = envelope.event_id();
    let aggregate_id = envelope.aggregate_id();
    let sequence = envelope.sequence();
    let timestamp = envelope.timestamp();
    let correlation_id = envelope.correlation_id();
    let causation_id = envelope.causation_id();
    let payload = from_dto(envelope.payload().clone())?;
    EventEnvelope::new(
        event_id,
        aggregate_id,
        sequence,
        timestamp,
        correlation_id,
        causation_id,
        payload,
    )
    .map_err(|e| StoreError::CorruptData(Box::new(e)))
}

/// `.pgno`-backed [`EventStore`]`<Event = cherry_pit_core::SchedulerEvent>`.
///
/// `DurableScheduler` pins its store parameter to the CONCRETE
/// `cherry_pit_core::SchedulerEvent` (not `GenomeSafe` — CHE-0029:R4/R6
/// forbids deriving it there). This store satisfies that pin on the
/// outside while persisting through the same `.pgno` /
/// `ObservedFiberStore` facade [`PgnoEventStore`] uses, by internally
/// converting every event to/from [`SchedulerEventDto`] at the
/// boundary.
///
/// # Design: composition over a generic converter
///
/// Implemented as a thin wrapper delegating to
/// `PgnoEventStore<SchedulerEventDto>` rather than adding a generic
/// `PgnoEventStore<Ev, Dto, Converter>` variant. The `SchedulerEvent
/// <-> SchedulerEventDto` mapping is a fixed 1:1 relationship with
/// exactly one consumer; a generic converter parameter would add an
/// indirection layer (a converter trait plus its own bound set) that
/// only this single call site would ever instantiate. Delegation
/// reuses 100% of `PgnoEventStore`'s substrate logic — per-aggregate
/// locking, optimistic-concurrency sequence checks, single-event-only
/// atomicity, and restart recovery — with zero duplicated logic; only
/// the boundary conversion is new code.
pub struct PgnoSchedulerStore {
    inner: PgnoEventStore<SchedulerEventDto>,
}

impl PgnoSchedulerStore {
    /// Create a fresh `.pgno`-backed scheduler store, truncating any
    /// existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot
    /// create the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: PgnoEventStore::create_pgno(path)?,
        })
    }

    /// Open an existing `.pgno`-backed scheduler store, rehydrating
    /// its fibers and seeding the `AggregateId` counter from the max
    /// id observed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open
    /// or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: PgnoEventStore::open_pgno(path)?,
        })
    }
}

impl EventStore for PgnoSchedulerStore {
    type Event = cherry_pit_core::SchedulerEvent;

    async fn load(&self, id: AggregateId) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        let envelopes = self.inner.load(id).await?;
        envelopes.iter().map(remap_envelope).collect()
    }

    async fn create(
        &self,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> StoreCreateResult<Self::Event> {
        let dtos = events
            .iter()
            .map(to_dto)
            .collect::<Result<Vec<_>, _>>()
            .map_err(conversion_error)?;
        let (id, envelopes) = self.inner.create(dtos, context).await?;
        let remapped = envelopes
            .iter()
            .map(remap_envelope)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((id, remapped))
    }

    async fn append(
        &self,
        id: AggregateId,
        expected_sequence: NonZeroU64,
        events: Vec<Self::Event>,
        context: CorrelationContext,
    ) -> Result<Vec<EventEnvelope<Self::Event>>, StoreError> {
        let dtos = events
            .iter()
            .map(to_dto)
            .collect::<Result<Vec<_>, _>>()
            .map_err(conversion_error)?;
        let envelopes = self
            .inner
            .append(id, expected_sequence, dtos, context)
            .await?;
        envelopes.iter().map(remap_envelope).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cherry_pit_core::{CorrelationContext, ScheduleArmed, ScheduleCancelled, ScheduleId};

    fn armed_event(tag: u8) -> cherry_pit_core::SchedulerEvent {
        cherry_pit_core::SchedulerEvent::Armed(ScheduleArmed::new(
            ScheduleId::from_uuid(uuid::Uuid::from_bytes([tag; 16])),
            jiff::Timestamp::from_second(1_700_000_000 + i64::from(tag)).unwrap(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            uuid::Uuid::from_bytes([tag.wrapping_add(1); 16]),
            format!("caller.event.{tag}"),
            vec![tag; 8],
            CorrelationContext::new(
                uuid::Uuid::from_bytes([tag.wrapping_add(2); 16]),
                uuid::Uuid::from_bytes([tag.wrapping_add(3); 16]),
            ),
        ))
    }

    fn cancelled_event(tag: u8) -> cherry_pit_core::SchedulerEvent {
        cherry_pit_core::SchedulerEvent::Cancelled(ScheduleCancelled::new(ScheduleId::from_uuid(
            uuid::Uuid::from_bytes([tag; 16]),
        )))
    }

    fn temp_pgno_path() -> tempfile::TempPath {
        let file = tempfile::NamedTempFile::new().expect("create temp file");
        let path = file.into_temp_path();
        std::fs::remove_file(&path).expect("clear placeholder so create_pgno starts fresh");
        path
    }

    #[test]
    fn scheduler_event_dto_roundtrip_preserves_every_armed_field() {
        let original = armed_event(7);
        let dto = to_dto(&original).expect("armed event converts");
        let back = from_dto(dto).expect("dto converts back");

        let cherry_pit_core::SchedulerEvent::Armed(a) = &original else {
            unreachable!()
        };
        let cherry_pit_core::SchedulerEvent::Armed(b) = &back else {
            panic!("expected Armed variant back")
        };
        assert_eq!(a.schedule_id(), b.schedule_id());
        assert_eq!(a.fire_at(), b.fire_at());
        assert_eq!(a.target_aggregate(), b.target_aggregate());
        assert_eq!(a.caller_event_id(), b.caller_event_id());
        assert_eq!(a.caller_event_type(), b.caller_event_type());
        assert_eq!(a.payload(), b.payload());
        assert_eq!(a.correlation(), b.correlation());
        assert_eq!(original, back);
    }

    #[test]
    fn scheduler_event_dto_roundtrip_preserves_cancelled_field() {
        let original = cancelled_event(3);
        let dto = to_dto(&original).expect("cancelled event converts");
        let back = from_dto(dto).expect("dto converts back");
        assert_eq!(original, back);
    }

    #[test]
    fn caller_event_type_over_bound_is_reported_not_silently_truncated() {
        let oversized = "x".repeat(CALLER_EVENT_TYPE_MAX + 1);
        let event = cherry_pit_core::SchedulerEvent::Armed(ScheduleArmed::new(
            ScheduleId::from_uuid(uuid::Uuid::now_v7()),
            jiff::Timestamp::now(),
            AggregateId::new(NonZeroU64::new(1).unwrap()),
            uuid::Uuid::now_v7(),
            oversized,
            vec![0u8; 4],
            CorrelationContext::none(),
        ));
        assert!(matches!(
            to_dto(&event),
            Err(SchedulerEventConversionError::CallerEventTypeTooLong { .. })
        ));
    }

    #[tokio::test]
    async fn create_then_load_roundtrip() {
        let path = temp_pgno_path();
        let store = PgnoSchedulerStore::create_pgno(&path).expect("create store");

        let (id, created) = store
            .create(vec![armed_event(1)], CorrelationContext::none())
            .await
            .expect("create succeeds");
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].sequence().get(), 1);

        let loaded = store.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].event_id(), created[0].event_id());
        assert_eq!(loaded[0].payload(), created[0].payload());
    }

    #[tokio::test]
    async fn append_single_event_extends_stream() {
        let path = temp_pgno_path();
        let store = PgnoSchedulerStore::create_pgno(&path).expect("create store");
        let (id, created) = store
            .create(vec![armed_event(1)], CorrelationContext::none())
            .await
            .expect("create succeeds");
        let expected_sequence = created[0].sequence();

        let appended = store
            .append(
                id,
                expected_sequence,
                vec![cancelled_event(1)],
                CorrelationContext::none(),
            )
            .await
            .expect("append succeeds");
        assert_eq!(appended.len(), 1);
        assert_eq!(appended[0].sequence().get(), 2);

        let loaded = store.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[1].payload(), appended[0].payload());
    }

    #[tokio::test]
    async fn append_rejects_wrong_expected_sequence() {
        let path = temp_pgno_path();
        let store = PgnoSchedulerStore::create_pgno(&path).expect("create store");
        let (id, _created) = store
            .create(vec![armed_event(1)], CorrelationContext::none())
            .await
            .expect("create succeeds");

        let wrong = NonZeroU64::new(99).unwrap();
        let result = store
            .append(
                id,
                wrong,
                vec![cancelled_event(1)],
                CorrelationContext::none(),
            )
            .await;

        assert!(matches!(
            result,
            Err(StoreError::ConcurrencyConflict {
                expected_sequence,
                actual_sequence: 1,
                ..
            }) if expected_sequence == wrong
        ));
    }

    #[tokio::test]
    async fn restart_recovery_survives_reopen() {
        let path = temp_pgno_path();
        let id;
        {
            let store = PgnoSchedulerStore::create_pgno(&path).expect("create store");
            let (created_id, created) = store
                .create(vec![armed_event(1)], CorrelationContext::none())
                .await
                .expect("create succeeds");
            id = created_id;
            store
                .append(
                    id,
                    created[0].sequence(),
                    vec![cancelled_event(1)],
                    CorrelationContext::none(),
                )
                .await
                .expect("append succeeds");
        }

        let reopened = PgnoSchedulerStore::open_pgno(&path).expect("reopen existing store");
        let loaded = reopened.load(id).await.expect("load succeeds");
        assert_eq!(loaded.len(), 2, "both events survive restart");
        assert_eq!(loaded[0].sequence().get(), 1);
        assert_eq!(loaded[1].sequence().get(), 2);

        let (new_id, _) = reopened
            .create(vec![armed_event(2)], CorrelationContext::none())
            .await
            .expect("create after reopen assigns a fresh id above the seeded max");
        assert!(
            new_id.get() > id.get(),
            "AggregateId counter must be seeded from the max id observed on open"
        );
    }
}
