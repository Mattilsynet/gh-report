use super::*;
use crate::dragline::DEFAULT_ANCHOR_BUFFER_CAP;
use crate::dragline::Line;
use std::io::Cursor;
/// Roadmap IO-PG-1 / oracle invariant I1: the append-shape
/// persist path must produce byte-identical output to the
/// full-rewrite persist path for the same dragline. Already
/// proven at the substrate level
/// (`pardosa-file/tests/append_writer_core.rs::append_writer_finish_byte_identical_to_one_shot_writer`);
/// this is the pardosa-layer pin that catches future regressions
/// where the two persist functions might drift on dragline
/// shapes the substrate-level test does not exercise (anchor
/// configuration, fiber lifecycle verbs, schema source).
#[test]
fn persist_with_source_append_is_byte_identical_to_full_rewrite() {
    let mut dragline: Line<u64> =
        Line::with_anchor_config("iopg1-parity".to_owned(), 1, DEFAULT_ANCHOR_BUFFER_CAP);
    for i in 0..7u64 {
        let _ = dragline.create(i).expect("commit");
    }
    let mut rewrite_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source(&dragline, &mut rewrite_sink, None)
        .expect("full-rewrite persist must succeed");
    let rewrite_bytes = rewrite_sink.into_inner();
    let mut append_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source_append(&dragline, &mut append_sink, None)
        .expect("append-shape persist must succeed");
    let append_bytes = append_sink.into_inner();
    assert_eq!(
        append_bytes, rewrite_bytes,
        "I1: persist_with_source_append must yield bytes byte-identical to persist_with_source for the same dragline",
    );
}
/// Schema-source slot must round-trip identically across both
/// persist paths — the embedded source is part of the header
/// hash region and any divergence is an I1 violation.
#[test]
fn persist_with_source_append_preserves_schema_source_slot() {
    let mut dragline: Line<u64> = Line::with_anchor_config(
        "iopg1-schema-source".to_owned(),
        1,
        DEFAULT_ANCHOR_BUFFER_CAP,
    );
    for i in 0..3u64 {
        let _ = dragline.create(i).expect("commit");
    }
    let schema_source = Some("test::SchemaSource v1");
    let mut rewrite_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source(&dragline, &mut rewrite_sink, schema_source)
        .expect("full-rewrite persist with schema_source");
    let mut append_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source_append(&dragline, &mut append_sink, schema_source)
        .expect("append-shape persist with schema_source");
    assert_eq!(
        append_sink.into_inner(),
        rewrite_sink.into_inner(),
        "I1: schema_source slot must round-trip identically across both persist paths",
    );
}
/// An empty dragline (zero events) must produce identical output
/// across both paths — the header / footer / index regions are
/// the entire output, so any header-flag drift surfaces here.
#[test]
fn persist_with_source_append_matches_full_rewrite_on_empty_dragline() {
    let dragline: Line<u64> = Line::new();
    let mut rewrite_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source(&dragline, &mut rewrite_sink, None)
        .expect("full-rewrite persist on empty dragline");
    let mut append_sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    persist_with_source_append(&dragline, &mut append_sink, None)
        .expect("append-shape persist on empty dragline");
    assert_eq!(
        append_sink.into_inner(),
        rewrite_sink.into_inner(),
        "I1: empty-dragline output must be byte-identical across both persist paths",
    );
}
/// Unpersistable preflight must fire on both paths with the same
/// variant — append-shape persist must not skip the
/// `check_persistable` guard the full-rewrite path enforces.
#[test]
fn persist_with_source_append_rejects_unpersistable_dragline() {
    let mut dragline: Line<u64> = Line::new();
    for i in 0..2u64 {
        let _ = dragline.create(i).expect("commit");
    }
    dragline.set_migrating(true);
    let mut sink: Cursor<Vec<u8>> = Cursor::new(Vec::new());
    let result = persist_with_source_append(&dragline, &mut sink, None);
    match result {
        Err(Error::UnpersistableState {
            kind: UnpersistableKind::Migrating,
        }) => {}
        other => panic!("expected UnpersistableState {{ Migrating }}, got {other:?}"),
    }
    assert!(
        sink.into_inner().is_empty(),
        "preflight must reject before any byte hits the sink",
    );
}
