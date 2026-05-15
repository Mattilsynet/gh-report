//! CHE-0022 append-only-forever falsifier for `gh_report::domain::events::DomainEvent`.
//!
//! Pins the current `(variant_name, sorted_field_names)` set as a **superset
//! floor**: future revisions of `DomainEvent` must be a superset of the
//! snapshot. CHE-0022 binds the contract:
//!
//!   - R1: new variants allowed (additive) — falsifier *passes* and reports
//!     the addition so the maintainer can update the snapshot.
//!   - R2: removing or renaming a persisted variant is forbidden — falsifier
//!     *fails* with a CHE-0022-violation message.
//!   - R3: new fields on existing variants must be `#[serde(default)]`. From
//!     the name-level perspective tested here, additive fields are simply
//!     a superset of the snapshot for that variant — *pass* with update
//!     instructions. The `repo_evaluated_pre_b6_json_deserializes_with_default_evidence`
//!     unit test in `src/domain/events.rs` covers the on-wire semantics.
//!
//! ## Why this test lives in `gh-report` and not a CHE crate
//!
//! `cherry-pit-core` is trait-only by CHE-0029 (workspace DAG) and CHE-0030
//! (flat public API) — it re-exports the `Aggregate` / `HandleCommand`
//! traits but no concrete event enum. There is nothing concrete to pin at
//! the CHE layer today. `gh-report` is the first downstream crate to carry
//! a concrete `DomainEvent`, so the falsifier rides with the concrete
//! surface. When row-14's `FakeBus` injection eventually enables generic
//! CHE-crate event tests, the *trait-level* obligations stay in CHE and
//! this *concrete-schema* falsifier remains here — they cover different
//! axes and should not be merged.
//!
//! ## Enumeration mechanism
//!
//! Hand-maintained `CURRENT_VARIANTS` constant mirroring the actual enum,
//! paired with the on-disk snapshot. Same dual-source-of-truth pattern as
//! cherry-pit-core's `aggregate_default_zero_state.rs` (sub-03 precedent).
//! No proc-macro, no reflection crate, no new deps — CHE-0022 evolution is
//! a per-event-bus concern, not a workspace-wide reflection problem.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

/// Current variants of `gh_report::domain::events::DomainEvent`, with the
/// field name list per variant sorted ascending. Hand-maintained; must be
/// updated alongside any change to the enum in `src/domain/events.rs`.
///
/// CHE-0022 binds every entry. When adding a variant or field here, append
/// the corresponding line(s) to `tests/fixtures/event_schema_snapshot.txt`
/// in the same form and cite CHE-0022 in the commit message.
const CURRENT_VARIANTS: &[(&str, &[&str])] = &[
    (
        "SweepStarted",
        &["batch_id", "org", "repo_count", "timestamp"],
    ),
    (
        "RepoEvaluated",
        &[
            "domain_key",
            "duration_ms",
            "evidence",
            "repo_name",
            "source",
            "success",
            "timestamp",
        ],
    ),
    ("RepoRemoved", &["domain_key", "repo_name", "timestamp"]),
    (
        "SweepCompleted",
        &["batch_id", "duration_ms", "repo_count", "timestamp"],
    ),
    ("WebhookReceived", &["action", "repo", "timestamp"]),
    (
        "EvidencePublished",
        &["page_count", "timestamp", "warm_start"],
    ),
    (
        "SweepFailed",
        &["batch_id", "duration_ms", "error", "timestamp"],
    ),
    (
        "SweepProgress",
        &["batch_id", "completed", "timestamp", "total"],
    ),
];

type Record = (String, Vec<String>);

fn parse_snapshot(raw: &str) -> BTreeSet<Record> {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|line| {
            let (name, fields) = line.split_once(':').unwrap_or_else(|| {
                panic!("malformed snapshot line (expected `Name: f1,f2`): {line}")
            });
            let name = name.trim().to_owned();
            let fields: Vec<String> = fields
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect();
            (name, fields)
        })
        .collect()
}

fn current_records() -> BTreeSet<Record> {
    CURRENT_VARIANTS
        .iter()
        .map(|(name, fields)| {
            let mut fs: Vec<String> = fields.iter().map(|s| (*s).to_owned()).collect();
            fs.sort();
            ((*name).to_owned(), fs)
        })
        .collect()
}

fn render_record(r: &Record) -> String {
    format!("{}: {}", r.0, r.1.join(", "))
}

#[test]
fn event_schema_is_append_only() {
    let snapshot_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("event_schema_snapshot.txt");
    let raw = fs::read_to_string(&snapshot_path)
        .unwrap_or_else(|e| panic!("snapshot {} unreadable: {e}", snapshot_path.display()));

    let snapshot = parse_snapshot(&raw);
    let current = current_records();

    // CHE-0022:R2 — removal or rename of a snapshot record is forbidden.
    let missing: Vec<&Record> = snapshot.difference(&current).collect();
    if !missing.is_empty() {
        // Distinguish "variant entirely missing" (rename or removal at the
        // variant level) from "variant present but field signature changed"
        // (rename or removal at the field level).
        let current_names: BTreeSet<&str> = current.iter().map(|(n, _)| n.as_str()).collect();
        let mut details = String::new();
        for rec in &missing {
            let fields = rec.1.join(", ");
            if current_names.contains(rec.0.as_str()) {
                // `write!` into String is infallible — `Write for String` never errors.
                let _ = writeln!(
                    details,
                    "  - field signature changed for `{}` — snapshot had [{fields}]",
                    rec.0,
                );
            } else {
                let _ = writeln!(
                    details,
                    "  - variant `{}` removed or renamed (was: [{fields}])",
                    rec.0,
                );
            }
        }
        panic!(
            "CHE-0022 violation (R2: removing or renaming persisted event variants is forbidden).\n\
             The following snapshot record(s) are missing from the current enum:\n\
             {details}\n\
             If this is intentional, you are making a breaking schema change — bump the\n\
             schema version and migrate persisted events. Do NOT silently update the\n\
             snapshot at {} to make this pass.",
            snapshot_path.display()
        );
    }

    // CHE-0022:R1/R3 — additive change detected. Pass with directional
    // instructions so the maintainer updates the snapshot in a follow-up
    // commit (or the same commit, citing CHE-0022).
    let added: Vec<&Record> = current.difference(&snapshot).collect();
    if !added.is_empty() {
        let lines: Vec<String> = added.iter().map(|r| render_record(r)).collect();
        eprintln!(
            "additive change detected; update snapshot at {} and re-run.\n\
             Append the following line(s) (CHE-0022:R1 new variant / R3 new field):\n\
             {}",
            snapshot_path.display(),
            lines.join("\n")
        );
    }
}
