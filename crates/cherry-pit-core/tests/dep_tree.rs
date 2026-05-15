//! Transitive-dependency gate for `cherry-pit-core`.
//!
//! Enforces CHE-0029:R6 and CHE-0018:R3: the foundational traits crate
//! must remain leaf with respect to async runtimes. A future contributor
//! who adds an async-runtime crate directly or transitively must fail CI.
//!
//! Approach: parse the workspace `Cargo.lock` at compile time, BFS the
//! transitive closure from `cherry-pit-core`, and assert the closure
//! contains no banned crate name.
//!
//! `cargo metadata` is intentionally NOT invoked from inside this test:
//! a cargo subprocess spawned from a cargo-spawned test process can
//! deadlock the build-graph file lock.

use std::collections::{BTreeSet, VecDeque};

use serde::Deserialize;

/// Crates banned from `cherry-pit-core`'s transitive closure.
///
/// Match by exact `name` field in `Cargo.lock` — canonical package names.
const BANNED: &[&str] = &[
    "tokio",
    "axum",
    "async-nats",
    "tracing",
    "async-trait",
    "smol",
    "async-std",
    "actix",
    "actix-rt",
];

const LOCKFILE: &str = include_str!("../../../Cargo.lock");

#[derive(Deserialize)]
struct Lockfile {
    package: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

/// Parse a `dependencies` entry's package name.
///
/// Cargo.lock dependency strings have the form `"name"`, `"name version"`,
/// or `"name version (source)"`. We only need the leading crate name.
fn dep_name(raw: &str) -> &str {
    raw.split_whitespace().next().unwrap_or(raw)
}

#[test]
fn no_async_runtime_in_transitive_closure() {
    let parsed: Lockfile = toml::from_str(LOCKFILE).expect("parse Cargo.lock");

    // Index packages by name. Cargo.lock may contain multiple versions of
    // the same crate (rare in this workspace); group their dep lists.
    let mut deps_by_name: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for pkg in &parsed.package {
        let entry = deps_by_name.entry(pkg.name.as_str()).or_default();
        for d in &pkg.dependencies {
            entry.push(dep_name(d));
        }
    }

    // BFS from cherry-pit-core.
    let root = "cherry-pit-core";
    assert!(
        deps_by_name.contains_key(root),
        "{root} not found in Cargo.lock — workspace layout drifted"
    );

    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    queue.push_back(root);
    visited.insert(root);

    while let Some(cur) = queue.pop_front() {
        if let Some(deps) = deps_by_name.get(cur) {
            for &d in deps {
                if visited.insert(d) {
                    queue.push_back(d);
                }
            }
        }
    }

    let violations: Vec<&str> = BANNED
        .iter()
        .copied()
        .filter(|b| visited.contains(b))
        .collect();

    assert!(
        violations.is_empty(),
        "cherry-pit-core transitive closure contains banned async-runtime \
         crates: {violations:?}. CHE-0029:R6 requires cherry-pit-core to \
         remain runtime-agnostic. Closure size: {} crates.",
        visited.len()
    );
}
