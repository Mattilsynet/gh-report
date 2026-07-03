use super::{
    Decode, EventStore, FrontierPublisher, GenomeSafe, PardosaError, Path, PathBuf, Validate,
    ValidatedReplayError,
};
use crate::authoritative::{AuthoritativeBackend, BackendDispatch, admit_into_dispatch};
use crate::backend::jetstream::JetStreamDurableFrame;
use crate::backend::rehydrate::from_pgno_bytes_unchecked;
use crate::dragline::Dragline;
use crate::event::Event;
use crate::frontier::Frontier;
use pardosa_file::manifest::{
    ManifestRecord, RecoveredPrefix, RecoveryError, RecoveryOutcome, RecoveryReaderErrorKind,
    finalize_recovered_prefix, recover_footerless_prefix,
};
use pardosa_file::{FileError, Reader, Syncable};
use pardosa_wire::from_bytes;
use std::io::{Cursor, Seek};
fn pgno_manifest_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(".pgix");
    PathBuf::from(os)
}
fn clean_recovered_prefix(path: &Path) -> Result<(RecoveredPrefix, bool), crate::persist::Error> {
    let manifest_path = pgno_manifest_path(path);
    if manifest_path.exists()
        && let Ok(pgno_bytes) = std::fs::read(path)
        && let Ok(manifest_bytes) = std::fs::read(&manifest_path)
        && let Ok(recovered) = recover_footerless_prefix(&pgno_bytes, &manifest_bytes)
    {
        return Ok((recovered, true));
    }
    let file = std::fs::File::open(path).map_err(crate::persist::Error::Io)?;
    let reader = Reader::open(file).map_err(crate::persist::Error::File)?;
    let records: Vec<ManifestRecord> = reader
        .index()
        .iter()
        .map(|entry| ManifestRecord {
            offset: entry.offset(),
            size: entry.size(),
            checksum: entry.checksum(),
        })
        .collect();
    let data_end = records.iter().try_fold(
        pardosa_file::format::messages_offset(reader.schema_size()) as u64,
        |_, record| {
            record
                .offset
                .checked_add(u64::from(record.size))
                .ok_or(FileError::IndexOverflow)
        },
    )?;
    Ok((
        RecoveredPrefix {
            schema_hash: reader.schema_hash(),
            page_class: reader.page_class(),
            schema_size: reader.schema_size(),
            schema_source: reader.schema_source().map(str::to_owned),
            records,
            data_end,
            frontier: None,
        },
        false,
    ))
}
fn recover_footerless_pgno_at_path(
    path: &Path,
) -> Result<(RecoveredPrefix, u64), crate::persist::Error> {
    let manifest_path = pgno_manifest_path(path);
    if !manifest_path.exists() {
        return Err(crate::persist::Error::File(FileError::InvalidIndex));
    }
    let pgno_bytes = std::fs::read(path).map_err(crate::persist::Error::Io)?;
    let original_file_len = pgno_bytes.len() as u64;
    let manifest_bytes = std::fs::read(&manifest_path).map_err(crate::persist::Error::Io)?;
    let recovered = recover_footerless_prefix(&pgno_bytes, &manifest_bytes).map_err(|source| {
        crate::persist::Error::File(FileError::TornWriteRecovery {
            source: Box::new(source),
        })
    })?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(crate::persist::Error::Io)?;
    finalize_recovered_prefix(&recovered, &mut file).map_err(crate::persist::Error::File)?;
    <std::fs::File as Syncable>::sync_data(&mut file).map_err(crate::persist::Error::Io)?;
    Ok((recovered, original_file_len))
}

/// Read-only plan for an offline `.pgno` recovery attempt.
///
/// The plan is produced by the same manifest recovery core used by
/// [`recover_offline_pgno`]. It does not open the `.pgno` for writing
/// and does not finalize or truncate the file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct OfflineRecoveryPlan {
    pub schema_hash: u128,
    pub manifest_message_count: u64,
    pub records_preserved: u64,
    pub data_end: u64,
    pub file_len: u64,
    pub truncated_bytes: u64,
    pub footer_valid: bool,
    pub status: OfflineRecoveryStatus,
}

/// Footer/read status observed before offline recovery is attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OfflineRecoveryStatus {
    Recoverable {
        reader_error: RecoveryReaderErrorKind,
    },
    AlreadySealed,
}

fn recovery_declined(source: RecoveryError) -> PardosaError {
    PardosaError::CursorRead {
        source: Box::new(crate::persist::Error::File(FileError::TornWriteRecovery {
            source: Box::new(source),
        })),
    }
}

fn reader_recovery_status(pgno_bytes: &[u8]) -> Result<OfflineRecoveryStatus, PardosaError> {
    match Reader::open(Cursor::new(pgno_bytes)) {
        Ok(_) => Ok(OfflineRecoveryStatus::AlreadySealed),
        Err(err) => RecoveryReaderErrorKind::from_file_error(&err)
            .map(|reader_error| OfflineRecoveryStatus::Recoverable { reader_error })
            .ok_or_else(|| {
                persist_error_to_cursor_read(crate::persist::Error::File(
                    FileError::TornWriteRecovery {
                        source: Box::new(RecoveryError::Manifest(err)),
                    },
                ))
            }),
    }
}

/// Compute the offline `.pgno` recovery plan without mutating `path`.
///
/// # Errors
///
/// Returns [`PardosaError::CursorRead`] when the companion `.pgix`
/// manifest is absent or when the existing recovery core declines the
/// prefix, including durable-region body checksum mismatches and
/// manifest frontier mismatches.
pub fn plan_offline_pgno_recovery(path: &Path) -> Result<OfflineRecoveryPlan, PardosaError> {
    let manifest_path = pgno_manifest_path(path);
    if !manifest_path.exists() {
        return Err(persist_error_to_cursor_read(crate::persist::Error::File(
            FileError::TornWriteRecovery {
                source: Box::new(RecoveryError::Manifest(FileError::InvalidIndex)),
            },
        )));
    }
    let pgno_bytes = std::fs::read(path).map_err(|source| PardosaError::CursorJournalOpen {
        source: Box::new(source),
    })?;
    let file_len = pgno_bytes.len() as u64;
    let status = reader_recovery_status(&pgno_bytes)?;
    let manifest_bytes = std::fs::read(manifest_path)
        .map_err(|source| persist_error_to_cursor_read(crate::persist::Error::Io(source)))?;
    let recovered =
        recover_footerless_prefix(&pgno_bytes, &manifest_bytes).map_err(recovery_declined)?;
    let records_preserved = u64::try_from(recovered.records.len()).map_err(|_| {
        persist_error_to_cursor_read(crate::persist::Error::File(FileError::InvalidIndex))
    })?;
    Ok(OfflineRecoveryPlan {
        schema_hash: recovered.schema_hash,
        manifest_message_count: records_preserved,
        records_preserved,
        data_end: recovered.data_end,
        file_len,
        truncated_bytes: file_len.saturating_sub(recovered.data_end),
        footer_valid: matches!(status, OfflineRecoveryStatus::AlreadySealed),
        status,
    })
}

