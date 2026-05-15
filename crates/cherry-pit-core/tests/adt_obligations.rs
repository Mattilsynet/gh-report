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
/// (CHE-0029 R4). Also covers M23 (no thiserror).
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

/// M29: Public API is flat — no `pub mod` in lib.rs (CHE-0030 R1/R2).
#[test]
fn m29_no_pub_mod_in_lib() {
    let lib_rs = include_str!("../src/lib.rs");
    for (i, line) in lib_rs.lines().enumerate() {
        let trimmed = line.trim();
        // Skip doc comments and regular comments.
        if trimmed.starts_with("//") {
            continue;
        }
        assert!(
            !trimmed.starts_with("pub mod"),
            "lib.rs line {} contains `pub mod` — CHE-0030 forbids public modules. \
             Use `mod` + `pub use` re-exports instead. Line: {trimmed}",
            i + 1
        );
    }
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
