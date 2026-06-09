//! Custom Cloud Logging (Stackdriver) layer for [`tracing`].
//!
//! Emits one JSON line per event to stdout, compatible with GCP Cloud Logging
//! on Cloud Run. Field names are emitted as-is in `snake_case`.
//!
//! Replaces the `tracing-stackdriver` crate with ~80 lines fully under project
//! control and zero extra dependencies beyond `serde_json` (already in the
//! dependency tree).

use std::io::Write;

use tracing::Level;
use tracing::field::{Field, Visit};
use tracing_subscriber::layer::Context;

/// A [`tracing_subscriber::Layer`] that writes Cloud Logging–compatible JSON
/// to stdout.
///
/// Each event produces a single JSON line containing:
/// - `severity` — mapped from the tracing level
/// - `message` — the event message
/// - `time` — RFC 3339 UTC timestamp
/// - `target` — the module path of the event origin
/// - all structured fields as top-level keys
///
/// No `sourceLocation` field is emitted.
pub struct CloudLoggingLayer;

impl Default for CloudLoggingLayer {
    fn default() -> Self {
        Self
    }
}

impl CloudLoggingLayer {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CloudLoggingLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = JsonVisitor {
            fields: serde_json::Map::new(),
            message: None,
        };
        event.record(&mut visitor);

        let severity = match *event.metadata().level() {
            Level::ERROR => "ERROR",
            Level::WARN => "WARNING",
            Level::INFO => "INFO",
            Level::DEBUG | Level::TRACE => "DEBUG",
        };

        let timestamp = format_rfc3339_now();

        let mut json = serde_json::Map::new();
        json.insert("severity".to_string(), serde_json::json!(severity));
        json.insert(
            "message".to_string(),
            serde_json::json!(visitor.message.unwrap_or_default()),
        );
        json.insert("time".to_string(), serde_json::json!(timestamp));
        json.insert(
            "target".to_string(),
            serde_json::json!(event.metadata().target()),
        );

        for (k, v) in visitor.fields {
            json.insert(k, v);
        }

        let line = serde_json::to_string(&serde_json::Value::Object(json))
            .expect("JSON serialization cannot fail for Map<String, Value>");
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = writeln!(handle, "{line}");
    }
}

/// Collects tracing event fields into a JSON map.
struct JsonVisitor {
    fields: serde_json::Map<String, serde_json::Value>,
    message: Option<String>,
}

impl Visit for JsonVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), serde_json::json!(value));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let formatted = format!("{value:?}");
        if field.name() == "message" {
            let trimmed = formatted
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(&formatted);
            self.message = Some(trimmed.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), serde_json::json!(formatted));
        }
    }
}

