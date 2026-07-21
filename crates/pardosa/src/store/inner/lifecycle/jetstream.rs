use super::{Decode, GenomeSafe, PardosaError};
use crate::backend::jetstream::JetStreamDurableFrame;
use crate::backend::rehydrate::from_pgno_bytes_unchecked;
use crate::event::Event;
use crate::frontier::Frontier;
use pardosa_wire::from_bytes;

fn io_error_to_cursor_read(e: std::io::Error) -> PardosaError {
    super::persist_error_to_cursor_read(crate::persist::Error::Io(e))
}
pub(super) fn backend_error_to_cursor_read(
    context: &'static str,
    e: crate::error::BackendError,
) -> PardosaError {
    match e {
        crate::error::BackendError::ConcurrencyConflict {
            expected_seq,
            actual_seq,
            source,
        } => PardosaError::ConcurrencyConflict {
            expected_seq,
            actual_seq,
            source,
        },
        other => io_error_to_cursor_read(std::io::Error::other(format!("{context}: {other}"))),
    }
}
fn fetch_jetstream_frames(
    adapter: &mut crate::authoritative::jetstream::JetStreamBackendAdapter,
) -> Result<Vec<JetStreamDurableFrame>, PardosaError> {
    adapter
        .fetch_durable_frames()
        .map_err(|e| backend_error_to_cursor_read("JetStream rehydrate fetch failed", e))
}

pub(super) fn fetch_gated_jetstream_frames<T>(
    adapter: &mut crate::authoritative::jetstream::JetStreamBackendAdapter,
    expected_marker: &str,
) -> Result<Vec<JetStreamDurableFrame>, PardosaError>
where
    T: GenomeSafe,
{
    adapter
        .set_schema_tag(expected_marker.to_owned())
        .map_err(io_error_to_cursor_read)?;
    let frames = fetch_jetstream_frames(adapter)?;
    let marker = adapter
        .read_stream_description()
        .map_err(|e| backend_error_to_cursor_read("JetStream stream marker read failed", e))?;
    gate_stream_marker::<T>(marker.as_deref(), frames.is_empty())?;
    Ok(frames)
}

pub(super) fn rehydrate_jetstream_frames<T>(
    frames: &[JetStreamDurableFrame],
    mode: crate::persist::PrecursorCheckMode,
) -> Result<(crate::dragline::Line<T>, usize), PardosaError>
where
    T: Decode + GenomeSafe,
{
    if frames.is_empty() {
        return Ok((crate::dragline::Line::new(), 0));
    }
    if let Some((pgno_idx, event_frames)) = event_frames_from_latest_pgno::<T>(frames)? {
        if pgno_idx + 1 == frames.len() {
            let line = from_pgno_bytes_unchecked::<T>(&frames[pgno_idx].payload, mode)
                .map_err(super::persist_error_to_cursor_read)?;
            let synced_events = line.read_line().len();
            return Ok((line, synced_events));
        }
        let mut replay_frames: Vec<JetStreamDurableFrame> = event_frames
            .into_iter()
            .map(legacy_jetstream_frame)
            .collect();
        replay_frames.extend(frames[pgno_idx + 1..].iter().cloned());
        let line = rehydrate_event_frames::<T>(&replay_frames, mode)?;
        let synced_events = line.read_line().len();
        return Ok((line, synced_events));
    }
    let line = rehydrate_event_frames::<T>(frames, mode)?;
    let synced_events = line.read_line().len();
    Ok((line, synced_events))
}

type PgnoFrameMatch = Option<(usize, Vec<Vec<u8>>)>;

fn event_frames_from_latest_pgno<T>(
    frames: &[JetStreamDurableFrame],
) -> Result<PgnoFrameMatch, PardosaError>
where
    T: Decode + GenomeSafe,
{
    for (idx, frame) in frames.iter().enumerate().rev() {
        match event_frames_from_pgno::<T>(&frame.payload) {
            Ok(frames) => return Ok(Some((idx, frames))),
            Err(err) if is_schema_hash_mismatch(&err) => return Err(err),
            Err(_) => {}
        }
    }
    Ok(None)
}

fn is_schema_hash_mismatch(err: &PardosaError) -> bool {
    matches!(
        err,
        PardosaError::CursorRead { source }
            if matches!(source.as_ref(), crate::persist::Error::SchemaHashMismatch { .. })
    )
}

