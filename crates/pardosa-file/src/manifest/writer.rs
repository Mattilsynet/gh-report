use super::record::{ManifestRecord, encode_footer, encode_header, encode_record};
use std::io::{self, SeekFrom};
/// Incremental manifest writer. Tracks the records already persisted
/// to the manifest sink so each [`Self::sync_data`] writes only the
/// delta plus a fixed 28-byte footer — `O(delta_records)` bytes per
/// sync, not `O(records)`.
pub(crate) struct IndexManifestWriter<'w, M: crate::Syncable + std::io::Seek> {
    sink: &'w mut M,
    schema_hash: u128,
    page_class: u8,
    schema_size: u32,
    records: Vec<ManifestRecord>,
    synced_records: usize,
    header_synced: bool,
}
impl<'w, M: crate::Syncable + std::io::Seek> IndexManifestWriter<'w, M> {
    pub(crate) fn new_with_records(
        sink: &'w mut M,
        schema_hash: u128,
        page_class: u8,
        schema_size: u32,
        records: Vec<ManifestRecord>,
        synced_records: usize,
        header_synced: bool,
    ) -> Self {
        Self {
            sink,
            schema_hash,
            page_class,
            schema_size,
            synced_records: synced_records.min(records.len()),
            records,
            header_synced,
        }
    }
    pub(crate) fn record(&mut self, entry: ManifestRecord) {
        self.records.push(entry);
    }
    /// Expected on-disk length of the manifest **after** the most
    /// recent [`Self::sync_data`] succeeded. Zero before the first
    /// sync. Computed from `synced_records`, not by querying the
    /// sink — the writer never needs to observe the sink's actual
    /// length to remain consistent.
    pub(crate) fn persisted_len(&self) -> u64 {
        if !self.header_synced {
            return 0;
        }
        (super::MANIFEST_HEADER_SIZE as u64)
            + (self.synced_records as u64) * (super::MANIFEST_RECORD_SIZE as u64)
            + (super::MANIFEST_FOOTER_SIZE as u64)
    }
    pub(crate) fn sync_data(&mut self, data_end: u64, frontier: [u8; 32]) -> io::Result<()> {
        if !self.header_synced {
            self.sink.seek(SeekFrom::Start(0))?;
            let header = encode_header(self.schema_hash, self.page_class, self.schema_size);
            self.sink.write_all(&header)?;
            self.header_synced = true;
        }
        let delta_start = self.synced_records;
        let delta_records = &self.records[delta_start..];
        if delta_records.is_empty() {
            let footer_pos = (super::MANIFEST_HEADER_SIZE as u64)
                + (self.records.len() as u64) * (super::MANIFEST_RECORD_SIZE as u64);
            self.sink.seek(SeekFrom::Start(footer_pos))?;
        } else {
            let delta_pos = (super::MANIFEST_HEADER_SIZE as u64)
                + (delta_start as u64) * (super::MANIFEST_RECORD_SIZE as u64);
            self.sink.seek(SeekFrom::Start(delta_pos))?;
            let mut record_buf = [0u8; super::MANIFEST_RECORD_SIZE];
            for r in delta_records {
                encode_record(r, &mut record_buf);
                self.sink.write_all(&record_buf)?;
            }
        }
        let message_count = u64::try_from(self.records.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "manifest record count overflows u64",
            )
        })?;
        let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
        hasher.update(&encode_header(
            self.schema_hash,
            self.page_class,
            self.schema_size,
        ));
        let mut record_buf = [0u8; super::MANIFEST_RECORD_SIZE];
        for r in &self.records {
            encode_record(r, &mut record_buf);
            hasher.update(&record_buf);
        }
        hasher.update(&frontier);
        let checksum = hasher.digest();
        let footer = encode_footer(message_count, data_end, frontier, checksum);
        self.sink.write_all(&footer)?;
        let total_len = (super::MANIFEST_HEADER_SIZE as u64)
            + (self.records.len() as u64) * (super::MANIFEST_RECORD_SIZE as u64)
            + (super::MANIFEST_FOOTER_SIZE as u64);
        <M as crate::Syncable>::set_len(self.sink, total_len)?;
        <M as crate::Syncable>::sync_data(self.sink)?;
        self.synced_records = self.records.len();
        Ok(())
    }
}
