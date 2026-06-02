use std::time::Duration;

use cherry_pit_storage::{
    DEFAULT_LOCK_FILENAME, DEFAULT_LOCK_TTL, PersistenceError, acquire, atomic_write_bytes,
    atomic_write_text, build_snapshot_signature, lock_path,
};
use proptest::prelude::*;
use serde_json::{Map, Value};
use tempfile::TempDir;

fn arb_json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1_000_000i64..1_000_000i64).prop_map(|n| Value::Number(serde_json::Number::from(n))),
        "[a-zA-Z0-9 _\"\\\\]{0,20}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 32, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(("[a-z]{1,5}", inner), 0..4).prop_map(|pairs| {
                let mut map = Map::new();
                for (k, v) in pairs {
                    map.insert(k, v);
                }
                Value::Object(map)
            }),
        ]
    })
}

fn arb_kv_pairs() -> impl Strategy<Value = Vec<(String, Value)>> {
    prop::collection::vec(("[a-z]{1,5}", arb_json_value()), 0..8).prop_map(|pairs| {
        let mut seen = std::collections::HashSet::new();
        pairs
            .into_iter()
            .filter(|(k, _)| seen.insert(k.clone()))
            .collect()
    })
}

fn object_from_pairs(pairs: &[(String, Value)]) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs {
        map.insert(k.clone(), v.clone());
    }
    Value::Object(map)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    #[test]
    fn p1_signature_is_deterministic(v in arb_json_value()) {
        let a = build_snapshot_signature(Some(&v));
        let b = build_snapshot_signature(Some(&v));
        prop_assert_eq!(a, b);
    }

    #[test]
    fn p2_signature_length_always_64_hex(v in arb_json_value()) {
        let sig = build_snapshot_signature(Some(&v));
        prop_assert_eq!(sig.len(), 64);
        prop_assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn p3_signature_invariant_under_object_key_order(
        pairs in arb_kv_pairs(),
        seed in any::<u64>(),
    ) {
        let forward = object_from_pairs(&pairs);
        let mut reversed = pairs.clone();
        reversed.reverse();
        let backward = object_from_pairs(&reversed);
        let mut shuffled = pairs.clone();
        let len = shuffled.len();
        if len > 1 {
            let mut state = seed;
            for i in (1..len).rev() {
                state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                let j = (state >> 33) as usize % (i + 1);
                shuffled.swap(i, j);
            }
        }
        let mixed = object_from_pairs(&shuffled);

        let sig_forward = build_snapshot_signature(Some(&forward));
        let sig_backward = build_snapshot_signature(Some(&backward));
        let sig_mixed = build_snapshot_signature(Some(&mixed));

        prop_assert_eq!(&sig_forward, &sig_backward);
        prop_assert_eq!(sig_forward, sig_mixed);
    }

    #[test]
    fn p4_signature_excludes_run_timestamp(
        pairs in arb_kv_pairs().prop_filter(
            "exclude run_timestamp key",
            |p| p.iter().all(|(k, _)| k != "run_timestamp"),
        ),
        ts_a in "[a-zA-Z0-9:_-]{0,40}",
        ts_b in "[a-zA-Z0-9:_-]{0,40}",
    ) {
        let base = object_from_pairs(&pairs);
        let mut with_a = base.as_object().unwrap().clone();
        with_a.insert("run_timestamp".to_string(), Value::String(ts_a));
        let mut with_b = base.as_object().unwrap().clone();
        with_b.insert("run_timestamp".to_string(), Value::String(ts_b));

        let sig_base = build_snapshot_signature(Some(&base));
        let sig_a = build_snapshot_signature(Some(&Value::Object(with_a)));
        let sig_b = build_snapshot_signature(Some(&Value::Object(with_b)));

        prop_assert_eq!(&sig_base, &sig_a);
        prop_assert_eq!(sig_a, sig_b);
    }

    #[test]
    fn p5_atomic_write_bytes_round_trip(data in prop::collection::vec(any::<u8>(), 0..4096)) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("p5.bin");
        atomic_write_bytes(&path, &data).unwrap();
        let read_back = std::fs::read(&path).unwrap();
        prop_assert_eq!(read_back, data);
    }

    #[test]
    fn p6_atomic_write_bytes_overwrite_last_write_wins(
        first in prop::collection::vec(any::<u8>(), 0..2048),
        second in prop::collection::vec(any::<u8>(), 0..2048),
    ) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("p6.bin");
        atomic_write_bytes(&path, &first).unwrap();
        atomic_write_bytes(&path, &second).unwrap();
        let read_back = std::fs::read(&path).unwrap();
        prop_assert_eq!(read_back, second);
    }

    #[test]
    fn p7_atomic_write_text_round_trip(text in "[\\x20-\\x7e\\n\\r\\t]{0,2048}") {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("p7.txt");
        atomic_write_text(&path, &text).unwrap();
        let read_back = std::fs::read_to_string(&path).unwrap();
        prop_assert_eq!(read_back, text);
    }

    #[test]
    fn p8_lock_acquire_release_cycles_are_exclusive(
        run_ids in prop::collection::vec("[a-z0-9]{1,8}", 1..6),
    ) {
        let dir = TempDir::new().unwrap();
        for run_id in &run_ids {
            let lock = acquire(
                dir.path(),
                run_id,
                DEFAULT_LOCK_TTL,
                false,
                DEFAULT_LOCK_FILENAME,
            ).unwrap();
            prop_assert_eq!(lock.metadata().run_id.as_str(), run_id.as_str());
            prop_assert!(lock.path().exists());

            let contended = acquire(
                dir.path(),
                "other",
                DEFAULT_LOCK_TTL,
                false,
                DEFAULT_LOCK_FILENAME,
            );
            let is_lock_failed = matches!(contended, Err(PersistenceError::LockFailed { .. }));
            prop_assert!(is_lock_failed);
            drop(lock);
        }

        prop_assert!(!lock_path(dir.path(), DEFAULT_LOCK_FILENAME).exists());
    }

    #[test]
    fn p9_signature_changes_when_non_timestamp_value_changes(
        key in "[a-z]{1,4}",
        a in -1_000_000i64..1_000_000i64,
        delta in 1i64..1_000_000i64,
    ) {
        prop_assume!(key != "run_timestamp");
        let v1 = serde_json::json!({ key.as_str(): a });
        let v2 = serde_json::json!({ key.as_str(): a.wrapping_add(delta) });
        prop_assert_ne!(
            build_snapshot_signature(Some(&v1)),
            build_snapshot_signature(Some(&v2))
        );
    }

    #[test]
    fn p10_atomic_write_then_signature_matches_original(v in arb_json_value()) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("snap.json");
        let serialized = serde_json::to_string(&v).unwrap();
        atomic_write_bytes(&path, serialized.as_bytes()).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&raw).unwrap();
        prop_assert_eq!(
            build_snapshot_signature(Some(&v)),
            build_snapshot_signature(Some(&parsed))
        );
    }
}

#[test]
fn lock_acquire_release_drops_file_smoke() {
    let dir = TempDir::new().unwrap();
    let path = {
        let lock = acquire(
            dir.path(),
            "smoke",
            Duration::from_mins(1),
            false,
            DEFAULT_LOCK_FILENAME,
        )
        .unwrap();
        lock.path().to_path_buf()
    };
    assert!(!path.exists());
}