pub(super) fn legacy_jetstream_frame(payload: Vec<u8>) -> JetStreamDurableFrame {
    JetStreamDurableFrame {
        payload,
        schema_tag: None,
    }
}

pub(super) fn schema_tag<T>() -> String
where
    T: GenomeSafe,
{
    format!("{:032x}", Event::<T>::ENVELOPE_HASH)
}

fn mismatch_sentinel(expected: u128) -> u128 {
    u128::from(expected == 0)
}

fn parse_schema_tag(tag: &str) -> Option<u128> {
    let hex = tag
        .strip_prefix("0x")
        .or_else(|| tag.strip_prefix("0X"))
        .unwrap_or(tag);
    u128::from_str_radix(hex, 16).ok()
}

fn schema_marker_mismatch(expected: u128, found: u128) -> PardosaError {
    super::persist_error_to_cursor_read(crate::persist::Error::SchemaHashMismatch {
        expected,
        found,
    })
}

pub(super) fn gate_stream_marker<T>(
    marker: Option<&str>,
    stream_is_empty: bool,
) -> Result<(), PardosaError>
where
    T: GenomeSafe,
{
    let expected = Event::<T>::ENVELOPE_HASH;
    let Some(marker) = marker else {
        return if stream_is_empty {
            Ok(())
        } else {
            let expected_marker = format!("{expected:032x}");
            tracing::warn!(
                event = "schema_marker_absent_populated_stream",
                expected_schema_marker = %expected_marker,
                recovery = "stream may have been recreated out-of-band; bounce the revision to re-provision it; see crates/gh-report/OPERATIONS.md",
                "schema marker absent on populated JetStream stream"
            );
            Err(super::persist_error_to_cursor_read(
                crate::persist::Error::SchemaMarkerAbsent { expected },
            ))
        };
    };
    let found = parse_schema_tag(marker).unwrap_or_else(|| mismatch_sentinel(expected));
    if found == expected {
        return Ok(());
    }
    Err(schema_marker_mismatch(expected, found))
}

pub(super) fn gate_replay_schema_tag<T>(tag: Option<&str>) -> Result<(), PardosaError>
where
    T: GenomeSafe,
{
    let Some(tag) = tag else {
        return Ok(());
    };
    let expected = Event::<T>::ENVELOPE_HASH;
    let found = parse_schema_tag(tag).unwrap_or_else(|| mismatch_sentinel(expected));
    if found == expected {
        return Ok(());
    }
    Err(schema_marker_mismatch(expected, found))
}
fn event_frames_from_pgno<T>(bytes: &[u8]) -> Result<Vec<Vec<u8>>, PardosaError>
where
    T: Decode + GenomeSafe,
{
    let mut reader = pardosa_file::Reader::open(std::io::Cursor::new(bytes))
        .map_err(crate::persist::Error::File)
        .map_err(super::persist_error_to_cursor_read)?;
    let found = reader.schema_hash();
    let expected = Event::<T>::ENVELOPE_HASH;
    if found != expected {
        return Err(super::persist_error_to_cursor_read(
            crate::persist::Error::SchemaHashMismatch { expected, found },
        ));
    }
    let n = reader.index().len();
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(n);
    for i in 0..n {
        frames.push(
            reader
                .read_message(i)
                .map_err(crate::persist::Error::File)
                .map_err(super::persist_error_to_cursor_read)?,
        );
    }
    Ok(frames)
}
pub(super) fn rehydrate_event_frames<T>(
    frames: &[JetStreamDurableFrame],
    mode: crate::persist::PrecursorCheckMode,
) -> Result<crate::dragline::Line<T>, PardosaError>
where
    T: Decode + GenomeSafe,
{
    let mut events: Vec<Event<T>> = Vec::new();
    let mut raw_bytes: Vec<Vec<u8>> = Vec::new();
    let mut frontier = Frontier::GENESIS;
    for frame in frames {
        gate_replay_schema_tag::<T>(frame.schema_tag.as_deref())?;
        let bytes = frame.as_ref();
        frontier = frontier.roll(bytes);
        let event: Event<T> = from_bytes(bytes)
            .map_err(crate::persist::Error::Decode)
            .map_err(super::persist_error_to_cursor_read)?;
        events.push(event);
        raw_bytes.push(bytes.to_vec());
    }
    crate::persist::rebuild_dragline_with_frontier(events, frontier, Some(&raw_bytes), mode)
}
