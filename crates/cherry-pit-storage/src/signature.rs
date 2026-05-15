//! Content-addressable signature utilities.
//!
//! Provides canonical JSON serialization and SHA-256 hashing for
//! deterministic, content-addressable diffing of structured data.

use sha2::{Digest, Sha256};

/// Build a deterministic SHA-256 signature from a JSON [`Value`],
/// excluding the `run_timestamp` field.
///
/// The `run_timestamp` field is excluded so that signatures remain stable
/// across runs on the same day — only actual data changes invalidate the
/// checkpoint.
#[must_use]
pub fn build_snapshot_signature(snapshot: Option<&serde_json::Value>) -> String {
    let canonical = match snapshot {
        Some(serde_json::Value::Object(map)) => {
            // Build canonical JSON directly, filtering run_timestamp without cloning
            let mut keys: Vec<&str> = map
                .keys()
                .filter(|k| k.as_str() != "run_timestamp")
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let v = canonical_json(&map[k]);
                    format!("\"{}\":{v}", escape_json_string(k))
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        Some(other) => canonical_json(other),
        None => "{}".to_string(),
    };

    let hash = Sha256::digest(canonical.as_bytes());
    hash.iter()
        .fold(String::with_capacity(64), |mut acc, byte| {
            use std::fmt::Write;
            let _ = write!(acc, "{byte:02x}");
            acc
        })
}

/// Produce a canonical JSON string with sorted keys and compact separators.
///
/// Output is used for SHA-256 signing and must produce byte-identical output
/// for semantically identical inputs. Keys are sorted lexicographically at
/// every nesting level. No whitespace separators. This is intentionally
/// simple and correct rather than fast — checkpoint signatures are computed
/// at most once per run.
pub(crate) fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    let v = canonical_json(&map[k]);
                    format!("\"{}\":{v}", escape_json_string(k))
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        serde_json::Value::Array(arr) => {
            let entries: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", entries.join(","))
        }
        serde_json::Value::String(s) => format!("\"{}\"", escape_json_string(s)),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
    }
}

