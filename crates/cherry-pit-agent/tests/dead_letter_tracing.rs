//! Golden-file schema test for [`cherry_pit_agent::TracingDeadLetterSink`]
//! per CHE-0024:R5 + CHE-0040:R3 + CHE-0051:R7.
//!
//! Locks the field set + ordering + level of the structured
//! `tracing::error!` event emitted by the default sink. The schema
//! is the public observability contract for v0.1; changes here are
//! breaking and must update CHE-0024:R5 / CHE-0051:R7 first.
//!
//! ## Capture strategy
//!
//! We use a per-test scoped subscriber via
//! [`tracing::subscriber::with_default`] writing into a
//! `Mutex<Vec<u8>>` `MakeWriter`. No global subscriber is set —
//! parallel tests in the same binary remain isolated, satisfying
//! the contract `abort_if` "Golden-file test cannot capture tracing
//! without setting a global subscriber" — halt.

use std::io::Write;
use std::sync::{Arc, Mutex};

use cherry_pit_agent::{DeadLetterRecord, DeadLetterSink, TracingDeadLetterSink};
use cherry_pit_core::ErrorCategory;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct VecWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl VecWriter {
    fn snapshot(&self) -> String {
        String::from_utf8(self.buf.lock().unwrap().clone()).expect("utf-8")
    }
}

impl Write for VecWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for VecWriter {
    type Writer = VecWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Strip volatile fields (timestamp prefix, UUIDs) so the assertion
/// targets only the schema, not the values.
fn redact(s: &str) -> String {
    // Drop the leading timestamp `YYYY-MM-DDTHH:MM:SS.ffffffZ ` that
    // the default fmt layer prepends; everything from " ERROR" onward
    // is the schema we care about.
    let after_ts = s.find(" ERROR ").map_or(s, |idx| &s[idx + 1..]).to_string();
    // Replace UUIDv7 hex (8-4-4-4-12) with a stable token.
    regex_lite_uuid(&after_ts)
}

/// Tiny inline UUID redactor — avoids pulling regex into dev-deps.
/// Walks the string char-by-char looking for `8hex-4hex-4hex-4hex-12hex`
/// patterns and rewrites each to `<uuid>`.
fn regex_lite_uuid(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 36 <= bytes.len() && is_uuid_at(&bytes[i..i + 36]) {
            out.push_str("<uuid>");
            i += 36;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn is_uuid_at(b: &[u8]) -> bool {
    // 8-4-4-4-12 hex with hyphens at 8, 13, 18, 23
    if b.len() < 36 {
        return false;
    }
    let hex = |c: u8| c.is_ascii_hexdigit();
    let dash_at = [8, 13, 18, 23];
    for (i, &c) in b.iter().take(36).enumerate() {
        if dash_at.contains(&i) {
            if c != b'-' {
                return false;
            }
        } else if !hex(c) {
            return false;
        }
    }
    true
}

#[tokio::test]
async fn tracing_dead_letter_sink_emits_pinned_schema() {
    let writer = VecWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .with_target(false)
        .with_level(true)
        .finish();

    let event_id = uuid::Uuid::now_v7();
    let correlation_id = uuid::Uuid::now_v7();
    let causation_id = uuid::Uuid::now_v7();

    tracing::subscriber::with_default(subscriber, || {
        let sink = TracingDeadLetterSink::new();
        let record = DeadLetterRecord::new(
            event_id,
            Some(correlation_id),
            Some(causation_id),
            ErrorCategory::Terminal,
            "Notify",
            "OrderNotifier",
            "smtp 421".into(),
        );
        // record() is async but the future is Ready on first poll —
        // a tiny block_on suffices and keeps the test sync within
        // the with_default scope.
        futures_block_on(sink.record(record)).unwrap();
    });

    let captured = writer.snapshot();
    let redacted = redact(&captured);

    let expected = "ERROR policy output dispatch failed terminally; routed to dead-letter \
                    event_id=<uuid> \
                    correlation_id=Some(<uuid>) \
                    causation_id=Some(<uuid>) \
                    error_category=Terminal \
                    output_type=\"Notify\" \
                    policy_identity=\"OrderNotifier\" \
                    error_message=smtp 421\n";

    assert_eq!(
        redacted, expected,
        "tracing dead-letter schema drift — see CHE-0024:R5 / CHE-0051:R7 before changing"
    );
}

#[tokio::test]
async fn tracing_dead_letter_sink_renders_none_uuid_fields() {
    let writer = VecWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .with_target(false)
        .with_level(true)
        .finish();

    let event_id = uuid::Uuid::now_v7();

    tracing::subscriber::with_default(subscriber, || {
        let sink = TracingDeadLetterSink::new();
        let record = DeadLetterRecord::new(
            event_id,
            None,
            None,
            ErrorCategory::Terminal,
            "Notify",
            "OrderNotifier",
            "no correlation".into(),
        );
        futures_block_on(sink.record(record)).unwrap();
    });

    let captured = writer.snapshot();
    let redacted = redact(&captured);

    assert!(
        redacted.contains("correlation_id=None"),
        "Option<Uuid> None must render as `None` (not be dropped); got: {redacted}"
    );
    assert!(
        redacted.contains("causation_id=None"),
        "Option<Uuid> None must render as `None`; got: {redacted}"
    );
}

/// Minimal future-blocker for a future known to be `Ready` on first
/// poll. The [`TracingDeadLetterSink::record`] body is `tracing::error!`
/// (sync) followed by `async move { Ok(()) }` — yields immediately.
fn futures_block_on<F: std::future::Future>(mut fut: F) -> F::Output {
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    // SAFETY: fut is on the stack and not moved after this point.
    let fut = unsafe { Pin::new_unchecked(&mut fut) };
    let mut cx = Context::from_waker(Waker::noop());
    match fut.poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("TracingDeadLetterSink::record future must be Ready on first poll"),
    }
}

/// Per S7 §1 line 40: explicit golden lock on the [`DeadLetterRecord`]
/// public field shape. Constructs a record with every public field
/// set, then asserts the rendered tracing event names every field —
/// any future field rename, reorder, or deletion fails this test
/// loud-and-clear independently of the dispatcher round-trip.
#[test]
fn dead_letter_record_field_shape_is_locked() {
    let writer = VecWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .with_target(false)
        .with_level(true)
        .finish();

    tracing::subscriber::with_default(subscriber, || {
        let sink = TracingDeadLetterSink::new();
        let record = DeadLetterRecord::new(
            uuid::Uuid::now_v7(),
            Some(uuid::Uuid::now_v7()),
            Some(uuid::Uuid::now_v7()),
            ErrorCategory::Terminal,
            "Cmd",
            "Pol",
            "boom".into(),
        );
        futures_block_on(sink.record(record)).unwrap();
    });

    let captured = writer.snapshot();
    // Lock every public field name verbatim. If a field is renamed or
    // dropped, this assertion fails — and the failing field name is
    // visible in the diff.
    for field in [
        "event_id=",
        "correlation_id=",
        "causation_id=",
        "error_category=",
        "output_type=",
        "policy_identity=",
        "error_message=",
    ] {
        assert!(
            captured.contains(field),
            "DeadLetterRecord field `{field}` missing from tracing render \
             — public field shape changed; update CHE-0024:R5 / CHE-0051:R7 first.\n\
             captured = {captured}"
        );
    }
}
