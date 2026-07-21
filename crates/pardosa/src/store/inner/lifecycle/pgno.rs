use super::{Decode, GenomeSafe, PardosaError, Path, PathBuf, Validate, ValidatedReplayError};
use pardosa_file::manifest::{
    ManifestRecord, RecoveredPrefix, RecoveryError, RecoveryOutcome, RecoveryReaderErrorKind,
    finalize_recovered_prefix, recover_footerless_prefix,
};
use pardosa_file::{FileError, Reader, Syncable};
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
                super::persist_error_to_cursor_read(crate::persist::Error::File(
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
        return Err(super::persist_error_to_cursor_read(
            crate::persist::Error::File(FileError::TornWriteRecovery {
                source: Box::new(RecoveryError::Manifest(FileError::InvalidIndex)),
            }),
        ));
    }
    let pgno_bytes = std::fs::read(path).map_err(|source| PardosaError::CursorJournalOpen {
        source: Box::new(source),
    })?;
    let file_len = pgno_bytes.len() as u64;
    let status = reader_recovery_status(&pgno_bytes)?;
    let manifest_bytes = std::fs::read(manifest_path)
        .map_err(|source| super::persist_error_to_cursor_read(crate::persist::Error::Io(source)))?;
    let recovered =
        recover_footerless_prefix(&pgno_bytes, &manifest_bytes).map_err(recovery_declined)?;
    let records_preserved = u64::try_from(recovered.records.len()).map_err(|_| {
        super::persist_error_to_cursor_read(crate::persist::Error::File(FileError::InvalidIndex))
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
        return Err(super::persist_error_to_cursor_read(
            crate::persist::Error::File(FileError::TornWriteRecovery {
                source: Box::new(RecoveryError::Manifest(FileError::InvalidIndex)),
            }),
        ));
    };
    let (recovered, original_file_len) =
        recover_footerless_pgno_at_path(path).map_err(super::persist_error_to_cursor_read)?;
    let recovered_records = u64::try_from(recovered.records.len()).map_err(|_| {
        super::persist_error_to_cursor_read(crate::persist::Error::File(FileError::InvalidIndex))
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

pub(super) type OpenUnchecked<T> = (
    std::fs::File,
    crate::dragline::Line<T>,
    RecoveredPrefix,
    bool,
    Option<RecoveryOutcome>,
);

pub(super) type OpenValidated<T> = (
    std::fs::File,
    crate::dragline::Line<T>,
    Option<RecoveryOutcome>,
);

pub(super) fn open_rw_seek_and_rehydrate_unchecked<T>(
    path: &Path,
    mode: crate::persist::PrecursorCheckMode,
) -> Result<OpenUnchecked<T>, PardosaError>
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
        match crate::persist::rehydrate_unchecked::<T, _>(&mut file, mode) {
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
                let dragline = crate::persist::rehydrate_unchecked::<T, _>(&mut file, mode)
                    .map_err(|e| PardosaError::CursorRead {
                        source: Box::new(e),
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
pub(super) fn open_rw_seek_and_rehydrate_validated<T>(
    path: &Path,
    mode: crate::persist::PrecursorCheckMode,
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
    let (dragline, last_recovery) =
        match crate::persist::rehydrate_validated::<T, _>(&mut file, mode) {
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
                    crate::persist::rehydrate_validated::<T, _>(&mut file, mode)?,
                    Some(recovery_outcome),
                )
            }
            Err(e) => return Err(e),
        };
    file.seek(SeekFrom::Start(0))
        .map_err(|e| ValidatedReplayError::Replay(crate::persist::Error::Io(e)))?;
    Ok((file, dragline, last_recovery))
}