/// Escape a string for JSON embedding.
///
/// Handles RFC 8259 §7 required escapes: `\x00`–`\x1F` control chars,
/// `"`, and `\\`. Also escapes `U+007F` (DEL) and C1 controls
/// (`U+0080`–`U+009F`) via Rust's `char::is_control()` catch-all.
/// Deterministic for signing purposes.
#[must_use]
pub(crate) fn escape_json_string(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_snapshot_signature ────────────────────────────────────

    #[test]
    fn signature_none_is_deterministic() {
        let s1 = build_snapshot_signature(None);
        let s2 = build_snapshot_signature(None);
        assert_eq!(s1, s2);
        assert_eq!(s1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn signature_excludes_run_timestamp() {
        let a = serde_json::json!({"total": 5, "run_timestamp": "old"});
        let b = serde_json::json!({"total": 5, "run_timestamp": "new"});
        assert_eq!(
            build_snapshot_signature(Some(&a)),
            build_snapshot_signature(Some(&b))
        );
    }

    #[test]
    fn signature_differs_when_data_changes() {
        let a = serde_json::json!({"total": 5});
        let b = serde_json::json!({"total": 6});
        assert_ne!(
            build_snapshot_signature(Some(&a)),
            build_snapshot_signature(Some(&b))
        );
    }

    #[test]
    fn signature_non_object_input_does_not_panic() {
        let arr = serde_json::json!([1, 2, 3]);
        let sig = build_snapshot_signature(Some(&arr));
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn signature_deeply_nested_objects() {
        let val = serde_json::json!({"a": {"b": {"c": {"d": {"e": 42}}}}});
        let sig = build_snapshot_signature(Some(&val));
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn signature_empty_string_vs_null_differ() {
        let with_empty = serde_json::json!({"k": ""});
        let with_null = serde_json::json!({"k": null});
        assert_ne!(
            build_snapshot_signature(Some(&with_empty)),
            build_snapshot_signature(Some(&with_null))
        );
    }

    // ── canonical_json ──────────────────────────────────────────────

    #[test]
    fn canonical_json_sorts_keys() {
        let val = serde_json::json!({"z": 1, "a": 2, "m": 3});
        assert_eq!(canonical_json(&val), r#"{"a":2,"m":3,"z":1}"#);
    }

    #[test]
    fn canonical_json_nested() {
        let val = serde_json::json!({"b": {"d": 1, "c": 2}, "a": [3, 4]});
        assert_eq!(canonical_json(&val), r#"{"a":[3,4],"b":{"c":2,"d":1}}"#);
    }

    #[test]
    fn canonical_json_empty_object() {
        assert_eq!(canonical_json(&serde_json::json!({})), "{}");
    }

    #[test]
    fn canonical_json_string_escaping() {
        let val = serde_json::json!({"key": "val\"ue"});
        assert_eq!(canonical_json(&val), r#"{"key":"val\"ue"}"#);
    }

    #[test]
    fn canonical_json_null_and_bool() {
        let val = serde_json::json!({"a": null, "b": true, "c": false});
        assert_eq!(canonical_json(&val), r#"{"a":null,"b":true,"c":false}"#);
    }

    // ── escape_json_string ───────────────────────────────────────────

    #[test]
    fn escape_json_string_backslash() {
        assert_eq!(escape_json_string(r"\"), r"\\");
    }

    #[test]
    fn escape_json_string_double_quote() {
        assert_eq!(escape_json_string("\""), "\\\"");
    }

    #[test]
    fn escape_json_string_c0_control_chars() {
        assert_eq!(escape_json_string("\0"), "\\u0000");
        assert_eq!(escape_json_string("\n"), "\\n");
        assert_eq!(escape_json_string("\r"), "\\r");
        assert_eq!(escape_json_string("\t"), "\\t");
    }

    #[test]
    fn escape_json_string_del() {
        assert_eq!(escape_json_string("\x7F"), "\\u007f");
    }

    #[test]
    fn escape_json_string_c1_control() {
        assert_eq!(escape_json_string("\u{0085}"), "\\u0085");
    }

    #[test]
    fn escape_json_string_normal_ascii_passthrough() {
        assert_eq!(escape_json_string("hello world 123"), "hello world 123");
    }

    #[test]
    fn escape_json_string_multibyte_utf8_passthrough() {
        assert_eq!(escape_json_string("café"), "café");
        assert_eq!(escape_json_string("日本語"), "日本語");
    }

    #[test]
    fn escape_json_string_empty() {
        assert_eq!(escape_json_string(""), "");
    }

    #[test]
    fn signature_keys_with_special_characters() {
        let val = serde_json::json!({"key\"with\\escapes": 1, "normal": 2});
        let sig = build_snapshot_signature(Some(&val));
        assert_eq!(sig.len(), 64);
        // Deterministic
        assert_eq!(sig, build_snapshot_signature(Some(&val)));
    }

    #[test]
    fn signature_large_input() {
        let mut map = serde_json::Map::new();
        for i in 0..1000 {
            map.insert(format!("key_{i:04}"), serde_json::Value::Number(i.into()));
        }
        let val = serde_json::Value::Object(map);
        let sig = build_snapshot_signature(Some(&val));
        assert_eq!(sig.len(), 64);
    }

    // ── canonical_json proptest ─────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
            let leaf = prop_oneof![
                Just(serde_json::Value::Null),
                any::<bool>().prop_map(serde_json::Value::Bool),
                (-1_000_000i64..1_000_000i64)
                    .prop_map(|n| serde_json::Value::Number(serde_json::Number::from(n))),
                "[a-zA-Z0-9 _]{0,20}".prop_map(serde_json::Value::String),
            ];
            leaf.prop_recursive(
                3,  // depth
                32, // desired size
                4,  // items per collection
                |inner| {
                    prop_oneof![
                        prop::collection::vec(inner.clone(), 0..4)
                            .prop_map(serde_json::Value::Array),
                        prop::collection::hash_map("[a-z]{1,5}", inner, 0..4).prop_map(|map| {
                            serde_json::Value::Object(map.into_iter().collect::<serde_json::Map<
                                String,
                                serde_json::Value,
                            >>(
                            ))
                        }),
                    ]
                },
            )
        }

        proptest! {
            #[test]
            fn canonical_json_idempotent(v in arb_json_value()) {
                let first = canonical_json(&v);
                let parsed: serde_json::Value = serde_json::from_str(&first).unwrap();
                let second = canonical_json(&parsed);
                prop_assert_eq!(first, second);
            }

            #[test]
            fn canonical_json_deterministic(v in arb_json_value()) {
                let a = canonical_json(&v);
                let b = canonical_json(&v);
                prop_assert_eq!(a, b);
            }
        }
    }
}
