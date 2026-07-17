use crate::opaque::OpaqueTail;
use pardosa::store::{Event, Precursor};
use pardosa_nats::JetStreamReplayRecord;
use pardosa_wire::DecodeError;
use serde::Serialize;
use std::fmt::Write as _;

/// Agent-facing RON render of a single [`JetStreamReplayRecord`]:
/// the envelope frame decoded structurally, the `domain_event` body
/// rendered as opaque hex (feynman orientation `adr-fmt-de91s` —
/// the wire format is schema-driven, not self-describing, so the
/// concrete payload type cannot be recovered without linking a
/// consumer's event types).
#[derive(Debug, Serialize)]
pub struct ReplayRecordRon {
    pub ack: u64,
    pub schema_tag: Option<String>,
    pub event_id: u64,
    pub fiber_id: u64,
    pub detached: bool,
    pub precursor: String,
    pub precursor_hash_hex: String,
    pub domain_event_hex: String,
    pub domain_event_byte_len: usize,
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Decode a single replay record's canonical payload bytes as an
/// envelope-partial [`Event<OpaqueTail>`] and project it into
/// [`ReplayRecordRon`].
///
/// # Errors
/// Returns [`DecodeError`] when the envelope frame itself is
/// malformed or truncated (an opaque, undecodable `domain_event`
/// body is the expected normal case, not an error).
pub fn decode_record(record: &JetStreamReplayRecord) -> Result<ReplayRecordRon, DecodeError> {
    let event = decode_envelope(&record.payload)?;
    Ok(render(
        record.ack.as_u64(),
        record.schema_tag.clone(),
        &event,
    ))
}

/// Decode canonical payload bytes as an envelope-partial
/// [`Event<OpaqueTail>`] (frame structured, `domain_event` opaque).
///
/// # Errors
/// Returns [`DecodeError`] when the envelope frame is malformed or
/// truncated.
pub fn decode_envelope(payload: &[u8]) -> Result<Event<OpaqueTail>, DecodeError> {
    pardosa_wire::from_bytes(payload)
}

fn render(ack: u64, schema_tag: Option<String>, event: &Event<OpaqueTail>) -> ReplayRecordRon {
    let precursor = match event.precursor() {
        Precursor::Genesis => "Genesis".to_string(),
        Precursor::Of(index) => format!("Of({})", index.value()),
        _ => "Unknown".to_string(),
    };
    ReplayRecordRon {
        ack,
        schema_tag,
        event_id: event.event_id().value(),
        fiber_id: event.fiber_id().value(),
        detached: event.detached(),
        precursor,
        precursor_hash_hex: hex_bytes(&event.precursor_hash()),
        domain_event_hex: event.domain_event().hex(),
        domain_event_byte_len: event.domain_event().byte_len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pardosa::store::{EventId, FiberId, Index};

    #[test]
    fn synthetic_event_decodes_with_opaque_hex_body() {
        let event = Event::new_unchecked(
            EventId::new(7),
            FiberId::new(3),
            false,
            Precursor::Of(Index::new(6)),
            [0u8; 32],
            42u64,
        );
        let bytes = pardosa_wire::to_vec(&event);

        let decoded = decode_envelope(&bytes).expect("envelope-partial decode");
        let rendered = render(1, Some("gh-report.v18.events".to_string()), &decoded);

        assert_eq!(rendered.event_id, 7);
        assert_eq!(rendered.fiber_id, 3);
        assert!(!rendered.detached);
        assert_eq!(rendered.precursor, "Of(6)");
        assert_eq!(rendered.precursor_hash_hex, "00".repeat(32));
        assert_eq!(rendered.domain_event_byte_len, 8);
        assert_eq!(rendered.domain_event_hex, hex_bytes(&42u64.to_le_bytes()));

        let ron = ron::ser::to_string_pretty(&rendered, ron::ser::PrettyConfig::default())
            .expect("ron render");
        assert!(ron.contains("event_id"));
        assert!(ron.contains(&rendered.domain_event_hex));
    }
}
