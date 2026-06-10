use pardosa::store::HasEventSchemaSource;
use pardosa_schema::{EventBytes, EventString, GenomeSafe, Validate};

pub const MAX_ENVELOPE_BYTES: usize = 1_048_576;
pub const MAX_DOMAIN_KEY_BYTES: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Eq, GenomeSafe)]
pub struct EnvelopePayload {
    pub envelope: EventBytes<MAX_ENVELOPE_BYTES>,
    pub aggregate_id: u64,
    pub domain_key: EventString<MAX_DOMAIN_KEY_BYTES>,
}

impl EnvelopePayload {
    /// Build a validated `EnvelopePayload` from raw envelope bytes,
    /// an aggregate id, and a domain key.
    ///
    /// # Errors
    ///
    /// Returns [`PayloadError::Domain`] wrapping
    /// [`pardosa_schema::DomainError::TooLong`] when `envelope` exceeds
    /// [`MAX_ENVELOPE_BYTES`] or `domain_key` exceeds
    /// [`MAX_DOMAIN_KEY_BYTES`].
    pub fn new(
        envelope: Vec<u8>,
        aggregate_id: u64,
        domain_key: String,
    ) -> Result<Self, PayloadError> {
        Ok(Self {
            envelope: EventBytes::try_from(envelope)?,
            aggregate_id,
            domain_key: EventString::try_from(domain_key)?,
        })
    }

    #[must_use]
    pub fn envelope_bytes(&self) -> &[u8] {
        self.envelope.as_slice()
    }

    #[must_use]
    pub fn domain_key(&self) -> &str {
        self.domain_key.as_str()
    }
}

impl HasEventSchemaSource for EnvelopePayload {
    const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some("cherry-pit-pardosa-envelope-payload-v1");
}

impl Validate for EnvelopePayload {
    type Error = PayloadError;

    fn validate(&self) -> Result<(), Self::Error> {
        self.envelope.validate()?;
        self.domain_key.validate()?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PayloadError {
    #[error(transparent)]
    Domain(#[from] pardosa_schema::DomainError),
}
