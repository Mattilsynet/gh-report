//! Typed file-API façade over `pardosa-file`.
//!
//! `pardosa-file` stays schema-agnostic (ADR-0002). Typed
//! concerns — pairing `T` with `Event::<T>::ENVELOPE_HASH` and an
//! optional `EVENT_SCHEMA_SOURCE` — live here.
//!
//! [`HasEventSchemaSource`] lets a payload `T` declare the
//! schema-source string embedded into a `.pgno` schema-source
//! slot; embedding happens at
//! [`persist::persist_with_source`] (called via
//! `EventLog::with_schema_source`). [`TypedReader`] wraps
//! `pardosa_file::Reader` preserving `T`.
//!
//! Container `schema_hash` is compared with
//! `Event::<T>::ENVELOPE_HASH` at open; mismatch yields
//! [`persist::Error::SchemaHashMismatch`].
use crate::Event;
use crate::persist;
use pardosa_file::Reader;
use pardosa_schema::GenomeSafe;
use std::io::{Read, Seek};
use std::marker::PhantomData;
/// Opt-in: declare the human-readable schema source that the file
/// writer should embed in the `.pgno` schema-source slot for a
/// payload type `T`.
///
/// Implementors set [`EVENT_SCHEMA_SOURCE`] to `Some(<source string>)`
/// to embed, or `None` to leave the slot empty (byte-identical to
/// the in-crate `persist::persist_with_source` helper).
///
/// The constant is currently a free-form `&'static str`; the format is
/// deliberately unconstrained pending a future cross-language schema
/// surface. Today it serves human inspection of `.pgno` containers
/// (see [`Reader::schema_source`]).
///
/// [`EVENT_SCHEMA_SOURCE`]: HasEventSchemaSource::EVENT_SCHEMA_SOURCE
pub trait HasEventSchemaSource {
    /// `Some(s)` embeds `s` into the file header's schema-source slot;
    /// `None` leaves the slot empty (no bytes consumed beyond the
    /// fixed 40-byte header).
    const EVENT_SCHEMA_SOURCE: Option<&'static str>;
}
/// Typed wrapper around `pardosa_file::Reader<R>` carrying the static
/// payload type `T` and surfacing file-header metadata (schema hash,
/// schema source, message count).
///
/// `TypedReader::open` performs the same schema-hash check as
/// [`persist::stream`], so a mismatch surfaces up front.
#[derive(Debug)]
pub(crate) struct TypedReader<R: Read + Seek, T> {
    inner: Reader<R>,
    _t: PhantomData<fn() -> T>,
}
impl<R: Read + Seek, T: GenomeSafe> TypedReader<R, T> {
    /// Open `source` and validate that the container's schema hash
    /// equals `Event::<T>::ENVELOPE_HASH`.
    ///
    /// # Errors
    /// * [`persist::Error::File`] for framing errors from
    ///   `pardosa-file::Reader::open`.
    /// * [`persist::Error::SchemaHashMismatch`] when the container's
    ///   recorded schema hash disagrees with `Event::<T>::ENVELOPE_HASH`.
    pub fn open(source: R) -> Result<Self, persist::Error> {
        let inner = Reader::open(source)?;
        let expected = Event::<T>::ENVELOPE_HASH;
        let found = inner.schema_hash();
        if found != expected {
            return Err(persist::Error::SchemaHashMismatch { expected, found });
        }
        Ok(Self {
            inner,
            _t: PhantomData,
        })
    }
    /// Embedded schema source string, if any. Empty schema-source
    /// slots return `None`.
    #[must_use]
    pub fn schema_source(&self) -> Option<&str> {
        self.inner.schema_source()
    }
    /// Composed `Event::<T>::ENVELOPE_HASH` recorded in the header.
    #[must_use]
    pub fn schema_hash(&self) -> u128 {
        self.inner.schema_hash()
    }
    /// Number of message bodies recorded in the container index.
    #[must_use]
    pub fn message_count(&self) -> u64 {
        self.inner.message_count()
    }
    pub(crate) fn inner_mut(&mut self) -> &mut Reader<R> {
        &mut self.inner
    }
}