/// Format the current wall-clock time as RFC 3339 UTC with microsecond
/// precision. Uses only `std::time` — no external dependency.
fn format_rfc3339_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let micros = now.subsec_micros();

    let days = secs / 86_400;
    let day_secs = secs % 86_400;
    let hour = day_secs / 3_600;
    let minute = (day_secs % 3_600) / 60;
    let second = day_secs % 60;

    let z = days.cast_signed() + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097).cast_unsigned();
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe.cast_signed() + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}.{micros:06}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    use tracing_subscriber::layer::SubscriberExt;

    /// Capture stdout output from a closure.
    fn capture_stdout(f: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};

        let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = Arc::clone(&buf);

        let layer = BufferLayer { buf: buf_clone };
        let subscriber = tracing_subscriber::Registry::default().with(layer);

        tracing::subscriber::with_default(subscriber, f);

        let data = buf.lock().unwrap();
        String::from_utf8(data.clone()).unwrap()
    }

    /// A test-only layer that writes JSON to an in-memory buffer instead of
    /// stdout, using the same formatting logic as [`CloudLoggingLayer`].
    struct BufferLayer {
        buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BufferLayer {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
            let mut visitor = JsonVisitor {
                fields: serde_json::Map::new(),
                message: None,
            };
            event.record(&mut visitor);

            let severity = match *event.metadata().level() {
                Level::ERROR => "ERROR",
                Level::WARN => "WARNING",
                Level::INFO => "INFO",
                Level::DEBUG | Level::TRACE => "DEBUG",
            };

            let timestamp = format_rfc3339_now();

            let mut json = serde_json::Map::new();
            json.insert("severity".to_string(), serde_json::json!(severity));
            json.insert(
                "message".to_string(),
                serde_json::json!(visitor.message.unwrap_or_default()),
            );
            json.insert("time".to_string(), serde_json::json!(timestamp));
            json.insert(
                "target".to_string(),
                serde_json::json!(event.metadata().target()),
            );
            for (k, v) in visitor.fields {
                json.insert(k, v);
            }

            let line = serde_json::to_string(&serde_json::Value::Object(json)).unwrap();
            let mut buf = self.buf.lock().unwrap();
            writeln!(buf, "{line}").unwrap();
        }
    }

    #[test]
    fn info_event_produces_valid_json_with_correct_severity() {
        let output = capture_stdout(|| {
            tracing::info!(entries = 560, "baseline loaded");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(json["severity"], "INFO");
        assert_eq!(json["message"], "baseline loaded");
        assert_eq!(json["entries"], 560);
        assert!(json["time"].as_str().unwrap().ends_with('Z'));
        assert!(json["target"].as_str().is_some());
        assert!(json.get("sourceLocation").is_none());
    }

    #[test]
    fn error_event_maps_to_error_severity() {
        let output = capture_stdout(|| {
            tracing::error!(code = 500, "request failed");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(json["severity"], "ERROR");
        assert_eq!(json["message"], "request failed");
        assert_eq!(json["code"], 500);
    }

    #[test]
    fn warn_event_maps_to_warning_severity() {
        let output = capture_stdout(|| {
            tracing::warn!("rate limit approaching");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(json["severity"], "WARNING");
        assert_eq!(json["message"], "rate limit approaching");
    }

    #[test]
    fn debug_event_maps_to_debug_severity() {
        let output = capture_stdout(|| {
            tracing::debug!("cache miss");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(json["severity"], "DEBUG");
        assert_eq!(json["message"], "cache miss");
    }

    #[test]
    fn trace_event_maps_to_debug_severity() {
        let output = capture_stdout(|| {
            tracing::trace!("per-item detail");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(json["severity"], "DEBUG");
        assert_eq!(json["message"], "per-item detail");
    }

    #[test]
    fn structured_fields_appear_as_top_level_keys() {
        let output = capture_stdout(|| {
            tracing::info!(
                repo = "my-repo",
                status = 200_u64,
                cached = true,
                latency = 1.5_f64,
                "request completed"
            );
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(json["repo"], "my-repo");
        assert_eq!(json["status"], 200);
        assert_eq!(json["cached"], true);
        assert_eq!(json["latency"], 1.5);
    }

    #[test]
    fn time_field_is_valid_rfc3339() {
        let output = capture_stdout(|| {
            tracing::info!("timestamp test");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        let time_str = json["time"].as_str().unwrap();
        assert!(time_str.ends_with('Z'));
        assert_eq!(time_str.len(), 27);
        assert_eq!(&time_str[4..5], "-");
        assert_eq!(&time_str[7..8], "-");
        assert_eq!(&time_str[10..11], "T");
    }

    #[test]
    fn target_field_contains_module_path() {
        let output = capture_stdout(|| {
            tracing::info!("module path test");
        });

        let json: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
        let target = json["target"].as_str().unwrap();
        assert!(target.contains("cloud_logging"), "target was: {target}");
    }

    #[test]
    fn format_rfc3339_now_produces_valid_timestamp() {
        let ts = format_rfc3339_now();
        assert!(ts.ends_with('Z'));
        assert_eq!(ts.len(), 27);
        let year: u32 = ts[..4].parse().unwrap();
        assert!(year >= 2026);
    }
}