/// Destructively truncate and re-seal an offline `.pgno` via the recovery core.
///
/// # Errors
///
/// Returns [`PardosaError::CursorRead`] when `path` is already sealed,
/// when its companion manifest is absent, or when the manifest recovery
/// core declines the prefix. Declines preserve the original `.pgno`
/// bytes and include durable-region checksum failures and frontier
/// mismatches.
pub fn recover_offline_pgno(path: &Path) -> Result<RecoveryOutcome, PardosaError> {
    let plan = plan_offline_pgno_recovery(path)?;
    let OfflineRecoveryStatus::Recoverable { reader_error } = plan.status else {
        return Err(persist_error_to_cursor_read(crate::persist::Error::File(
            FileError::TornWriteRecovery {
                source: Box::new(RecoveryError::Manifest(FileError::InvalidIndex)),
            },
        )));
    };
    let (recovered, original_file_len) =
        recover_footerless_pgno_at_path(path).map_err(persist_error_to_cursor_read)?;
    let recovered_records = u64::try_from(recovered.records.len()).map_err(|_| {
        persist_error_to_cursor_read(crate::persist::Error::File(FileError::InvalidIndex))
    })?;
    Ok(RecoveryOutcome::new(
        reader_error,
        recovered_records,
        original_file_len.saturating_sub(recovered.data_end),
        recovered.data_end,
        plan.manifest_message_count,
    ))
}

fn recovery_outcome(
    open_error: &FileError,
    recovered: &RecoveredPrefix,
    original_file_len: u64,
) -> Result<RecoveryOutcome, crate::persist::Error> {
    let reader_error = RecoveryReaderErrorKind::from_file_error(open_error)
        .ok_or(crate::persist::Error::File(FileError::InvalidIndex))?;
    let recovered_records = u64::try_from(recovered.records.len())
        .map_err(|_| crate::persist::Error::File(FileError::InvalidIndex))?;
    let truncated_bytes = original_file_len.saturating_sub(recovered.data_end);
    Ok(RecoveryOutcome::new(
        reader_error,
        recovered_records,
        truncated_bytes,
        recovered.data_end,
        recovered_records,
    ))
}

fn warn_recovered(path: &Path, outcome: &RecoveryOutcome) {
    tracing::warn!(
        event = "pgno_torn_tail_recovered",
        path = %path.display(),
        reader_error = outcome.reader_error.as_str(),
        recovered_records = outcome.recovered_records,
        truncated_bytes = outcome.truncated_bytes,
        last_durable_offset = outcome.last_durable_offset,
        manifest_message_count = outcome.manifest_message_count,
        "pgno torn-tail recovered"
    );
}

fn warn_declined(path: &Path, open_error: &FileError, cause: &crate::persist::Error) {
    let reader_error = RecoveryReaderErrorKind::from_file_error(open_error)
        .map_or("other", RecoveryReaderErrorKind::as_str);
    tracing::warn!(
        event = "pgno_recovery_declined",
        path = %path.display(),
        reader_error,
        cause = %cause,
        "pgno recovery declined"
    );
}

fn can_attempt_manifest_recovery(open_error: &FileError) -> bool {
    matches!(
        open_error,
        FileError::InvalidMagic
            | FileError::InvalidIndex
            | FileError::InvalidChecksum
            | FileError::InvalidReserved
    )
}
fn attempt_manifest_recovery_after_open_error(
    path: &Path,
    open_error: FileError,
) -> Result<(RecoveredPrefix, RecoveryOutcome), crate::persist::Error> {
    if !can_attempt_manifest_recovery(&open_error) {
        return Err(crate::persist::Error::File(open_error));
    }
    match recover_footerless_pgno_at_path(path) {
        Ok((recovered, original_file_len)) => {
            let outcome = recovery_outcome(&open_error, &recovered, original_file_len)?;
            warn_recovered(path, &outcome);
            Ok((recovered, outcome))
        }
        Err(cause) => {
            warn_declined(path, &open_error, &cause);
            Err(crate::persist::Error::File(FileError::TornWriteRecovery {
                source: Box::new(match cause {
                    crate::persist::Error::File(FileError::TornWriteRecovery { source }) => *source,
                    crate::persist::Error::File(e) => {
                        pardosa_file::manifest::RecoveryError::Manifest(e)
                    }
                    crate::persist::Error::Io(e) => pardosa_file::manifest::RecoveryError::Io(e),
                    _ => pardosa_file::manifest::RecoveryError::Manifest(FileError::InvalidIndex),
                }),
            }))
        }
    }
}

type OpenUnchecked<T> = (
    std::fs::File,
    crate::dragline::Line<T>,
    RecoveredPrefix,
    bool,
    Option<RecoveryOutcome>,
);

type OpenValidated<T> = (
    std::fs::File,
    crate::dragline::Line<T>,
    Option<RecoveryOutcome>,
);

