use std::marker::PhantomData;
use std::num::NonZeroU64;
use std::path::Path;

use cherry_pit_core::{
    AggregateId, CorrelationContext, DomainEvent, EventEnvelope, EventStore, StoreCreateResult,
    StoreError,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::PgnoEventStore;

/// Upper bound on the JSON-serialized event byte length carried
/// through [`SerdeEnvelopeDto`].
const SERDE_ENVELOPE_MAX: usize = 262_144;

/// Bridge-crate-local opaque-bytes envelope used by
/// [`PgnoSerdeStore`] to persist an arbitrary `serde`-capable event
/// type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerdeEnvelopeDto {
    bytes: Vec<u8>,
}

impl DomainEvent for SerdeEnvelopeDto {
    fn event_type(&self) -> &'static str {
        "pgno-serde-bridge.envelope"
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
    if actual > SERDE_ENVELOPE_MAX {
        return Err(SerdeBridgeError::TooLarge { actual });
    }
    Ok(SerdeEnvelopeDto { bytes: json })
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
/// `serde`-capable domain event type.
pub struct PgnoSerdeStore<Ev> {
    inner: PgnoEventStore<SerdeEnvelopeDto>,
    _event: PhantomData<Ev>,
}

impl<Ev> PgnoSerdeStore<Ev>
where
    Ev: DomainEvent + Serialize + DeserializeOwned + Clone + Send + 'static,
{
    /// Create a fresh `.pgno`-backed store, truncating any existing file.
    ///
    /// # Errors
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot create
    /// the backing container.
    pub fn create_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: PgnoEventStore::create_pgno(path)?,
            _event: PhantomData,
        })
    }

    /// Open an existing `.pgno`-backed store.
    ///
    /// # Errors
    /// Returns [`StoreError::Infrastructure`] when pardosa cannot open
    /// or fold the backing container.
    pub fn open_pgno(path: &Path) -> Result<Self, StoreError> {
        Ok(Self {
            inner: PgnoEventStore::open_pgno(path)?,
            _event: PhantomData,
        })
    }
}

impl<Ev: DomainEvent + serde::Serialize + DeserializeOwned + Clone + Send + 'static> EventStore
    for PgnoSerdeStore<Ev>
{
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
