//! Static analysis tests verifying CHE ADR obligations on cherry-pit-core.
//!
//! These tests verify structural and boundary invariants that are
//! difficult to enforce via the type system alone.

/// M1: `#![forbid(unsafe_code)]` at crate root (CHE-0007 R1).
#[test]
fn m1_forbid_unsafe_code() {
    let lib_rs = include_str!("../src/lib.rs");
    assert!(
        lib_rs.contains("#![forbid(unsafe_code)]"),
        "lib.rs must contain #![forbid(unsafe_code)] per CHE-0007"
    );
}

/// M3: `cherry-pit-core` dependencies restricted to {serde, uuid, jiff}
/// (CHE-0029 R4). Pardosa-encoding has been removed from this workspace
/// (mission pardosa-deletion-1779100000); ADR cleanup deferred per user
/// mission scope. Also covers M23 (no thiserror). The CHE-0029 R6
/// closure check (forbidden transitives: tokio/axum/async-nats/tracing)
/// is orthogonal and verified by `dep_tree.rs`.
#[test]
fn m3_dependency_allowlist() {
    let cargo_toml = include_str!("../Cargo.toml");

    // Parse the [dependencies] section — everything between [dependencies]
    // and the next [section] or EOF.
    let deps_start = cargo_toml
        .find("[dependencies]")
        .expect("Cargo.toml must have [dependencies]");
    let deps_section = &cargo_toml[deps_start + "[dependencies]".len()..];
    let deps_end = deps_section.find("\n[").unwrap_or(deps_section.len());
    let deps = &deps_section[..deps_end];

    let allowed = ["serde", "uuid", "jiff"];
    for line in deps.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Extract crate name (before '=' or '{').
        let name = trimmed
            .split(|c: char| c == '=' || c == '{' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            continue;
        }
        assert!(
            allowed.contains(&name),
            "Unexpected dependency '{name}' in cherry-pit-core. \
             Allowed: {allowed:?} per CHE-0029 R4."
        );
    }
}

/// M29: Public API is flat — no `pub mod` in lib.rs (CHE-0030 R1/R2),
/// except the CHE-0058 carve-out: a `pub mod` declaration is permitted
/// when immediately preceded by a `#[cfg(...)]` attribute whose
/// predicate names `test` or `feature = "testing"`.
#[test]
fn m29_no_pub_mod_in_lib() {
    let lib_rs = include_str!("../src/lib.rs");
    let lines: Vec<&str> = lib_rs.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Skip doc comments and regular comments.
        if trimmed.starts_with("//") {
            continue;
        }
        if !trimmed.starts_with("pub mod") {
            continue;
        }
        // CHE-0058 carve-out: walk back past blank lines / comments to
        // the previous meaningful line. If it is a `#[cfg(...)]`
        // attribute whose predicate contains either `test` or
        // `feature = "testing"`, the `pub mod` is permitted.
        let mut j = i;
        let prev_meaningful = loop {
            if j == 0 {
                break None;
            }
            j -= 1;
            let prev = lines[j].trim();
            if prev.is_empty() || prev.starts_with("//") {
                continue;
            }
            break Some(prev);
        };
        let permitted = match prev_meaningful {
            Some(prev) => {
                prev.starts_with("#[cfg(")
                    && (prev.contains("test") || prev.contains("feature = \"testing\""))
            }
            None => false,
        };
        assert!(
            permitted,
            "lib.rs line {} contains `pub mod` — CHE-0030 forbids public modules. \
             CHE-0058 carve-out requires preceding `#[cfg(...)]` naming `test` or \
             `feature = \"testing\"`. Line: {trimmed}",
            i + 1
        );
    }
}

/// M29b: CHE-0058 carve-out detection — synthetic smoke test exercising
/// both permitted (gated) and forbidden (bare) shapes against the same
/// detection logic as M29. Keeps M29 honest if `lib.rs` happens to have
/// zero `pub mod` declarations.
#[test]
fn m29b_carve_out_logic() {
    // Inline copy of the M29 detection logic so we can drive it with
    // synthetic input rather than the real lib.rs.
    fn check(src: &str) -> Result<(), String> {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || !trimmed.starts_with("pub mod") {
                continue;
            }
            let mut j = i;
            let prev_meaningful = loop {
                if j == 0 {
                    break None;
                }
                j -= 1;
                let prev = lines[j].trim();
                if prev.is_empty() || prev.starts_with("//") {
                    continue;
                }
                break Some(prev);
            };
            let permitted = match prev_meaningful {
                Some(prev) => {
                    prev.starts_with("#[cfg(")
                        && (prev.contains("test") || prev.contains("feature = \"testing\""))
                }
                None => false,
            };
            if !permitted {
                return Err(format!("line {}: {trimmed}", i + 1));
            }
        }
        Ok(())
    }

    // Bare `pub mod` — forbidden.
    assert!(check("pub mod foo;").is_err());
    // Gated by `cfg(test)` — permitted.
    assert!(check("#[cfg(test)]\npub mod testing;").is_ok());
    // Gated by `cfg(feature = \"testing\")` — permitted.
    assert!(check("#[cfg(feature = \"testing\")]\npub mod testing;").is_ok());
    // Gated by `cfg(any(test, feature = \"testing\"))` — permitted.
    assert!(check("#[cfg(any(test, feature = \"testing\"))]\npub mod testing;").is_ok());
    // Gated by an unrelated cfg (e.g. `target_os`) — still forbidden.
    assert!(check("#[cfg(target_os = \"linux\")]\npub mod foo;").is_err());
    // Blank line / comment between cfg and pub mod — still permitted.
    assert!(check("#[cfg(test)]\n\n// comment\npub mod testing;").is_ok());
}

/// M20: `AggregateNotFound` is a `DispatchError` variant, not `StoreError`
/// (CHE-0019 R2).
#[test]
fn m20_aggregate_not_found_on_dispatch_error() {
    use cherry_pit_core::{AggregateId, DispatchError};
    use std::num::NonZeroU64;

    let id = AggregateId::new(NonZeroU64::new(1).unwrap());
    let err: DispatchError<std::io::Error> = DispatchError::AggregateNotFound { aggregate_id: id };
    // Pattern match succeeds — variant exists on DispatchError.
    assert!(matches!(err, DispatchError::AggregateNotFound { .. }));

    // StoreError has no AggregateNotFound variant — verified by absence
    // in the source (no runtime test possible for variant absence without
    // trybuild, but the compile-time shape is authoritative).
}

/// M22: `ErrorCategory` exposed on `DispatchError`, `StoreError`,
/// `BusError`, `EnvelopeError` (CHE-0021 R3).
#[test]
fn m22_error_category_on_all_error_types() {
    use cherry_pit_core::{
        AggregateId, BusError, DispatchError, EnvelopeError, ErrorCategory, StoreError,
    };
    use std::num::NonZeroU64;

    let id = AggregateId::new(NonZeroU64::new(1).unwrap());

    // DispatchError
    let de: DispatchError<std::io::Error> = DispatchError::AggregateNotFound { aggregate_id: id };
    assert_eq!(de.category(), ErrorCategory::Terminal);

    // StoreError
    let se = StoreError::ConcurrencyConflict {
        aggregate_id: id,
        expected_sequence: NonZeroU64::new(1).unwrap(),
        actual_sequence: 2,
    };
    assert_eq!(se.category(), ErrorCategory::Retryable);

    // BusError
    let be = BusError::new("test");
    assert_eq!(be.category(), ErrorCategory::Retryable);

    // EnvelopeError
    let ee = EnvelopeError::NilEventId;
    assert_eq!(ee.category(), ErrorCategory::Terminal);
}