fn open_rw_seek_and_rehydrate_unchecked<T>(path: &Path) -> Result<OpenUnchecked<T>, PardosaError>
where
    T: Decode + GenomeSafe,
{
    use std::io::SeekFrom;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| PardosaError::CursorJournalOpen {
            source: Box::new(e),
        })?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| PardosaError::CursorRead {
            source: Box::new(crate::persist::Error::Io(e)),
        })?;
    let (dragline, recovered_prefix, manifest_already_synced, recovery_outcome) =
        match crate::persist::rehydrate_unchecked::<T, _>(&mut file) {
            Ok(dragline) => {
                let (recovered_prefix, manifest_already_synced) = clean_recovered_prefix(path)
                    .map_err(|e| PardosaError::CursorRead {
                        source: Box::new(e),
                    })?;
                (dragline, recovered_prefix, manifest_already_synced, None)
            }
            Err(crate::persist::Error::File(open_error))
                if can_attempt_manifest_recovery(&open_error) =>
            {
                let (recovered_prefix, recovery_outcome) =
                    attempt_manifest_recovery_after_open_error(path, open_error).map_err(|e| {
                        PardosaError::CursorRead {
                            source: Box::new(e),
                        }
                    })?;
                file.seek(SeekFrom::Start(0))
                    .map_err(|e| PardosaError::CursorRead {
                        source: Box::new(crate::persist::Error::Io(e)),
                    })?;
                let dragline =
                    crate::persist::rehydrate_unchecked::<T, _>(&mut file).map_err(|e| {
                        PardosaError::CursorRead {
                            source: Box::new(e),
                        }
                    })?;
                (dragline, recovered_prefix, true, Some(recovery_outcome))
            }
            Err(e) => {
                return Err(PardosaError::CursorRead {
                    source: Box::new(e),
                });
            }
        };
    file.seek(SeekFrom::Start(0))
        .map_err(|e| PardosaError::CursorRead {
            source: Box::new(crate::persist::Error::Io(e)),
        })?;
    Ok((
        file,
        dragline,
        recovered_prefix,
        manifest_already_synced,
        recovery_outcome,
    ))
}
fn open_rw_seek_and_rehydrate_validated<T>(
    path: &Path,
) -> Result<OpenValidated<T>, ValidatedReplayError<<T as Validate>::Error>>
where
    T: Decode + GenomeSafe + Validate,
{
    use std::io::SeekFrom;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    let (dragline, last_recovery) = match crate::persist::rehydrate_validated::<T, _>(&mut file) {
        Ok(dragline) => (dragline, None),
        Err(ValidatedReplayError::Replay(crate::persist::Error::File(open_error)))
            if can_attempt_manifest_recovery(&open_error) =>
        {
            let (_, recovery_outcome) =
                attempt_manifest_recovery_after_open_error(path, open_error)
                    .map_err(ValidatedReplayError::Replay)?;
            file.seek(SeekFrom::Start(0))
                .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
            (
                crate::persist::rehydrate_validated::<T, _>(&mut file)?,
                Some(recovery_outcome),
            )
        }
        Err(e) => return Err(e),
    };
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    Ok((file, dragline, last_recovery))
}
fn persist_error_to_cursor_read(e: crate::persist::Error) -> PardosaError {
    PardosaError::CursorRead {
        source: Box::new(e),
    }
}
fn io_error_to_cursor_read(e: std::io::Error) -> PardosaError {
    persist_error_to_cursor_read(crate::persist::Error::Io(e))
}
fn backend_error_to_cursor_read(
    context: &'static str,
    e: crate::error::BackendError,
) -> PardosaError {
    match e {
        crate::error::BackendError::ConcurrencyConflict { source } => {
            PardosaError::ConcurrencyConflict { source }
        }
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

fn fetch_gated_jetstream_frames<T>(
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

fn rehydrate_jetstream_frames<T>(
    frames: &[JetStreamDurableFrame],
) -> Result<(crate::dragline::Line<T>, usize), PardosaError>
where
    T: Decode + GenomeSafe,
{
    if frames.is_empty() {
        return Ok((crate::dragline::Line::new(), 0));
    }
    if let Some((pgno_idx, event_frames)) = event_frames_from_latest_pgno::<T>(frames)? {
        if pgno_idx + 1 == frames.len() {
            let line = from_pgno_bytes_unchecked::<T>(&frames[pgno_idx].payload)
                .map_err(persist_error_to_cursor_read)?;
            let synced_events = line.read_line().len();
            return Ok((line, synced_events));
        }
        let mut replay_frames: Vec<JetStreamDurableFrame> = event_frames
            .into_iter()
            .map(legacy_jetstream_frame)
            .collect();
        replay_frames.extend(frames[pgno_idx + 1..].iter().cloned());
        let line = rehydrate_event_frames::<T>(&replay_frames)?;
        let synced_events = line.read_line().len();
        return Ok((line, synced_events));
    }
    let line = rehydrate_event_frames::<T>(frames)?;
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

fn legacy_jetstream_frame(payload: Vec<u8>) -> JetStreamDurableFrame {
    JetStreamDurableFrame {
        payload,
        schema_tag: None,
    }
}

fn schema_tag<T>() -> String
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
    persist_error_to_cursor_read(crate::persist::Error::SchemaHashMismatch { expected, found })
}

fn gate_stream_marker<T>(marker: Option<&str>, stream_is_empty: bool) -> Result<(), PardosaError>
where
    T: GenomeSafe,
{
    let expected = Event::<T>::ENVELOPE_HASH;
    let Some(marker) = marker else {
        return if stream_is_empty {
            Ok(())
        } else {
            Err(persist_error_to_cursor_read(
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

fn gate_replay_schema_tag<T>(tag: Option<&str>) -> Result<(), PardosaError>
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
    let mut reader = Reader::open(std::io::Cursor::new(bytes))
        .map_err(crate::persist::Error::File)
        .map_err(persist_error_to_cursor_read)?;
    let found = reader.schema_hash();
    let expected = Event::<T>::ENVELOPE_HASH;
    if found != expected {
        return Err(persist_error_to_cursor_read(
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
                .map_err(persist_error_to_cursor_read)?,
        );
    }
    Ok(frames)
}
fn rehydrate_event_frames<T>(
    frames: &[JetStreamDurableFrame],
) -> Result<crate::dragline::Line<T>, PardosaError>
where
    T: Decode + GenomeSafe,
{
    use std::collections::{HashMap, HashSet};
    let mut events: Vec<Event<T>> = Vec::new();
    let mut frontier = Frontier::GENESIS;
    for frame in frames {
        gate_replay_schema_tag::<T>(frame.schema_tag.as_deref())?;
        let bytes = frame.as_ref();
        frontier = frontier.roll(bytes);
        let event: Event<T> = from_bytes(bytes)
            .map_err(crate::persist::Error::Decode)
            .map_err(persist_error_to_cursor_read)?;
        events.push(event);
    }
    let mut lookup: HashMap<crate::FiberId, (crate::Fiber, crate::FiberState)> = HashMap::new();
    let purged_ids: HashSet<crate::FiberId> = HashSet::new();
    let mut max_fiber_id: Option<crate::FiberId> = None;
    let mut next_event_id: u64 = 0;
    for (i, event) in events.iter().enumerate() {
        let position_u64 = u64::try_from(i).expect("line position fits u64");
        if event.event_id().value() != position_u64 {
            return Err(PardosaError::FiberInvariant(
                crate::error::FiberInvariantKind::Integrity(
                    crate::error::IntegrityKind::EventIdPositionMismatch {
                        event_id: event.event_id().value(),
                        position: position_u64,
                    },
                ),
            ));
        }
        let idx = crate::Index::from_decoded(position_u64);
        let fiber_id = event.fiber_id();
        max_fiber_id = Some(match max_fiber_id {
            None => fiber_id,
            Some(prev) if fiber_id.value() > prev.value() => fiber_id,
            Some(prev) => prev,
        });
        match lookup.get_mut(&fiber_id) {
            None => {
                let fiber = crate::Fiber::new(idx, 1, idx)?;
                let state = if event.detached() {
                    crate::FiberState::Detached
                } else {
                    crate::FiberState::Defined
                };
                lookup.insert(fiber_id, (fiber, state));
            }
            Some((fiber, state)) => {
                fiber.advance(idx)?;
                if event.detached() {
                    *state = crate::FiberState::Detached;
                } else {
                    *state = crate::FiberState::Defined;
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
        None => crate::FiberId::from_decoded(0),
        Some(m) => m.checked_next()?,
    };
    Ok(crate::dragline::Line::from_parts_no_verify(
        events,
        lookup,
        purged_ids,
        next_id,
        crate::EventId::from_decoded(next_event_id),
        false,
        frontier,
    ))
}
impl<T> EventStore<T, std::fs::File>
where
    T: super::Encode + Decode + GenomeSafe + crate::typed::HasEventSchemaSource,
{
    /// Construct a fresh `EventStore<T>` over a freshly-created
    /// `.pgno` file at `path`. Overwrites any existing file.
    ///
    /// When `T` declares
    /// [`crate::typed::HasEventSchemaSource::EVENT_SCHEMA_SOURCE`] as
    /// `Some(source)`, that string is embedded in the container
    /// on the first [`super::StoreWriter::sync`].
    ///
    /// # Durability
    ///
    /// The parent directory is `sync_data`-fenced via
    /// [`pardosa_file::fsync_parent_dir`] so the new entry is
    /// durable per the host's POSIX contract (ADR-0010 §D3).
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorJournalOpen`] on file create failure
    /// or on parent-directory `sync_data` failure.
    pub fn create(path: &Path) -> Result<Self, PardosaError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .map_err(|e| PardosaError::CursorJournalOpen {
                source: Box::new(e),
            })?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        pardosa_file::fsync_parent_dir(parent).map_err(|e| PardosaError::CursorJournalOpen {
            source: Box::new(e),
        })?;
        let inner = Dragline::new(file);
        let schema_source = <T as crate::typed::HasEventSchemaSource>::EVENT_SCHEMA_SOURCE;
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source,
            last_recovery: None,
        })
    }
    /// Construct a fresh typed-backend `EventStore<T>` from an
    /// admitted authoritative backend.
    ///
    /// Mirrors [`EventStore::create`] at the typed-backend seam:
    /// path-backed backends delegate to the `.pgno` create path;
    /// `JetStream` backends author the canonical empty `.pgno`
    /// container inside pardosa and seed it only when replay shows the
    /// stream is empty. Populated `JetStream` streams are rehydrated
    /// without writing, so repeated create attempts cannot clobber
    /// existing data.
    ///
    /// # Errors
    ///
    /// [`PardosaError::CursorJournalOpen`] when scratch or path-backed
    /// file creation fails. [`PardosaError::CursorRead`] when backend
    /// replay, canonical empty-container serialisation, seed append,
    /// seed sync, or `.pgno` rehydrate fails.
    pub fn create_with_backend<B: AuthoritativeBackend>(backend: B) -> Result<Self, PardosaError> {
        match admit_into_dispatch(backend) {
            BackendDispatch::Pgno(p) => Self::create(p.path()),
            BackendDispatch::JetStream(boxed_adapter) => {
                let mut adapter = *boxed_adapter;
                let marker = schema_tag::<T>();
                let frames = fetch_gated_jetstream_frames::<T>(&mut adapter, &marker)?;
                let (dragline, synced_events) = rehydrate_jetstream_frames::<T>(&frames)?;
                let scratch =
                    tempfile::tempfile().map_err(|e| PardosaError::CursorJournalOpen {
                        source: Box::new(e),
                    })?;
                let inner = Dragline::from_backend_for_open_jetstream(
                    dragline,
                    scratch,
                    adapter,
                    synced_events,
                );
                Ok(Self {
                    inner,
                    journal: PathBuf::new(),
                    schema_source: <T as crate::typed::HasEventSchemaSource>::EVENT_SCHEMA_SOURCE,
                    last_recovery: None,
                })
            }
            #[cfg(any(test, feature = "test-support"))]
            BackendDispatch::InMem(_) => Err(PardosaError::CursorJournalOpen {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "InMemoryBackend is reserved for in-crate test fixtures \
                     and is not admissible via EventStore::create_with_backend",
                )),
            }),
        }
    }
}
impl<T> EventStore<T, std::fs::File>
where
    T: Decode + GenomeSafe,
{
    /// Open an existing `.pgno` log at `path` (ADR-0018 §D7).
    ///
    /// Validates the container header (schema-hash mismatch →
    /// [`PardosaError::CursorRead`]) and rehydrates the dragline.
    /// No auto-migration; [`super::super::migrate::migrate_keep`]
    /// is the only public migration path.
    ///
    /// ADR-0020 scope: framing, schema-hash, and contiguity checks
    /// only. Per-event precursor-hash and [`Validate`] payload
    /// checks live on [`EventStore::open_validated`].
    ///
    /// Visibility: `pub(crate)` by default; widened to `pub` under
    /// `feature = "test-support"` so integration tests can compare
    /// against the validated open path.
    ///
    /// # Errors
    ///
    /// [`PardosaError`] from the rehydrate pipeline.
    #[cfg(not(any(test, feature = "test-support")))]
    #[expect(
        dead_code,
        reason = "pub(crate) mirror of the test-support pub variant below; \
                  retained for visibility-symmetry across the cfg split so \
                  the rehydrate pipeline has a single in-crate entry shape"
    )]
    pub(crate) fn open(path: &Path) -> Result<Self, PardosaError> {
        let (file, dragline, _, _, last_recovery) =
            open_rw_seek_and_rehydrate_unchecked::<T>(path)?;
        let inner = Dragline::from_line_for_open(dragline, file);
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
            last_recovery,
        })
    }
    /// Test-support variant of [`EventStore::open`]: same
    /// rehydrate pipeline, broader visibility so integration tests
    /// and adopters under `feature = "test-support"` can exercise
    /// the unchecked open path against the validated one
    /// ([`EventStore::open_validated`]). Mirrors the `pub(crate)`
    /// form bit-for-bit; the cfg split only widens visibility
    /// under the gate.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open(path: &Path) -> Result<Self, PardosaError> {
        let (file, dragline, _, _, last_recovery) =
            open_rw_seek_and_rehydrate_unchecked::<T>(path)?;
        let inner = Dragline::from_line_for_open(dragline, file);
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
            last_recovery,
        })
    }
    /// Open the substrate identified by `backend` (ADR-0022 §D1 /
    /// §D11 / §D12). Canonical typed-backend constructor; accepts
    /// any in-crate sealed [`AuthoritativeBackend`]:
    ///
    /// * [`PgnoBackend`] — delegates to the `.pgno` rehydrate path.
    /// * [`crate::store::JetStreamBackend`] — rehydrates from the
    ///   sync-fenced blob via the §D2 reader-side seam; writer
    ///   `sync` routes through sealed
    ///   [`crate::backend::BackendSink`]. Alias arity preserved.
    ///
    /// ADR-0022 §D12 admits only `open_with_backend` to the
    /// audit allowlist.
    ///
    /// # Errors
    ///
    /// [`PardosaError`] from the rehydrate or scratch-tempfile
    /// path (surfacing as [`PardosaError::CursorRead`] /
    /// [`PardosaError::CursorJournalOpen`]).
    pub fn open_with_backend<B: AuthoritativeBackend>(backend: B) -> Result<Self, PardosaError> {
        match admit_into_dispatch(backend) {
            BackendDispatch::Pgno(p) => {
                let (file, dragline, recovered_prefix, manifest_already_synced, last_recovery) =
                    open_rw_seek_and_rehydrate_unchecked::<T>(p.path())?;
                let inner = Dragline::from_backend_for_open(
                    dragline,
                    file,
                    p.path(),
                    Some(recovered_prefix),
                    manifest_already_synced,
                );
                Ok(Self {
                    inner,
                    journal: p.path().to_path_buf(),
                    schema_source: None,
                    last_recovery,
                })
            }
            BackendDispatch::JetStream(boxed_adapter) => {
                let mut adapter = *boxed_adapter;
                let marker = schema_tag::<T>();
                let frames = fetch_gated_jetstream_frames::<T>(&mut adapter, &marker)?;
                let (dragline, synced_events) = rehydrate_jetstream_frames::<T>(&frames)?;
                let scratch =
                    tempfile::tempfile().map_err(|e| PardosaError::CursorJournalOpen {
                        source: Box::new(e),
                    })?;
                let inner = Dragline::from_backend_for_open_jetstream(
                    dragline,
                    scratch,
                    adapter,
                    synced_events,
                );
                Ok(Self {
                    inner,
                    journal: PathBuf::new(),
                    schema_source: None,
                    last_recovery: None,
                })
            }
            #[cfg(any(test, feature = "test-support"))]
            BackendDispatch::InMem(_) => Err(PardosaError::CursorJournalOpen {
                source: Box::new(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "InMemoryBackend is reserved for in-crate test fixtures \
                     and is not admissible via EventStore::open_with_backend",
                )),
            }),
        }
    }
    /// Open an existing `.pgno` log at `path` and attach a durable
    /// [`FrontierPublisher`] (ADR-0018 §12 bullet 3;
    /// ADR-0016 §§D5–D8).
    ///
    /// Pairs the rehydrated dragline with `publisher` plus a
    /// publish-watermark sidecar at `publish_sidecar` (fsynced
    /// after each successful anchor dispatch). On reopen,
    /// unpublished anchors are reconstructed from the persisted
    /// line (ADR-0016 §D6).
    ///
    /// `stream_name` interpolates into
    /// `pardosa.{stream_name}.frontier` (ADR-0015 §D3).
    /// `anchor_interval` is per-tick event count (`0` → `1`).
    ///
    /// # Errors
    ///
    /// [`PardosaError`] from rehydrate, [`PardosaError::PublishWatermark`]
    /// from sidecar read, or [`PardosaError::CursorJournalOpen`]
    /// from the file open.
    pub fn open_with_publisher(
        path: &Path,
        publish_sidecar: PathBuf,
        stream_name: String,
        anchor_interval: u64,
        publisher: Box<dyn FrontierPublisher>,
    ) -> Result<Self, PardosaError> {
        let (file, dragline, _, _, last_recovery) =
            open_rw_seek_and_rehydrate_unchecked::<T>(path)?;
        let inner = Dragline::with_line_and_publisher_path(
            dragline,
            file,
            publish_sidecar,
            stream_name,
            anchor_interval,
            publisher,
        )?;
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
            last_recovery,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pardosa_schema::schema_hash_bytes;
    use pardosa_wire::Validate;
    use pardosa_wire::{Decode, DecodeError, Decoder, Encode, EventSafe};
    use std::io::Write;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TaggedPayload(u64);

    impl pardosa_wire::sealed::Sealed for TaggedPayload {}
    impl EventSafe for TaggedPayload {}
    impl GenomeSafe for TaggedPayload {
        const SCHEMA_HASH: u128 = schema_hash_bytes(b"LifecycleTaggedPayload");
        const SCHEMA_SOURCE: &'static str = "LifecycleTaggedPayload";
    }
    impl Encode for TaggedPayload {
        fn encode(&self, out: &mut Vec<u8>) {
            self.0.encode(out);
        }
    }
    impl Decode for TaggedPayload {
        fn decode(d: &mut Decoder<'_>) -> Result<Self, DecodeError> {
            u64::decode(d).map(Self)
        }
    }
    impl Validate for TaggedPayload {
        type Error = core::convert::Infallible;

        fn validate(&self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    impl crate::typed::HasEventSchemaSource for TaggedPayload {
        const EVENT_SCHEMA_SOURCE: Option<&'static str> = Some(Self::SCHEMA_SOURCE);
    }

    fn frame(value: u64) -> Vec<u8> {
        pardosa_wire::to_vec(&Event::new_unchecked(
            crate::EventId::from_decoded(0),
            crate::FiberId::from_decoded(0),
            false,
            crate::event::Precursor::Genesis,
            [0u8; 32],
            TaggedPayload(value),
        ))
    }

    fn backend_source(msg: &str) -> Box<dyn core::error::Error + Send + Sync + 'static> {
        Box::new(std::io::Error::other(msg))
    }

    fn lifecycle_temp_path(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(format!("pardosa-lifecycle-{name}.pgno"));
        (dir, path)
    }

    fn manifest_path(path: &Path) -> PathBuf {
        let mut os = path.as_os_str().to_os_string();
        os.push(".pgix");
        PathBuf::from(os)
    }

    fn backend_backed_synced_store(path: &Path) -> EventStore<TaggedPayload> {
        {
            let mut store = EventStore::<TaggedPayload>::create(path).expect("create");
            let first = store.writer().begin(TaggedPayload(1)).expect("begin first");
            let _second = store
                .writer()
                .append(first.fiber(), TaggedPayload(2))
                .expect("append second");
            let _lsn = store.writer().sync().expect("sync full pgno");
        }
        let mut store =
            EventStore::<TaggedPayload>::open_with_backend(crate::store::PgnoBackend::open(path))
                .expect("open backend-backed store");
        let _lsn = store.writer().sync().expect("sync manifest");
        store
    }

    fn lifecycle_frontier(path: &Path) -> [u8; 32] {
        let store = EventStore::<TaggedPayload>::open(path).expect("open for frontier");
        *store.inner.frontier().as_bytes()
    }

    fn reopen_payloads(path: &Path) -> Vec<u64> {
        EventStore::<TaggedPayload>::open(path)
            .expect("open")
            .reader()
            .fiber(crate::FiberId::from_decoded(0))
            .iter()
            .expect("history")
            .map(|event| event.domain_event().0)
            .collect()
    }

    fn reopen_validated_payloads(path: &Path) -> Vec<u64> {
        EventStore::<TaggedPayload>::open_validated(path)
            .expect("open_validated")
            .reader()
            .fiber(crate::FiberId::from_decoded(0))
            .iter()
            .expect("history")
            .map(|event| event.domain_event().0)
            .collect()
    }

    fn reopen_seed_plus_resumed_payloads(path: &Path, direct_seed_count: u64) -> Vec<u64> {
        let store = EventStore::<TaggedPayload>::open(path).expect("open");
        let mut payloads: Vec<u64> = store
            .reader()
            .fiber(crate::FiberId::from_decoded(0))
            .iter()
            .expect("seed history")
            .map(|event| event.domain_event().0)
            .collect();
        for fiber_id in 1.. {
            let reader = store.reader();
            let history = reader.fiber(crate::FiberId::from_decoded(fiber_id)).iter();
            let next: Vec<u64> = match history {
                Ok(iter) => iter,
                Err(PardosaError::FiberNotFound(_)) => break,
                Err(err) => panic!("resumed history: {err}"),
            }
            .map(|event| event.domain_event().0)
            .collect();
            if next.is_empty() {
                break;
            }
            payloads.extend(next);
        }
        assert_eq!(
            &payloads[..usize::try_from(direct_seed_count).expect("seed count")],
            &(1..=direct_seed_count).collect::<Vec<_>>()
        );
        payloads
    }

    fn corrupt_footer_magic(path: &Path) {
        let mut bytes = std::fs::read(path).expect("read pgno");
        let magic = bytes
            .len()
            .checked_sub(pardosa_file::format::FILE_FOOTER_SIZE)
            .and_then(|start| start.checked_add(pardosa_file::format::FOOTER_MAGIC_OFFSET))
            .expect("footer magic offset");
        bytes[magic] ^= 0xFF;
        std::fs::write(path, bytes).expect("write pgno");
    }

    fn append_reader_invalid_checksum_tail(path: &Path) {
        let manifest = pardosa_file::manifest::parse_manifest(
            &std::fs::read(manifest_path(path)).expect("manifest bytes"),
        )
        .expect("manifest parses");
        let mut tail = [0u8; pardosa_file::format::FILE_FOOTER_SIZE];
        tail[pardosa_file::format::FOOTER_INDEX_OFFSET
            ..pardosa_file::format::FOOTER_INDEX_OFFSET + 8]
            .copy_from_slice(&manifest.data_end.to_le_bytes());
        tail[pardosa_file::format::FOOTER_MESSAGE_COUNT_OFFSET
            ..pardosa_file::format::FOOTER_MESSAGE_COUNT_OFFSET + 8]
            .copy_from_slice(
                &u64::try_from(manifest.records.len())
                    .expect("manifest count")
                    .to_le_bytes(),
            );
        tail[pardosa_file::format::FOOTER_MAGIC_OFFSET
            ..pardosa_file::format::FOOTER_MAGIC_OFFSET + 4]
            .copy_from_slice(&pardosa_file::format::MAGIC);
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open pgno append")
            .write_all(&tail)
            .expect("append checksum-invalid tail");
    }

    fn corrupt_first_body_byte(path: &Path) {
        let mut bytes = std::fs::read(path).expect("read pgno");
        let schema_size = u32::try_from(TaggedPayload::SCHEMA_SOURCE.len()).expect("schema size");
        let body = pardosa_file::format::messages_offset(schema_size);
        bytes[body] ^= 0xFF;
        std::fs::write(path, bytes).expect("write pgno");
    }

    fn rewrite_manifest_frontier(path: &Path, frontier: [u8; 32]) {
        let manifest_path = manifest_path(path);
        let snapshot = pardosa_file::manifest::parse_manifest(
            &std::fs::read(&manifest_path).expect("read manifest"),
        )
        .expect("parse manifest");
        let mut manifest = Vec::new();
        pardosa_file::manifest::write_complete_manifest(
            &mut manifest,
            snapshot.schema_hash,
            snapshot.page_class,
            snapshot.schema_size,
            &snapshot.records,
            snapshot.data_end,
            frontier,
        )
        .expect("rewrite manifest");
        std::fs::write(manifest_path, manifest).expect("write manifest");
    }

    fn assert_footerless_file_matches_manifest_data_end(path: &Path) {
        let manifest = pardosa_file::manifest::parse_manifest(
            &std::fs::read(manifest_path(path)).expect("manifest bytes"),
        )
        .expect("manifest parses");
        let pgno_len = std::fs::metadata(path).expect("pgno metadata").len();
        assert_eq!(pgno_len, manifest.data_end);
    }

    #[test]
    fn backend_concurrency_conflict_maps_to_typed_pardosa_error() {
        let err = crate::error::BackendError::ConcurrencyConflict {
            source: backend_source("wrong last sequence"),
        };
        match backend_error_to_cursor_read("context should not flatten", err) {
            PardosaError::ConcurrencyConflict { source } => assert!(
                source.to_string().contains("wrong last sequence"),
                "typed conflict source preserved: {source}"
            ),
            other => panic!("expected PardosaError::ConcurrencyConflict, got {other:?}"),
        }
    }

    #[test]
    fn backend_publish_still_flattens_to_cursor_read() {
        let err = crate::error::BackendError::Publish {
            source: backend_source("ordinary publish failure"),
        };
        match backend_error_to_cursor_read("JetStream rehydrate fetch failed", err) {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::Io(io) => {
                    let rendered = io.to_string();
                    assert!(
                        rendered.contains("JetStream rehydrate fetch failed"),
                        "context preserved: {rendered}"
                    );
                    assert!(
                        rendered.contains("ordinary publish failure"),
                        "source display still flattened: {rendered}"
                    );
                }
                other => panic!("expected Io flattening, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn gate_stream_marker_allows_present_matching_marker() {
        let marker = schema_tag::<TaggedPayload>();
        gate_stream_marker::<TaggedPayload>(Some(&marker), false)
            .expect("matching stream marker admits populated stream");
    }

    #[test]
    fn gate_stream_marker_rejects_present_differing_marker() {
        let err =
            gate_stream_marker::<TaggedPayload>(Some("fedcba9876543210fedcba9876543210"), false)
                .expect_err("foreign stream marker must refuse before rehydrate");
        match err {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::SchemaHashMismatch { expected, found } => {
                    assert_eq!(expected, Event::<TaggedPayload>::ENVELOPE_HASH);
                    assert_eq!(found, 0xfedc_ba98_7654_3210_fedc_ba98_7654_3210);
                }
                other => panic!("expected SchemaHashMismatch, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn gate_stream_marker_allows_absent_empty_stream() {
        gate_stream_marker::<TaggedPayload>(None, true)
            .expect("markerless empty stream remains admissible");
    }

    #[test]
    fn gate_stream_marker_rejects_absent_populated_stream() {
        let err = gate_stream_marker::<TaggedPayload>(None, false)
            .expect_err("markerless populated stream must refuse");
        match err {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::SchemaMarkerAbsent { expected } => {
                    assert_eq!(expected, Event::<TaggedPayload>::ENVELOPE_HASH);
                }
                other => panic!("expected SchemaMarkerAbsent, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn footerless_recovery_failure_surfaces_torn_write_recovery() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.pgno");
        {
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .expect("create footerless pgno");
            let mut writer =
                pardosa_file::AppendWriter::new(&mut file, Event::<TaggedPayload>::ENVELOPE_HASH);
            writer.append_message(&frame(1)).expect("append frame");
            writer.sync_data().expect("sync footerless pgno");
        }
        let mut manifest_os = path.as_os_str().to_os_string();
        manifest_os.push(".pgix");
        let manifest_path = std::path::PathBuf::from(manifest_os);
        std::fs::write(&manifest_path, b"not a manifest").expect("write manifest bytes");
        let err = open_rw_seek_and_rehydrate_unchecked::<TaggedPayload>(&path)
            .expect_err("bad footerless recovery must fail");
        match err {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::File(FileError::TornWriteRecovery { .. }) => {}
                other => panic!("expected TornWriteRecovery, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn open_recovers_invalid_checksum_torn_footer_from_manifest() {
        let (_tmp, path) = lifecycle_temp_path("invalid-checksum-tail");
        let _store = backend_backed_synced_store(&path);
        assert!(manifest_path(&path).exists());
        append_reader_invalid_checksum_tail(&path);
        assert_eq!(reopen_payloads(&path), vec![1, 2]);
    }

    #[test]
    fn open_reports_recovery_outcome_and_warn_for_torn_tail() {
        let (_tmp, path) = lifecycle_temp_path("recovery-outcome");
        let _store = backend_backed_synced_store(&path);
        let manifest = pardosa_file::manifest::parse_manifest(
            &std::fs::read(manifest_path(&path)).expect("manifest bytes"),
        )
        .expect("manifest parses");
        append_reader_invalid_checksum_tail(&path);
        let pgno_len = std::fs::metadata(&path).expect("pgno metadata").len();

        let store = EventStore::<TaggedPayload>::open(&path).expect("open recovers");
        let recovery = store.last_recovery().expect("recovery outcome");
        assert_eq!(
            recovery.reader_error,
            crate::store::RecoveryReaderErrorKind::InvalidChecksum,
        );
        assert_eq!(recovery.last_durable_offset, manifest.data_end);
        assert_eq!(
            recovery.manifest_message_count,
            u64::try_from(manifest.records.len()).expect("manifest count fits u64"),
        );
        assert_eq!(recovery.truncated_bytes, pgno_len - manifest.data_end);
        assert!(recovery.truncated_bytes > 0);
    }

    #[test]
    fn backend_resume_strips_prior_direct_footer_before_footerless_append() {
        let (_tmp, path) = lifecycle_temp_path("strip-direct-footer-tail");
        {
            let mut store = EventStore::<TaggedPayload>::create(&path).expect("create");
            let mut fiber = store
                .writer()
                .begin(TaggedPayload(1))
                .expect("begin first")
                .fiber();
            for value in 2..=8u64 {
                fiber = store
                    .writer()
                    .append(fiber, TaggedPayload(value))
                    .expect("append direct seed")
                    .fiber();
            }
            let _lsn = store.writer().sync().expect("direct sync writes footer");
        }
        {
            let mut store = EventStore::<TaggedPayload>::open_with_backend(
                crate::store::PgnoBackend::open(&path),
            )
            .expect("open backend-backed store");
            let _event = store
                .writer()
                .begin(TaggedPayload(9))
                .expect("append resumed");
            let _lsn = store.writer().sync().expect("backend sync");
        }
        assert_footerless_file_matches_manifest_data_end(&path);
        assert_eq!(
            reopen_seed_plus_resumed_payloads(&path, 8),
            (1..=9u64).collect::<Vec<_>>()
        );
    }

    #[test]
    fn backend_resume_sealed_file_property_across_sync_schedules() {
        for direct_seed_count in 1..=4u64 {
            for resumed_sync_count in 1..=4u64 {
                let (_tmp, path) = lifecycle_temp_path(&format!(
                    "sealed-schedule-{direct_seed_count}-{resumed_sync_count}"
                ));
                {
                    let mut store = EventStore::<TaggedPayload>::create(&path).expect("create");
                    let mut fiber = store
                        .writer()
                        .begin(TaggedPayload(1))
                        .expect("begin first")
                        .fiber();
                    for value in 2..=direct_seed_count {
                        fiber = store
                            .writer()
                            .append(fiber, TaggedPayload(value))
                            .expect("append direct seed")
                            .fiber();
                    }
                    let _lsn = store.writer().sync().expect("direct sync writes footer");
                }
                {
                    let mut store = EventStore::<TaggedPayload>::open_with_backend(
                        crate::store::PgnoBackend::open(&path),
                    )
                    .expect("open backend-backed store");
                    for step in 0..resumed_sync_count {
                        let value = direct_seed_count + step + 1;
                        let _event = store.writer().begin(TaggedPayload(value)).expect("append");
                        let _lsn = store.writer().sync().expect("backend sync");
                        assert_footerless_file_matches_manifest_data_end(&path);
                    }
                }
                let expected: Vec<u64> = (1..=direct_seed_count + resumed_sync_count).collect();
                assert_eq!(
                    reopen_seed_plus_resumed_payloads(&path, direct_seed_count),
                    expected
                );
            }
        }
    }

    #[test]
    fn sync_stamps_current_dragline_frontier_in_pgix_manifest() {
        let (_tmp, path) = lifecycle_temp_path("manifest-frontier");
        let _store = backend_backed_synced_store(&path);
        let manifest = pardosa_file::manifest::parse_manifest(
            &std::fs::read(manifest_path(&path)).expect("manifest bytes"),
        )
        .expect("manifest parses");
        assert_eq!(manifest.frontier, Some(lifecycle_frontier(&path)));
    }

    #[test]
    fn open_declines_frontier_mismatch_as_torn_write_recovery() {
        let (_tmp, path) = lifecycle_temp_path("frontier-mismatch");
        let _store = backend_backed_synced_store(&path);
        let mut wrong = lifecycle_frontier(&path);
        wrong[0] ^= 0xFF;
        rewrite_manifest_frontier(&path, wrong);
        corrupt_footer_magic(&path);
        let Err(err) = EventStore::<TaggedPayload>::open(&path) else {
            panic!("frontier mismatch must decline recovery")
        };
        match err {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::File(FileError::TornWriteRecovery { source }) => {
                    assert!(matches!(
                        *source,
                        pardosa_file::manifest::RecoveryError::FrontierMismatch { .. }
                    ));
                }
                other => panic!("expected TornWriteRecovery, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn open_declines_durable_body_corruption_with_original_open_error() {
        let (_tmp, path) = lifecycle_temp_path("durable-body-corrupt");
        let _store = backend_backed_synced_store(&path);
        assert!(manifest_path(&path).exists());
        corrupt_first_body_byte(&path);
        corrupt_footer_magic(&path);
        let Err(err) = EventStore::<TaggedPayload>::open(&path) else {
            panic!("durable-region corruption must not recover")
        };
        match err {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::File(FileError::TornWriteRecovery { .. }) => {}
                other => panic!("expected TornWriteRecovery, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn open_validated_recovers_invalid_checksum_torn_footer_from_manifest() {
        let (_tmp, path) = lifecycle_temp_path("validated-invalid-checksum-tail");
        let _store = backend_backed_synced_store(&path);
        assert!(manifest_path(&path).exists());
        append_reader_invalid_checksum_tail(&path);
        assert_eq!(reopen_validated_payloads(&path), vec![1, 2]);
    }

    #[test]
    fn rehydrate_event_frames_rejects_foreign_replay_tag_with_schema_hash_mismatch() {
        let frames = [JetStreamDurableFrame {
            payload: frame(7),
            schema_tag: Some("fedcba9876543210fedcba9876543210".to_string()),
        }];
        let err = rehydrate_event_frames::<TaggedPayload>(&frames)
            .expect_err("foreign replay tag must reject before decode");
        match err {
            PardosaError::CursorRead { source } => match *source {
                crate::persist::Error::SchemaHashMismatch { expected, found } => {
                    assert_eq!(
                        expected,
                        Event::<TaggedPayload>::ENVELOPE_HASH,
                        "typed mismatch reports this payload's envelope hash"
                    );
                    assert_eq!(
                        found, 0xfedc_ba98_7654_3210_fedc_ba98_7654_3210,
                        "typed mismatch reports the replay tag value"
                    );
                }
                other => panic!("expected SchemaHashMismatch, got {other:?}"),
            },
            other => panic!("expected CursorRead, got {other:?}"),
        }
    }

    #[test]
    fn rehydrate_event_frames_allows_absent_replay_tag() {
        let frames = [JetStreamDurableFrame {
            payload: frame(11),
            schema_tag: None,
        }];
        let line = rehydrate_event_frames::<TaggedPayload>(&frames)
            .expect("legacy frames without replay tags still decode");
        assert_eq!(
            line.read_line()[0].domain_event(),
            &TaggedPayload(11),
            "absent tag falls through to the current decode path"
        );
    }
}
impl<T> EventStore<T, std::fs::File>
where
    T: Decode + GenomeSafe + Validate,
{
    /// Open `path` with full per-event validation
    /// (Fiber-semantics goal 6; ADR-0018 §D7).
    ///
    /// Same invariants as [`EventStore::open`] plus per-event
    /// envelope-shape check and payload
    /// [`Validate::validate`]. Prefer this when foreign-payload
    /// `Decode` impls may produce domain-invalid `T`. No
    /// auto-migration; use [`super::super::migrate::migrate_keep`].
    ///
    /// # Errors
    ///
    /// Returns [`ValidatedReplayError`] for any per-event failure.
    /// File-open I/O surfaces as
    /// [`ValidatedReplayError::Replay`] wrapping
    /// [`crate::persist::Error::Io`].
    pub fn open_validated(
        path: &Path,
    ) -> Result<Self, ValidatedReplayError<<T as Validate>::Error>> {
        let (file, dragline, last_recovery) = open_rw_seek_and_rehydrate_validated::<T>(path)?;
        let inner = Dragline::from_line_for_open(dragline, file);
        Ok(Self {
            inner,
            journal: path.to_path_buf(),
            schema_source: None,
            last_recovery,
        })
    }
}
/// Adopter-facing snapshot of a persisted `.pgno`'s container
/// metadata (ADR-0018 §D7).
///
/// Returned by `EventStore::<T>::metadata`. Carries the values
/// adopters typically want before deciding whether to invoke
/// `EventStore::<T>::open_validated`: event count, the composed
/// `Event::<T>::ENVELOPE_HASH` from the header, and the optional
/// schema source embedded at create time. Owns its strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreMetadata {
    len: u64,
    schema_hash: u128,
    schema_source: Option<String>,
}
impl StoreMetadata {
    /// Number of events persisted in the log.
    #[must_use]
    pub fn len(&self) -> u64 {
        self.len
    }
    /// `true` when the log holds zero events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Composed `Event::<T>::ENVELOPE_HASH` recorded in the
    /// container header (ADR-0005 / ADR-0006).
    #[must_use]
    pub fn schema_hash(&self) -> u128 {
        self.schema_hash
    }
    /// Embedded human-readable schema source, if the writer set
    /// `T::EVENT_SCHEMA_SOURCE` to `Some(_)` at create time.
    #[must_use]
    pub fn schema_source(&self) -> Option<&str> {
        self.schema_source.as_deref()
    }
}
impl<T> EventStore<T, std::fs::File>
where
    T: Decode + GenomeSafe,
{
    /// Read container metadata from the `.pgno` at `path` without
    /// rehydrating a dragline (ADR-0018 §D7 / § Naming).
    ///
    /// Opens the file read-only, validates the container header's
    /// schema hash against `Event::<T>::ENVELOPE_HASH`, and returns
    /// a [`StoreMetadata`] snapshot. No fiber-state, line, or
    /// cursor data is materialised; the file handle is dropped
    /// before return.
    ///
    /// # Errors
    ///
    /// Returns [`PardosaError::CursorJournalOpen`] when the file
    /// cannot be opened, and [`PardosaError::CursorRead`] wrapping
    /// [`crate::persist::Error::SchemaHashMismatch`] (or other
    /// framing errors) when the header is invalid for `T`.
    pub fn metadata(path: &Path) -> Result<StoreMetadata, PardosaError> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| PardosaError::CursorJournalOpen {
                source: Box::new(e),
            })?;
        let reader = crate::typed::TypedReader::<std::fs::File, T>::open(file).map_err(|e| {
            PardosaError::CursorRead {
                source: Box::new(e),
            }
        })?;
        Ok(StoreMetadata {
            len: reader.message_count(),
            schema_hash: reader.schema_hash(),
            schema_source: reader.schema_source().map(String::from),
        })
    }
}
