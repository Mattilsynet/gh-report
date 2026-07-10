use pardosa_file::format::*;
#[test]
fn format_constants() {
    assert_eq!(MAGIC, *b"PGNO");
    assert_eq!(FORMAT_VERSION, 6);
    assert_eq!(FILE_HEADER_SIZE, 40);
    assert_eq!(FILE_FOOTER_SIZE, 32);
    assert_eq!(INDEX_ENTRY_SIZE, 24);
    assert_eq!(MIN_FILE_SIZE, 72);
    assert_eq!(NONE_SENTINEL, 0xFFFF_FFFF);
    assert_eq!(HEADER_MAGIC_OFFSET, 0);
    assert_eq!(HEADER_SCHEMA_HASH_LEN, 16);
    assert_eq!(HEADER_RESERVED_LEN, 7);
    assert_eq!(HEADER_MAGIC_OFFSET + 4, HEADER_VERSION_OFFSET);
    assert_eq!(HEADER_VERSION_OFFSET + 2, HEADER_FLAGS_OFFSET);
    assert_eq!(HEADER_FLAGS_OFFSET + 2, HEADER_SCHEMA_HASH_OFFSET);
    assert_eq!(
        HEADER_SCHEMA_HASH_OFFSET + HEADER_SCHEMA_HASH_LEN,
        HEADER_DICT_ID_OFFSET
    );
    assert_eq!(HEADER_DICT_ID_OFFSET + 4, HEADER_PAGE_CLASS_OFFSET);
    assert_eq!(HEADER_PAGE_CLASS_OFFSET + 1, HEADER_SCHEMA_SIZE_OFFSET);
    assert_eq!(HEADER_SCHEMA_SIZE_OFFSET + 4, HEADER_RESERVED_OFFSET);
    assert_eq!(
        HEADER_RESERVED_OFFSET + HEADER_RESERVED_LEN,
        FILE_HEADER_SIZE
    );
    assert_eq!(BARE_HEADER_SIZE, 23);
    assert_eq!(BARE_HEADER_COMPRESSED_SIZE, 27);
    assert_eq!(2 + HEADER_SCHEMA_HASH_LEN + 1 + 4, BARE_HEADER_SIZE);
    assert_eq!(
        2 + HEADER_SCHEMA_HASH_LEN + 1 + 4 + 4,
        BARE_HEADER_COMPRESSED_SIZE
    );
    assert_eq!(FOOTER_RESERVED_OFFSET, 16);
    assert_eq!(FOOTER_RESERVED_LEN, 4);
    assert_eq!(FOOTER_MAGIC_OFFSET, 20);
    assert_eq!(FOOTER_CHECKSUM_OFFSET, 24);
    assert_eq!(FOOTER_CHECKSUM_OFFSET + 8, FILE_FOOTER_SIZE);
}
#[test]
fn pad_to_8_cases() {
    assert_eq!(pad_to_8(0), 0);
    assert_eq!(pad_to_8(1), 8);
    assert_eq!(pad_to_8(7), 8);
    assert_eq!(pad_to_8(8), 8);
    assert_eq!(pad_to_8(9), 16);
    assert_eq!(pad_to_8(32), 32);
}
#[test]
fn messages_offset_no_schema() {
    assert_eq!(messages_offset(0), FILE_HEADER_SIZE);
}
#[test]
fn messages_offset_with_schema() {
    assert_eq!(messages_offset(100), FILE_HEADER_SIZE + pad_to_8(100));
    assert_eq!(messages_offset(8), FILE_HEADER_SIZE + pad_to_8(8));
    assert_eq!(pad_to_8(100), 104);
    assert_eq!(pad_to_8(8), 8);
}
#[test]
fn page_class_elements() {
    use pardosa_file::PageClass;
    assert_eq!(PageClass::Page0.max_elements(), 256);
    assert_eq!(PageClass::Page1.max_elements(), 4_096);
    assert_eq!(PageClass::Page2.max_elements(), 65_536);
    assert_eq!(PageClass::Page3.max_elements(), 1_048_576);
}
#[test]
fn page_class_from_byte() {
    use pardosa_file::PageClass;
    assert_eq!(PageClass::from_byte(0), Some(PageClass::Page0));
    assert_eq!(PageClass::from_byte(3), Some(PageClass::Page3));
    assert_eq!(PageClass::from_byte(4), None);
    assert_eq!(PageClass::from_byte(255), None);
}
#[test]
fn schema_size_header_offset() {
    assert_eq!(HEADER_PAGE_CLASS_OFFSET + 1, HEADER_SCHEMA_SIZE_OFFSET);
}
#[test]
fn file_error_display() {
    let err = pardosa_file::FileError::InvalidSchemaSource;
    let s = format!("{err}");
    assert!(s.contains("UTF-8"), "got: {s}");
}
