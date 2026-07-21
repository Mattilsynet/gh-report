use super::error::{CheckedReplayKind, Error, RehydrateInvariant, ValidatedReplayError};
use super::validated::stream_validated;
use crate::dragline::Line;
use crate::frontier::Frontier;
use crate::{Event, Fiber, FiberId, FiberState, PardosaError};
use pardosa_file::{Reader, Syncable, Writer};
use pardosa_schema::GenomeSafe;
use pardosa_wire::{Decode, Encode, Validate, from_bytes, precursor_hash_of, to_vec};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
/// Precursor-check enforcement mode consulted by
/// [`rebuild_dragline_with_frontier`] (roadmap `adr-fmt-t7t4v` P2a/P2b,
/// D2b). `ObserveOnly` computes the three precursor checks and emits a
/// non-blocking warn per would-fail; `Enforce` rejects instead.
/// [`precursor_check_mode`] resolves the runtime-selectable mode
/// (mission `adr-fmt-qkq9l` Part A, PGN-0010 P2b amendment); the
/// shipped default (env unset or unrecognised) stays `ObserveOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrecursorCheckMode {
    ObserveOnly,
    Enforce,
}
/// Environment variable that runtime-selects [`PrecursorCheckMode`]
/// per store-open (mission `adr-fmt-qkq9l` Part A). `enforce`
/// (case-insensitive) selects [`PrecursorCheckMode::Enforce`]; `observe`,
/// unset, empty, or any unrecognised value fails safe to
/// [`PrecursorCheckMode::ObserveOnly`] — the shipped default is
/// unchanged.
pub(crate) const PRECURSOR_CHECK_MODE_ENV: &str = "PARDOSA_PRECURSOR_CHECK_MODE";
/// Pure resolution of `raw` (the [`PRECURSOR_CHECK_MODE_ENV`] value, if
/// any) to a [`PrecursorCheckMode`], with no environment access —
/// separated from [`precursor_check_mode`] so the fail-safe-default
/// and unrecognised-value cases are unit-testable without mutating
/// process state.
fn resolve_precursor_check_mode(raw: Option<&str>) -> PrecursorCheckMode {
    match raw {
        Some(value) if value.eq_ignore_ascii_case("enforce") => PrecursorCheckMode::Enforce,
        Some(_) | None => PrecursorCheckMode::ObserveOnly,
    }
}
/// Resolve the runtime-selectable [`PrecursorCheckMode`] by reading
/// [`PRECURSOR_CHECK_MODE_ENV`] at the call site (per store-open, not
/// cached behind a process-global) so tests toggling the variable
/// stay order-independent.
pub(crate) fn precursor_check_mode() -> PrecursorCheckMode {
    resolve_precursor_check_mode(std::env::var(PRECURSOR_CHECK_MODE_ENV).ok().as_deref())
}
/// Precursor bounds/fiber/hash check for the batch rebuild path,
/// delegating to the pure functions shared with the streaming verify
/// chain in [`super::checked`] (adr-fmt-lutpd finding #2 /
/// adr-fmt-ibi23) — this replaces the former standalone
/// `precursor_would_fail` duplicate.
fn rebuild_precursor_check<T>(
    events: &[Event<T>],
    raw_bytes: &[Vec<u8>],
    position: usize,
    position_u64: u64,
) -> Result<(), CheckedReplayKind> {
    let event = &events[position];
    let Some(pidx) = super::checked::precursor_bounds(event, position, position_u64)? else {
        return Ok(());
    };
    let prior_event = &events[pidx];
    let prior_hash = precursor_hash_of(&raw_bytes[pidx]);
    let precursor_index = event
        .precursor()
        .as_index()
        .expect("precursor_bounds confirmed a precursor index")
        .value();
    super::checked::precursor_matches(event, precursor_index, prior_event.fiber_id(), prior_hash)
}
/// Write a `Line`'s full event line to `sink` as a `.pgno`
/// container, optionally embedding `schema_source` in the footer.
///
/// Supplied at the call site (not via `T: HasEventSchemaSource`) so
/// adopters can attach a runtime-known descriptor via
/// `EventLog::with_schema_source`.
///
/// # Errors
/// [`Error::File`] for framing/encoding errors from `pardosa-file`,
/// or [`Error::Io`] propagated from the underlying writer.
pub(crate) fn persist_with_source<T, W>(
    dragline: &Line<T>,
    sink: &mut W,
    schema_source: Option<&'static str>,
) -> Result<(), Error>
where
    T: Encode + GenomeSafe,
    W: Syncable,
{
    if let Err(kind) = dragline.check_persistable() {
        return Err(Error::UnpersistableState { kind });
    }
    let mut writer = Writer::new(sink, Event::<T>::ENVELOPE_HASH);
    if let Some(source) = schema_source {
        writer = writer.with_schema_source(source);
    }
    for event in dragline.read_line() {
        let bytes = to_vec(event);
        writer.write_message(&bytes)?;
    }
    writer.finish()?;
    Ok(())
}
/// Rehydrate a `Line<T>` from a `.pgno` container by streaming the
/// event line through the commit pipeline.
///
/// ADR-0020 reader bound: `T: Decode + GenomeSafe` only — no
/// `Encode`. The chain-frontier is folded over the canonical bytes
/// returned by [`Reader::read_message`] (byte-identical to
/// `to_vec(event)`), so no re-encoding is required.
///
/// # Errors
///
/// [`Error::File`] / [`Error::SchemaHashMismatch`] on container
/// header errors; [`Error::Decode`] on per-event decode failure;
/// [`Error::InvariantViolation`] on structural rebuild failure.
pub(crate) fn rehydrate_unchecked<T, R>(
    source: R,
    mode: PrecursorCheckMode,
) -> Result<Line<T>, Error>
where
    T: Decode + GenomeSafe,
    R: Read + Seek,
{
    let mut reader = Reader::open(source)?;
    let found = reader.schema_hash();
    let expected = Event::<T>::ENVELOPE_HASH;
    if found != expected {
        return Err(Error::SchemaHashMismatch { expected, found });
    }
    let n = reader.index().len();
    let mut events: Vec<Event<T>> = Vec::with_capacity(n);
    let mut raw_bytes: Vec<Vec<u8>> = Vec::with_capacity(n);
    let mut frontier = Frontier::GENESIS;
    for i in 0..n {
        let bytes = reader.read_message(i).map_err(Error::File)?;
        frontier = frontier.roll(&bytes);
        let event: Event<T> = from_bytes(&bytes).map_err(Error::Decode)?;
        events.push(event);
        raw_bytes.push(bytes);
    }
    rebuild_dragline_with_frontier(events, frontier, Some(&raw_bytes), mode).map_err(|e| match e {
        PardosaError::CursorRead { source } => *source,
        other => Error::InvariantViolation(RehydrateInvariant::from(other)),
    })
}
/// Drain a fallible `Event<T>` stream into the supplied pre-sized
/// destination Vec, surfacing the first per-item `Err` and short-
/// circuiting (no further reads). Retained for the validated
/// rehydrate path that drives [`ValidatedEventStream`].
///
/// [`ValidatedEventStream`]: super::ValidatedEventStream
fn stream_fold_line<I, T, E>(stream: I, mut dst: Vec<Event<T>>) -> Result<Vec<Event<T>>, E>
where
    I: IntoIterator<Item = Result<Event<T>, E>>,
{
    for item in stream {
        dst.push(item?);
    }
    Ok(dst)
}
pub(crate) fn rebuild_dragline_with_frontier<T>(
    events: Vec<Event<T>>,
    frontier: Frontier,
    raw_bytes: Option<&[Vec<u8>]>,
    mode: PrecursorCheckMode,
) -> Result<Line<T>, PardosaError> {
    let mut lookup: HashMap<FiberId, (Fiber, FiberState)> = HashMap::new();
    let purged_ids: HashSet<FiberId> = HashSet::new();
    let mut max_fiber_id: Option<FiberId> = None;
    let mut next_event_id: u64 = 0;
    for (i, event) in events.iter().enumerate() {
        let position_u64 = u64::try_from(i).expect("line position fits u64");
        if !super::checked::event_id_matches_position(event.event_id().value(), position_u64) {
            let kind = CheckedReplayKind::EventIdPositionMismatch {
                event_id: event.event_id().value(),
                position: position_u64,
            };
            return Err(if raw_bytes.is_some() {
                // Verify-stage arms (pgno/JetStream, raw_bytes Some):
                // unified onto the same `CheckedReplayKind` surface as
                // the streaming verify chain (adr-fmt-ibi23).
                PardosaError::CursorRead {
                    source: Box::new(Error::CheckedReplay { kind }),
                }
            } else {
                // Validated arm (raw_bytes None): contiguity was
                // already enforced upstream by
                // `stream_validated`/`stream_checked`; this is
                // builder-only defense-in-depth and keeps the
                // pre-existing `IntegrityKind` surface so
                // `RehydrateInvariant::from(PardosaError)` (which has
                // no `CursorRead` arm) is never asked to convert it.
                PardosaError::FiberInvariant(crate::error::FiberInvariantKind::Integrity(
                    crate::error::IntegrityKind::EventIdPositionMismatch {
                        event_id: event.event_id().value(),
                        position: position_u64,
                    },
                ))
            });
        }
        if let Some(raw) = raw_bytes {
            match rebuild_precursor_check(&events, raw, i, position_u64) {
                Ok(()) => {}
                Err(kind) => match mode {
                    PrecursorCheckMode::ObserveOnly => {
                        super::checked::warn_precursor_would_fail(event, &kind, i);
                    }
                    PrecursorCheckMode::Enforce => {
                        return Err(PardosaError::CursorRead {
                            source: Box::new(Error::CheckedReplay { kind }),
                        });
                    }
                },
            }
        }
        let idx = crate::Index::from_decoded(position_u64);
        let did = event.fiber_id();
        max_fiber_id = Some(match max_fiber_id {
            None => did,
            Some(prev) if did.value() > prev.value() => did,
            Some(prev) => prev,
        });
        match lookup.get_mut(&did) {
            None => {
                let fiber = Fiber::new(idx, 1, idx)?;
                let state = if event.detached() {
                    FiberState::Detached
                } else {
                    FiberState::Defined
                };
                lookup.insert(did, (fiber, state));
            }
            Some((fiber, state)) => {
                fiber.advance(idx)?;
                if event.detached() {
                    *state = FiberState::Detached;
                } else {
                    *state = FiberState::Defined;
                }
            }
        }
        next_event_id = event
            .event_id()
            .value()
            .checked_add(1)
            .ok_or(PardosaError::IndexOverflow)?;
    }
    let next_id = match max_fiber_id {
        None => FiberId::from_decoded(0),
        Some(m) => m.checked_next()?,
    };
    Ok(Line::from_parts_no_verify(
        events,
        lookup,
        purged_ids,
        next_id,
        crate::EventId::from_decoded(next_event_id),
        false,
        frontier,
    ))
}
/// Validate-aware variant of [`rehydrate_unchecked`] (o1ix.6, roadmap correctness 6).
///
/// Streams via [`stream_validated`] (which enforces per-envelope
/// shape and payload `Validate` checks per event) and folds the
/// resulting events back into a `Line<T>` via the same internal
/// rebuild step as [`rehydrate_unchecked`].
///
/// # Errors
/// Returns [`ValidatedReplayError`] for any per-event failure or
/// container-header error; folds the rebuild-time invariant failure
/// into [`ValidatedReplayError::Replay`] with
/// [`Error::InvariantViolation`].
pub(crate) fn rehydrate_validated<T, R>(
    source: R,
    mode: PrecursorCheckMode,
) -> Result<Line<T>, ValidatedReplayError<<T as Validate>::Error>>
where
    T: Decode + GenomeSafe + Validate,
    R: Read + Seek,
{
    let mut stream = stream_validated::<R, T>(source, None)?;
    let cap = stream.inner.reader.index().len();
    let events: Vec<Event<T>> = Vec::with_capacity(cap);
    let events = stream_fold_line(&mut stream, events)?;
    let frontier = stream.inner.frontier();
    rebuild_dragline_with_frontier(events, frontier, None, mode)
        .map_err(|e| ValidatedReplayError::Replay(Error::InvariantViolation(e.into())))
}
/// Append-shape sibling of [`persist_with_source`] (roadmap IO-PG-1).
///
/// Byte-identical wire output to [`persist_with_source`] for the
/// same `Line<T>`, but via [`pardosa_file::AppendWriter`] instead
/// of [`pardosa_file::Writer`] — the append sink lets the
/// backend-keyed write path stage bodies incrementally without
/// requiring `Seek` on the substrate.
///
/// Preserves I1 (append → replay byte identity) and I9
/// (`EventStore<T>` arity unchanged) per oracle summary
/// `rescue-pardosa-v0id`.
///
/// # Errors
///
/// [`Error::File`] for framing/encoding errors; [`Error::Io`]
/// propagated from the underlying sink.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn persist_with_source_append<T, W>(
    dragline: &Line<T>,
    sink: &mut W,
    schema_source: Option<&'static str>,
) -> Result<(), Error>
where
    T: Encode + GenomeSafe,
    W: Syncable,
{
    if let Err(kind) = dragline.check_persistable() {
        return Err(Error::UnpersistableState { kind });
    }
    let mut writer = pardosa_file::AppendWriter::new(sink, Event::<T>::ENVELOPE_HASH);
    if let Some(source) = schema_source {
        writer = writer.with_schema_source(source);
    }
    for event in dragline.read_line() {
        let bytes = to_vec(event);
        writer.append_message(&bytes)?;
    }
    writer.finish()?;
    Ok(())
}
#[cfg(test)]
mod precursor_check_mode_tests {
    use super::{PrecursorCheckMode, resolve_precursor_check_mode};

    #[test]
    fn env_unset_resolves_to_observe_only() {
        assert_eq!(
            resolve_precursor_check_mode(None),
            PrecursorCheckMode::ObserveOnly
        );
    }

    #[test]
    fn env_empty_resolves_to_observe_only() {
        assert_eq!(
            resolve_precursor_check_mode(Some("")),
            PrecursorCheckMode::ObserveOnly
        );
    }

    #[test]
    fn env_unrecognised_value_resolves_to_observe_only() {
        assert_eq!(
            resolve_precursor_check_mode(Some("not-a-real-mode")),
            PrecursorCheckMode::ObserveOnly
        );
    }

    #[test]
    fn env_observe_resolves_to_observe_only() {
        assert_eq!(
            resolve_precursor_check_mode(Some("observe")),
            PrecursorCheckMode::ObserveOnly
        );
    }

    #[test]
    fn env_enforce_case_insensitive_resolves_to_enforce() {
        assert_eq!(
            resolve_precursor_check_mode(Some("ENFORCE")),
            PrecursorCheckMode::Enforce
        );
        assert_eq!(
            resolve_precursor_check_mode(Some("enforce")),
            PrecursorCheckMode::Enforce
        );
    }
}
