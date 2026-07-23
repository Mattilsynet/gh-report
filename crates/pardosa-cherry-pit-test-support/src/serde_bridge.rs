use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::Path;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreCreateResult,
    StoreError,
};
use pardosa_schema::{EventBytes, GenomeSafe};
use serde::de::DeserializeOwned;

use crate::PgnoEventStore;

/// Upper bound on the JSON-serialized event byte length carried
/// through [`SerdeEnvelopeDto`]. Generous enough for any test-fixture
/// event this bridge exercises; not a production transport limit.
const SERDE_ENVELOPE_MAX: usize = 262_144;

type SerdeBytesDto = EventBytes<SERDE_ENVELOPE_MAX>;

/// Bridge-crate-local, `GenomeSafe` opaque-bytes envelope used by
/// [`PgnoSerdeStore`] to persist an arbitrary `serde`-capable event
/// type without that type itself deriving `GenomeSafe`.
///
/// Mirrors the `payload`-as-`EventBytes` convention already used by
/// [`crate::SchedulerEventDto`], generalized to the whole event rather
/// than one field: the wrapped bytes are the JSON encoding of the
/// caller's `Ev`, produced and consumed only inside this module.
#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct SerdeEnvelopeDto {
    bytes: SerdeBytesDto,
}

impl DomainEvent for SerdeEnvelopeDto {
    fn event_type(&self) -> &'static str {
        "pgno-serde-bridge.envelope"
    }
}

impl serde::Serialize for SerdeEnvelopeDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut bytes = Vec::new();
        pardosa_schema::Encode::encode(self, &mut bytes);
        serializer.serialize_bytes(&bytes)
    }
}

impl<'de> serde::Deserialize<'de> for SerdeEnvelopeDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes = <Vec<u8>>::deserialize(deserializer)?;
        pardosa_schema::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

/// Error converting between an arbitrary event type and its
/// [`SerdeEnvelopeDto`] wrapper.
#[derive(Debug)]
pub enum SerdeBridgeError {
    /// JSON encoding of the event exceeded [`SERDE_ENVELOPE_MAX`] bytes.
    TooLarge { actual: usize },
    /// JSON encode/decode of the wrapped event failed.
    Codec(serde_json::Error),
}

impl std::fmt::Display for SerdeBridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { actual } => write!(
                f,
                "serde-bridge event is {actual} bytes, exceeds bound {SERDE_ENVELOPE_MAX}"
            ),
            Self::Codec(e) => write!(f, "serde-bridge codec error: {e}"),
        }
    }
}

impl std::error::Error for SerdeBridgeError {}

fn to_dto<Ev: serde::Serialize>(event: &Ev) -> Result<SerdeEnvelopeDto, SerdeBridgeError> {
    let json = serde_json::to_vec(event).map_err(SerdeBridgeError::Codec)?;
    let actual = json.len();
    let bytes = SerdeBytesDto::try_from(json).map_err(|_| SerdeBridgeError::TooLarge { actual })?;
    Ok(SerdeEnvelopeDto { bytes })
}

fn from_dto<Ev: DomainEvent + DeserializeOwned>(
    dto: &SerdeEnvelopeDto,
) -> Result<Ev, SerdeBridgeError> {
    serde_json::from_slice(&dto.bytes).map_err(SerdeBridgeError::Codec)
}

fn conversion_error(error: SerdeBridgeError) -> StoreError {
    StoreError::Infrastructure(Box::new(error))
}

fn remap_envelope<Ev: DomainEvent + DeserializeOwned>(
    envelope: &EventEnvelope<SerdeEnvelopeDto>,
) -> Result<EventEnvelope<Ev>, StoreError> {
    let payload = from_dto(envelope.payload()).map_err(conversion_error)?;
    EventEnvelope::new(
        envelope.event_id(),
        envelope.aggregate_id(),
        envelope.sequence(),
        envelope.timestamp(),
        envelope.correlation_id(),
        envelope.causation_id(),
        payload,
    )
    .map_err(|e| StoreError::CorruptData(Box::new(e)))
}

/// `.pgno`-backed [`EventStore`]`<Event = Ev>` for a caller-supplied
/// `serde`-capable domain event type that cannot itself derive
/// `GenomeSafe` (e.g. a test-local fixture defined outside this
/// crate's dependency ring).
///
/// # Design: JSON-bytes bridge over `PgnoEventStore`
///
/// Delegates to `PgnoEventStore<SerdeEnvelopeDto>`, converting `Ev` to
/// and from an opaque JSON-encoded byte payload at the boundary. This
/// is deliberately more general than [`crate::PgnoSchedulerStore`]
/// (which maps a single fixed type field-for-field): callers here
/// supply an arbitrary `Ev: DomainEvent + Serialize + DeserializeOwned`
/// with no per-type bridge code required, at the cost of losing
/// field-level schema evolution guarantees `GenomeSafe` would give a
/// purpose-built DTO. Appropriate for test fixtures; not a
/// recommendation for production event types.
pub struct PgnoSerdeStore<Ev> {
    inner: PgnoEventStore<SerdeEnvelopeDto>,
    _event: PhantomData<Ev>,
}

impl<Ev> PgnoSerdeStore<Ev> {
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot
    /// create the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: PgnoEventStore::create_pgno(path)?,
            _event: PhantomData,
        })
    }

    /// Open an existing `.pgno`-backed store, rehydrating its fibers
    /// and seeding the `AggregateId` counter from the max id observed.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open
    /// or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: PgnoEventStore::open_pgno(path)?,
            _event: PhantomData,
        })
    }
}

impl<Ev: DomainEvent + serde::Serialize + DeserializeOwned> EventStore for PgnoSerdeStore<Ev> {
    type Event = Ev;

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
