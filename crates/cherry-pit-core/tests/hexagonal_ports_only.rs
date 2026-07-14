//! Hexagonal ports-and-adapters invariant for `cherry-pit-core`.
//!
//! CHE-0004:R2: domain logic behind trait-based ports, infrastructure in
//! adapter crates. This is the ports crate; adapters live in siblings.
//! An adapter entering core's transitive closure must fail locally.
//!
//! Two structural assertions:
//!
//! 1. **No adapter crates in the transitive closure.** BFS
//!    `Cargo.lock` from `cherry-pit-core`, assert none of the named
//!    adapters appear — bans the closure *outbound* to adapters.
//! 2. **Port traits are publicly exposed.** A path probe (`use`
//!    statements plus a never-called function ascribing each trait as
//!    a trait object) fails to compile if re-exports are removed or
//!    renamed. Not reflection, not proc-macros.
//!
//! BFS is duplicated from `dep_tree.rs` (shared-helper deferred);
//! `cargo metadata` is deliberately not invoked — a spawned cargo
//! subprocess can deadlock the build lock.

use std::collections::{BTreeSet, VecDeque};

use serde::Deserialize;

/// Adapter crates that must NOT appear in `cherry-pit-core`'s transitive
/// closure. Each is an adapter to a specific infrastructure concern;
/// presence in core would invert the hexagonal-architecture dependency
/// direction (adapters depend on ports, never the reverse).
const ADAPTER_CRATES: &[&str] = &[
    "cherry-pit-storage",
    "cherry-pit-web",
    "cherry-pit-app",
    "cherry-pit-wq",
    "cherry-pit-projection",
    "cherry-pit-gateway",
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

/// Cargo.lock `dependencies` strings are `"name"`, `"name version"`, or
/// `"name version (source)"`; only the leading token is the crate name.
fn dep_name(raw: &str) -> &str {
    raw.split_whitespace().next().unwrap_or(raw)
}

/// CHE-0004:R2 — no adapter crate appears in core's transitive closure.
#[test]
fn hexagonal_ports_only() {
    let parsed: Lockfile = toml::from_str(LOCKFILE).expect("parse Cargo.lock");

    let mut deps_by_name: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for pkg in &parsed.package {
        let entry = deps_by_name.entry(pkg.name.as_str()).or_default();
        for d in &pkg.dependencies {
            entry.push(dep_name(d));
        }
    }

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

    let violations: Vec<&str> = ADAPTER_CRATES
        .iter()
        .copied()
        .filter(|a| visited.contains(a))
        .collect();

    assert!(
        violations.is_empty(),
        "cherry-pit-core transitive closure contains adapter crates: \
         {violations:?}. CHE-0004:R2 requires adapters to depend on the \
         core ports, never the reverse. Closure size: {} crates.",
        visited.len()
    );
}

/// CHE-0004:R2 — port traits are publicly re-exported from `cherry_pit_core`.
///
/// Compile-time path probe. The generic functions below each carry a
/// trait bound naming a port trait by its public path. Bounds in fn
/// signatures are resolved at item-definition time, so a removed or
/// renamed `pub use` breaks compilation of this file — surfacing the
/// regression at `cargo test` time, not at downstream adapter
/// integration. The functions are never called.
///
/// NOT reflection. NOT proc-macros. Pure type-system.
#[test]
fn port_traits_are_public() {}

#[expect(
    dead_code,
    reason = "CHE-0028 probe: Aggregate port trait is publicly re-exported"
)]
fn probe_aggregate<T: cherry_pit_core::Aggregate>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: HandleCommand<C> port trait is publicly re-exported"
)]
fn probe_handle_command<A, C>()
where
    A: cherry_pit_core::Aggregate + cherry_pit_core::HandleCommand<C>,
    C: cherry_pit_core::Command,
{
}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: DomainEvent port trait is publicly re-exported"
)]
fn probe_domain_event<E: cherry_pit_core::DomainEvent>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: Command port trait is publicly re-exported"
)]
fn probe_command<C: cherry_pit_core::Command>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: Policy port trait is publicly re-exported"
)]
fn probe_policy<P: cherry_pit_core::Policy>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: Projection port trait is publicly re-exported"
)]
fn probe_projection<P: cherry_pit_core::Projection>() {}

#[expect(
    dead_code,
    reason = "CHE-0028 probe: CommandBus port trait is publicly re-exported"
)]
fn probe_command_bus<B: cherry_pit_core::CommandBus>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: CommandGateway port trait is publicly re-exported"
)]
fn probe_command_gateway<G: cherry_pit_core::CommandGateway>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: EventBus port trait is publicly re-exported"
)]
fn probe_event_bus<B: cherry_pit_core::EventBus>() {}
#[expect(
    dead_code,
    reason = "CHE-0028 probe: EventStore port trait is publicly re-exported"
)]
fn probe_event_store<S: cherry_pit_core::EventStore>() {}
