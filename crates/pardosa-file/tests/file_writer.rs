use pardosa_file::Writer;
use pardosa_file::format::{
    FILE_FOOTER_SIZE, FILE_HEADER_SIZE, FOOTER_CHECKSUM_OFFSET, FOOTER_INDEX_OFFSET,
    FOOTER_MAGIC_OFFSET, FOOTER_MESSAGE_COUNT_OFFSET, FOOTER_RESERVED_LEN, FOOTER_RESERVED_OFFSET,
    FORMAT_VERSION, HEADER_DICT_ID_OFFSET, HEADER_FLAGS_OFFSET, HEADER_MAGIC_OFFSET,
    HEADER_PAGE_CLASS_OFFSET, HEADER_RESERVED_LEN, HEADER_RESERVED_OFFSET, HEADER_SCHEMA_HASH_LEN,
    HEADER_SCHEMA_HASH_OFFSET, HEADER_SCHEMA_SIZE_OFFSET, HEADER_VERSION_OFFSET, INDEX_ENTRY_SIZE,
    MAGIC, MIN_FILE_SIZE,
};
use xxhash_rust::xxh64::xxh64;
const KNOWN_HASH: u128 = 0x0F0E_0D0C_0B0A_0908_0706_0504_0302_0100;
fn assert_header_bytes(buf: &[u8], schema_hash: u128, page_class: u8, schema_size: u32) {
    assert_eq!(
        &buf[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + 4],
        &MAGIC,
        "header magic must be ASCII \"PGNO\" at offset 0",
    );
    assert_eq!(
        u16::from_le_bytes(
            buf[HEADER_VERSION_OFFSET..HEADER_VERSION_OFFSET + 2]
                .try_into()
                .unwrap()
        ),
        FORMAT_VERSION,
        "FORMAT_VERSION must be {FORMAT_VERSION}",
    );
    assert_eq!(
        &buf[HEADER_FLAGS_OFFSET..HEADER_FLAGS_OFFSET + 2],
        &[0u8; 2],
        "flags must be zero in SM-1 (no compression)",
    );
    assert_eq!(
        &buf[HEADER_SCHEMA_HASH_OFFSET..HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN],
        &schema_hash.to_le_bytes(),
        "schema_hash must be u128 LE",
    );
    assert_eq!(
        &buf[HEADER_DICT_ID_OFFSET..HEADER_DICT_ID_OFFSET + 4],
        &[0u8; 4],
        "dict_id must be zero in v2",
    );
    assert_eq!(buf[HEADER_PAGE_CLASS_OFFSET], page_class, "page_class");
    assert_eq!(
        u32::from_le_bytes(
            buf[HEADER_SCHEMA_SIZE_OFFSET..HEADER_SCHEMA_SIZE_OFFSET + 4]
                .try_into()
                .unwrap()
        ),
        schema_size,
        "schema_size",
    );
    assert_eq!(
        &buf[HEADER_RESERVED_OFFSET..HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN],
        &[0u8; HEADER_RESERVED_LEN],
        "reserved bytes must be zero",
    );
}
fn assert_footer_bytes(buf: &[u8], expected_index_offset: u64, expected_message_count: u64) {
    let footer = &buf[buf.len() - FILE_FOOTER_SIZE..];
    assert_eq!(
        u64::from_le_bytes(
            footer[FOOTER_INDEX_OFFSET..FOOTER_INDEX_OFFSET + 8]
                .try_into()
                .unwrap()
        ),
        expected_index_offset,
        "footer.index_offset",
    );
    assert_eq!(
        u64::from_le_bytes(
            footer[FOOTER_MESSAGE_COUNT_OFFSET..FOOTER_MESSAGE_COUNT_OFFSET + 8]
                .try_into()
                .unwrap()
        ),
        expected_message_count,
        "footer.message_count",
    );
    assert_eq!(
        &footer[FOOTER_RESERVED_OFFSET..FOOTER_RESERVED_OFFSET + FOOTER_RESERVED_LEN],
        &[0u8; FOOTER_RESERVED_LEN],
        "footer.reserved must be zero",
    );
    assert_eq!(
        &footer[FOOTER_MAGIC_OFFSET..FOOTER_MAGIC_OFFSET + 4],
        &MAGIC,
        "footer.magic must be \"PGNO\"",
    );
    let expected_cksum = xxh64(&footer[..FOOTER_CHECKSUM_OFFSET], 0);
    assert_eq!(
        u64::from_le_bytes(
            footer[FOOTER_CHECKSUM_OFFSET..FOOTER_CHECKSUM_OFFSET + 8]
                .try_into()
                .unwrap()
        ),
        expected_cksum,
        "footer.checksum must be xxh64(seed=0) of footer[0..24]",
    );
}
#[test]
fn zero_message_file_is_minimum_size() {
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH);
    w.finish().expect("finish");
    assert_eq!(
        buf.len(),
        MIN_FILE_SIZE,
        "0-msg file must be exactly MIN_FILE_SIZE = {MIN_FILE_SIZE} bytes",
    );
    assert_eq!(buf.len(), 72, "MIN_FILE_SIZE must be 72");
}
#[test]
fn zero_message_file_header_and_footer_bytes() {
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH);
    w.finish().expect("finish");
    assert_header_bytes(&buf, KNOWN_HASH, 0, 0);
    let expected_index_offset = FILE_HEADER_SIZE as u64;
    assert_footer_bytes(&buf, expected_index_offset, 0);
}
#[test]
fn zero_message_file_page_class_passthrough() {
    let mut buf = Vec::new();
    let w = Writer::new(&mut buf, KNOWN_HASH).with_page_class(3);
    w.finish().expect("finish");
    assert_header_bytes(&buf, KNOWN_HASH, 3, 0);
}
#[test]
fn one_message_file_layout() {
    let payload: &[u8] = b"hello-pardosa";
    let mut buf = Vec::new();
    let mut w = Writer::new(&mut buf, KNOWN_HASH);
    w.write_message(payload).expect("write_message");
    w.finish().expect("finish");
    let n = payload.len();
    let msg_off = FILE_HEADER_SIZE;
    let idx_off = msg_off + n;
    let footer_off = idx_off + INDEX_ENTRY_SIZE;
    let total = footer_off + FILE_FOOTER_SIZE;
    assert_eq!(buf.len(), total, "1-msg total file size");
    assert_header_bytes(&buf, KNOWN_HASH, 0, 0);
    assert_eq!(&buf[msg_off..msg_off + n], payload, "message body verbatim");
    let entry = &buf[idx_off..idx_off + INDEX_ENTRY_SIZE];
    assert_eq!(
        u64::from_le_bytes(entry[0..8].try_into().unwrap()),
        msg_off as u64,
        "index[0].offset",
    );
    assert_eq!(
        u32::from_le_bytes(entry[8..12].try_into().unwrap()),
        u32::try_from(n).unwrap(),
        "index[0].size",
    );
    assert_eq!(&entry[12..16], &[0u8; 4], "index[0].reserved must be zero");
    let expected_msg_cksum = xxh64(payload, 0);
    assert_eq!(
        u64::from_le_bytes(entry[16..24].try_into().unwrap()),
        expected_msg_cksum,
        "index[0].checksum must be xxh64(seed=0) of message body",
    );
    assert_footer_bytes(&buf, idx_off as u64, 1);
}
