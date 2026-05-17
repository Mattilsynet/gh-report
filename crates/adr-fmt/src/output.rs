//! Unified output formatter — Alternative 4 markdown format.
//!
//! All modes emit concatenated markdown with structured header blocks
//! using `◆`/`◇` markers and `---` separators.
//! Optimized for LLM token efficiency.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::config::Config;
use crate::model::{AdrId, AdrRecord, DomainDir, RelVerb, Status};
use crate::nav::{compute_parent_children, compute_parent_edges};
use crate::refs::RefsReport;
use crate::report::Diagnostic;

/// Read-only context shared across recursive tree-rendering calls.
struct TreeContext<'a> {
    parent_children: &'a HashMap<AdrId, Vec<AdrId>>,
    record_by_id: &'a HashMap<&'a AdrId, &'a AdrRecord>,
    domain_prefix: &'a str,
}

// ── Output block types ─────────────────────────────────────────────

/// A group of rules emitted under a single root ADR in `--context` mode.
#[derive(Debug)]
pub struct RootGroup {
    pub root_id: AdrId,
    pub root_title: String,
    pub rules: Vec<EmittedRule>,
}

/// A single rule positioned in root-grouped context output.
#[derive(Debug)]
pub struct EmittedRule {
    pub adr_id: AdrId,
    pub rule_id: String,
    pub text: String,
    pub layer: u8,
    pub depth: u16,
}

// ── Refs rendering (--refs mode) ───────────────────────────────────

/// Render a `--refs` report as a compact markdown bullet list.
///
/// Output shape:
///
/// ```text
/// ## ◆ REFS: TARGET-ID | Tier: T | Status: S | <title>
///
/// - SRC-ID [Verb] | Tier: T | Status: S | <title>
/// - ...
/// ```
///
/// Empty reports emit `No references found.` after the header.
#[must_use]
pub fn render_refs(report: &RefsReport) -> String {
    let mut out = String::new();
    let target_tier = report
        .target_tier
        .map_or_else(|| "?".into(), |t| format!("{t}"));
    let target_status = render_status(report.target_status.as_ref());
    let target_title = report.target_title.as_deref().unwrap_or("<no title>");
    writeln!(
        out,
        "## ◆ REFS: {} | Tier: {target_tier} | Status: {target_status} | {target_title}",
        report.target_id
    )
    .unwrap();
    out.push('\n');

    if report.refs.is_empty() {
        writeln!(out, "No references found.").unwrap();
        return out;
    }

    for entry in &report.refs {
        let tier = entry
            .source_tier
            .map_or_else(|| "?".into(), |t| format!("{t}"));
        let status = render_status(entry.source_status.as_ref());
        let title = entry.source_title.as_deref().unwrap_or("<no title>");
        writeln!(
            out,
            "- {} [{}] | Tier: {tier} | Status: {status} | {title}",
            entry.source_id, entry.verb
        )
        .unwrap();
    }

    out
}

/// Render a `Status` for the refs view, preserving the full
/// `Superseded by X` payload (per AFM-0021 R2).
fn render_status(status: Option<&Status>) -> String {
    status.map_or_else(|| "?".into(), Status::short_display)
}

// ── Diagnostic rendering ───────────────────────────────────────────

/// Render diagnostics as Alternative 4 markdown blocks to stdout.
#[must_use]
pub fn render_diagnostics(diagnostics: &[Diagnostic], record_count: usize) -> String {
    let mut out = String::new();

    let mut warnings = 0u32;

    for d in diagnostics {
        if d.internal {
            continue;
        }
        match d.severity {
            crate::report::Severity::Warning => warnings += 1,
        }

        let location = if d.line > 0 {
            format!("{}:{}", d.file, d.line)
        } else {
            d.file.clone()
        };

        writeln!(
            out,
            "- **{}[{}]** {}: {}",
            d.severity, d.rule, location, d.message
        )
        .unwrap();
    }

    if out.is_empty() {
        writeln!(
            out,
            "## Diagnostics: 0 warning(s) across {record_count} ADR(s)"
        )
        .unwrap();
    } else {
        let header =
            format!("## Diagnostics: {warnings} warning(s) across {record_count} ADR(s)\n\n");
        out.insert_str(0, &header);
    }

    out
}

// ── Tree rendering (--tree mode) ───────────────────────────────────

/// Render root-grouped context output with preamble.
///
/// Rules are grouped by root ADR subtree. Each root with rules gets a
/// `### ROOT-ID. Title` heading. Rule lines use `- {text} [{ADR_ID}:{RULE_ID}:L{layer}]`
/// format with the anchoring ID at the end.
///
/// Groups with no rules after dedup are skipped. An optional "Unclaimed Rules"
/// section appears if any eligible rules were not reached by any root's BFS.
#[must_use]
pub fn render_root_groups(crate_name: &str, groups: &[RootGroup]) -> String {
    let mut out = String::new();

    // Preamble
    writeln!(out, "# Architecture Rules").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "These rules are mandatory constraints for all code in crate `{crate_name}`."
    )
    .unwrap();
    writeln!(out, "Follow every rule without exception.").unwrap();

    for group in groups {
        if group.rules.is_empty() {
            continue;
        }

        writeln!(out).unwrap();
        writeln!(out, "### {}. {}", group.root_id, group.root_title).unwrap();

        for rule in &group.rules {
            writeln!(
                out,
                "- {} [{}:{}:L{}]",
                rule.text, rule.adr_id, rule.rule_id, rule.layer
            )
            .unwrap();
        }
    }

    out
}

// ── Tree rendering (--tree mode, domain overview) ──────────────────

/// Render the domain tree with box-drawing to stdout.
///
/// For each domain (filtered by `domain_filter` if set), renders the
/// parent-edge tree(s) rooted at each Root-marked ADR in that domain.
/// Children are determined by `compute_parent_children` and restricted
/// to same-domain ADRs (cross-domain children appear in their own
/// domain's tree). Stale ADRs are excluded from rendering but counted.
///
/// Each ADR line shows: `<glyphs> ID Title [Tier] STATUS [also: X, Y]`
/// where `also: …` lists forward citations other than the structural
/// parent (Supersedes/Refines/etc.).
///
/// Per-domain orphan section lists ADRs in the domain that are not
/// reachable from any root via parent-edge traversal (cycles or
/// missing parent). These are rendered flat after the tree(s).
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "single-pass tree renderer; function is sequential branch-rendering whose split would scatter the indent/connector state across helpers without simplifying any branch"
)]
pub fn render_tree(
    records: &[AdrRecord],
    domain_dirs: &[DomainDir],
    config: &Config,
    domain_filter: Option<&str>,
) -> String {
    let mut out = String::new();

    // Build parent-edge projection across full corpus
    let parent_edges = compute_parent_edges(records);
    let parent_children = compute_parent_children(records);

    // Filter domains. GND is rendered last (foundational stack
    // underneath all other domain blocks); other domains keep their
    // TOML-declared order.
    let mut dirs: Vec<&DomainDir> = if let Some(filter) = domain_filter {
        domain_dirs.iter().filter(|d| d.prefix == filter).collect()
    } else {
        domain_dirs.iter().collect()
    };
    dirs.sort_by_key(|d| u32::from(d.prefix == "GND"));

    if dirs.is_empty() {
        if let Some(f) = domain_filter {
            writeln!(out, "No domain found matching '{f}'").unwrap();
        }
        return out;
    }

    // Group non-stale records by domain
    let mut by_prefix: HashMap<&str, Vec<&AdrRecord>> = HashMap::new();
    for record in records {
        if !record.is_stale {
            by_prefix.entry(&record.id.prefix).or_default().push(record);
        }
    }

    // Lookup table by ID for title/tier/status access during walk
    let record_by_id: HashMap<&AdrId, &AdrRecord> = records.iter().map(|r| (&r.id, r)).collect();

    for dir in &dirs {
        let domain_name = &dir.name;
        let foundation = config
            .domains
            .iter()
            .find(|d| d.prefix == dir.prefix)
            .is_some_and(|d| d.foundation);
        let foundation_marker = if foundation { " [foundation]" } else { "" };

        writeln!(
            out,
            "## {} ({}){foundation_marker}",
            domain_name, dir.prefix
        )
        .unwrap();

        let domain_records = by_prefix
            .get(dir.prefix.as_str())
            .cloned()
            .unwrap_or_default();

        // Find roots in this domain (sorted by ADR number)
        let mut roots: Vec<&AdrRecord> = domain_records
            .iter()
            .copied()
            .filter(|r| r.is_root())
            .collect();
        roots.sort_by_key(|r| r.id.number);

        // Track which domain ADRs are reached via tree traversal
        let mut reached: std::collections::HashSet<AdrId> = std::collections::HashSet::new();
        let ctx = TreeContext {
            parent_children: &parent_children,
            record_by_id: &record_by_id,
            domain_prefix: &dir.prefix,
        };

        for root in &roots {
            render_tree_node(
                &mut out,
                &root.id,
                &ctx,
                &mut reached,
                &mut Vec::new(),
                true,
            );
        }

        // Cross-domain forest roots: any non-root domain ADR whose
        // `Parent-cross-domain:` field validates against its first
        // `References:` target. These are rendered at top level of
        // their own domain, annotated with `↑ <PARENT-ID>` so the
        // cross-domain edge is visible without breaking the
        // domain-grouped layout. They also seed sub-trees: any
        // same-domain ADRs that parent through them descend below.
        let mut cross_domain_roots: Vec<&&AdrRecord> = domain_records
            .iter()
            .filter(|r| !r.is_root() && !reached.contains(&r.id))
            .filter(|r| validated_cross_domain_parent(r).is_some())
            .collect();
        cross_domain_roots.sort_by_key(|r| r.id.number);

        for record in &cross_domain_roots {
            // Invariant established by the `validated_cross_domain_parent(r).is_some()`
            // filter at line 299. Match locally rather than `.expect` so a future
            // refactor that moves or weakens the filter cannot panic at runtime —
            // the worst case becomes a silently-skipped record, surfaced by
            // existing orphan diagnostics below.
            let Some(cross_parent) = validated_cross_domain_parent(record) else {
                continue;
            };
            render_cross_domain_tree_node(&mut out, &record.id, &cross_parent, &ctx, &mut reached);
        }

        // Orphan section: domain ADRs not reached from any root. We
        // distinguish three subcategories so readers know whether the
        // root cause is a missing References, a cycle, or a chain that
        // terminates at a non-root mid-tier ADR.
        let orphans: Vec<&&AdrRecord> = domain_records
            .iter()
            .filter(|r| !reached.contains(&r.id))
            .collect();

        if !orphans.is_empty() {
            let mut sorted_orphans: Vec<&&AdrRecord> = orphans.into_iter().collect();
            sorted_orphans.sort_by_key(|r| r.id.number);
            writeln!(out, "  (orphans — not reachable from any root)").unwrap();
            for record in &sorted_orphans {
                let title = record.title.as_deref().unwrap_or("(untitled)");
                let tier = record.tier.map_or_else(|| "?".into(), |t| format!("{t}"));
                let status = record
                    .status
                    .as_ref()
                    .map_or_else(|| "?".into(), super::model::Status::short_display);
                let also = format_also_references(record, &parent_edges);

                // Categorize:
                //   - parent edge present → walk it (cycle vs non-root)
                //   - no parent edge → missing first References
                let reason = if parent_edges.contains_key(&record.id) {
                    match crate::nav::walk_parent_chain(&record.id, &parent_edges) {
                        Ok(_) => " (chain ends at non-root)",
                        Err(_) => " (cycle)",
                    }
                } else {
                    " (no References — parent missing)"
                };

                writeln!(
                    out,
                    "  {} {title} [{tier}] {status}{reason}{also}",
                    record.id
                )
                .unwrap();
            }
        }

        // Stale count for this domain
        let stale_count = records
            .iter()
            .filter(|r| r.is_stale && r.id.prefix == dir.prefix)
            .count();
        if stale_count > 0 {
            writeln!(out, "  ({stale_count} stale)").unwrap();
        }

        out.push('\n');
    }

    out
}

/// Recursively render a tree node and its same-domain children.
///
/// `prefix_stack` carries the per-level indent state: for each ancestor
/// level, `true` means "more siblings remain at this level" (use `│  `),
/// `false` means "last sibling" (use `   `). The current node's own
/// connector is `├─ ` if not last, `└─ ` if last.
fn render_tree_node(
    out: &mut String,
    id: &AdrId,
    ctx: &TreeContext<'_>,
    reached: &mut std::collections::HashSet<AdrId>,
    prefix_stack: &mut Vec<bool>,
    is_last: bool,
) {
    // Cycle guard: do not re-emit
    if !reached.insert(id.clone()) {
        return;
    }

    let record = match ctx.record_by_id.get(id) {
        Some(r) => *r,
        None => return,
    };

    // Build indent string from prefix_stack
    let mut indent = String::from("  ");
    for &more in prefix_stack.iter() {
        indent.push_str(if more { "│  " } else { "   " });
    }
    let connector = if prefix_stack.is_empty() {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };

    let title = record.title.as_deref().unwrap_or("(untitled)");
    let tier = record.tier.map_or_else(|| "?".into(), |t| format!("{t}"));
    let status = record
        .status
        .as_ref()
        .map_or_else(|| "?".into(), super::model::Status::short_display);

    let also = format_also_references_full(record);

    writeln!(
        out,
        "{indent}{connector}{} {title} [{tier}] {status}{also}",
        record.id
    )
    .unwrap();

    // Walk same-domain children only (cross-domain children render in
    // their own domain's tree section)
    let children: Vec<AdrId> = ctx
        .parent_children
        .get(id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.prefix == ctx.domain_prefix)
        .collect();

    let n = children.len();
    for (i, child) in children.iter().enumerate() {
        let last = i + 1 == n;
        prefix_stack.push(!is_last);
        render_tree_node(out, child, ctx, reached, prefix_stack, last);
        prefix_stack.pop();
    }
}

/// Render a cross-domain-parented record as a top-level forest root
/// in its own domain, annotated with `↑ <PARENT-ID>` to make the
/// cross-domain parent edge visible. Same-domain children that
/// parent-edge through this record descend beneath it normally.
fn render_cross_domain_tree_node(
    out: &mut String,
    id: &AdrId,
    cross_parent: &AdrId,
    ctx: &TreeContext<'_>,
    reached: &mut std::collections::HashSet<AdrId>,
) {
    if !reached.insert(id.clone()) {
        return;
    }

    let Some(record) = ctx.record_by_id.get(id) else {
        return;
    };

    let title = record.title.as_deref().unwrap_or("(untitled)");
    let tier = record.tier.map_or_else(|| "?".into(), |t| format!("{t}"));
    let status = record
        .status
        .as_ref()
        .map_or_else(|| "?".into(), super::model::Status::short_display);

    // The first References target IS the cross-domain parent, so omit
    // it from `also` (it's surfaced by the ↑ annotation instead).
    let also = format_also_references_skipping_first_ref(record);

    writeln!(
        out,
        "  {} {title} [{tier}] {status} ↑ {cross_parent}{also}",
        record.id
    )
    .unwrap();

    // Descend into same-domain children.
    let children: Vec<AdrId> = ctx
        .parent_children
        .get(id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| c.prefix == ctx.domain_prefix)
        .collect();

    let n = children.len();
    let mut prefix_stack = vec![false]; // not at top level any more
    for (i, child) in children.iter().enumerate() {
        let last = i + 1 == n;
        render_tree_node(out, child, ctx, reached, &mut prefix_stack, last);
    }
}

/// Format the "also references" annotation for a cross-domain root,
/// skipping the first `References:` target (which is surfaced as the
/// `↑` cross-domain parent annotation, not as a peer "also" link).
fn format_also_references_skipping_first_ref(record: &AdrRecord) -> String {
    let mut first_ref_seen = false;
    let mut others: Vec<String> = Vec::new();
    for rel in &record.relationships {
        if rel.verb.is_reverse() {
            continue;
        }
        if rel.verb == RelVerb::Root && rel.target == record.id {
            continue;
        }
        if !first_ref_seen && rel.verb == RelVerb::References {
            first_ref_seen = true;
            continue;
        }
        others.push(format!("{} {}", rel.verb, rel.target));
    }
    if others.is_empty() {
        String::new()
    } else {
        format!(" [also: {}]", others.join(", "))
    }
}

/// Format the "also references" annotation using the parent-edge map
/// (used for orphan section where parent may be missing).
fn format_also_references(record: &AdrRecord, parent_edges: &HashMap<AdrId, AdrId>) -> String {
    let parent = parent_edges.get(&record.id);
    let mut others: Vec<String> = Vec::new();
    for rel in &record.relationships {
        if rel.verb.is_reverse() {
            continue;
        }
        if rel.verb == RelVerb::Root && rel.target == record.id {
            continue;
        }
        if Some(&rel.target) == parent {
            continue;
        }
        others.push(format!("{} {}", rel.verb, rel.target));
    }
    if others.is_empty() {
        String::new()
    } else {
        format!(" [also: {}]", others.join(", "))
    }
}

/// Format "also references" for in-tree node. The structural parent
/// is the first `References:` target (per `compute_parent_edges`);
/// everything else (Supersedes, Refines, additional References) is
/// listed as "also". Root self-reference is always excluded.
fn format_also_references_full(record: &AdrRecord) -> String {
    let mut parent_seen = false;
    let mut others: Vec<String> = Vec::new();
    for rel in &record.relationships {
        if rel.verb.is_reverse() {
            continue;
        }
        if rel.verb == RelVerb::Root && rel.target == record.id {
            continue;
        }
        if !parent_seen && rel.verb == RelVerb::References {
            parent_seen = true;
            continue;
        }
        others.push(format!("{} {}", rel.verb, rel.target));
    }
    if others.is_empty() {
        String::new()
    } else {
        format!(" [also: {}]", others.join(", "))
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Return the validated cross-domain parent ID for a record, or `None`.
///
/// The cross-domain parent is "validated" when:
/// 1. The `Parent-cross-domain:` preamble field is present, AND
/// 2. The declared ID matches the record's first `References:` target.
///
/// A mismatch is a misdeclaration (surfaced by L018, not here); a missing
/// field on a cross-domain first-References is surfaced by L011. This
/// helper returns `Some` only when the field and the structural parent
/// edge agree, so callers can treat the result as "this ADR has a
/// declared, structurally-honoured cross-domain parent."
#[must_use]
pub fn validated_cross_domain_parent(record: &AdrRecord) -> Option<AdrId> {
    let declared = record.parent_cross_domain.as_ref()?;
    let first_ref_target = record
        .relationships
        .iter()
        .find(|r| r.verb == RelVerb::References)
        .map(|r| &r.target)?;
    (first_ref_target == declared).then(|| declared.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AdrId;
    use crate::model::Tier;

    fn make_id(prefix: &str, num: u16) -> AdrId {
        AdrId {
            prefix: prefix.into(),
            number: num,
        }
    }

    #[test]
    fn render_refs_empty_emits_no_references_line() {
        let report = RefsReport {
            target_id: make_id("CHE", 1),
            target_title: Some("Test ADR".into()),
            target_tier: Some(Tier::B),
            target_status: Some(crate::model::Status::Accepted),
            refs: vec![],
        };
        let output = render_refs(&report);
        assert!(
            output.contains("## ◆ REFS: CHE-0001"),
            "header missing:\n{output}"
        );
        assert!(output.contains("Tier: B"), "tier missing:\n{output}");
        assert!(
            output.contains("Status: Accepted"),
            "status missing:\n{output}"
        );
        assert!(output.contains("Test ADR"), "title missing:\n{output}");
        assert!(
            output.contains("No references found."),
            "empty placeholder missing:\n{output}"
        );
    }

    #[test]
    fn render_refs_with_entries() {
        let report = RefsReport {
            target_id: make_id("CHE", 1),
            target_title: Some("Target".into()),
            target_tier: Some(Tier::A),
            target_status: Some(crate::model::Status::Accepted),
            refs: vec![
                crate::refs::RefEntry {
                    source_id: make_id("CHE", 2),
                    verb: RelVerb::References,
                    source_tier: Some(Tier::B),
                    source_status: Some(crate::model::Status::Accepted),
                    source_title: Some("Referencer".into()),
                },
                crate::refs::RefEntry {
                    source_id: make_id("CHE", 3),
                    verb: RelVerb::Supersedes,
                    source_tier: Some(Tier::B),
                    source_status: Some(crate::model::Status::SupersededBy(make_id("CHE", 99))),
                    source_title: Some("Superseder".into()),
                },
            ],
        };
        let output = render_refs(&report);
        assert!(
            output.contains("- CHE-0002 [References] | Tier: B | Status: Accepted | Referencer"),
            "References row missing:\n{output}"
        );
        assert!(
            output.contains(
                "- CHE-0003 [Supersedes] | Tier: B | Status: Superseded by CHE-0099 | Superseder"
            ),
            "Supersedes row with full status missing:\n{output}"
        );
        assert!(
            !output.contains("No references found."),
            "non-empty report must not show empty placeholder:\n{output}"
        );
    }

    #[test]
    fn render_refs_handles_missing_metadata() {
        let report = RefsReport {
            target_id: make_id("CHE", 1),
            target_title: None,
            target_tier: None,
            target_status: None,
            refs: vec![crate::refs::RefEntry {
                source_id: make_id("CHE", 2),
                verb: RelVerb::References,
                source_tier: None,
                source_status: None,
                source_title: None,
            }],
        };
        let output = render_refs(&report);
        assert!(
            output.contains("Tier: ?"),
            "missing tier placeholder:\n{output}"
        );
        assert!(
            output.contains("Status: ?"),
            "missing status placeholder:\n{output}"
        );
        assert!(
            output.contains("<no title>"),
            "missing title placeholder:\n{output}"
        );
    }

    #[test]
    fn render_diagnostics_clean() {
        let output = render_diagnostics(&[], 5);
        assert!(output.contains("0 warning(s)"));
    }

    #[test]
    fn render_diagnostics_with_warnings() {
        let diags = vec![Diagnostic::warning(
            "T020",
            &std::path::PathBuf::from("test.md"),
            1,
            "missing title".into(),
        )];
        let output = render_diagnostics(&diags, 1);
        assert!(output.contains("1 warning(s)"));
        assert!(output.contains("T020"));
    }

    // ── render_root_groups tests ────────────────────────────────────

    #[test]
    fn render_root_groups_basic() {
        let groups = vec![RootGroup {
            root_id: make_id("COM", 1),
            root_title: "Foundation Principle".into(),
            rules: vec![EmittedRule {
                adr_id: make_id("COM", 1),
                rule_id: "R1".into(),
                text: "All modules must log errors.".into(),
                layer: 5,
                depth: 0,
            }],
        }];
        let output = render_root_groups("example-core", &groups);
        // Preamble
        assert!(output.contains("# Architecture Rules"), "output:\n{output}");
        assert!(output.contains("crate `example-core`"), "output:\n{output}");
        assert!(
            output.contains("Follow every rule without exception"),
            "output:\n{output}"
        );
        // Root header
        assert!(
            output.contains("### COM-0001. Foundation Principle"),
            "output:\n{output}"
        );
        // Rule line with ID and layer
        assert!(
            output.contains("- All modules must log errors. [COM-0001:R1:L5]"),
            "output:\n{output}"
        );
    }

    #[test]
    fn render_root_groups_empty_group_skipped() {
        let groups = vec![
            RootGroup {
                root_id: make_id("COM", 1),
                root_title: "Empty Root".into(),
                rules: vec![],
            },
            RootGroup {
                root_id: make_id("CHE", 1),
                root_title: "Non-empty Root".into(),
                rules: vec![EmittedRule {
                    adr_id: make_id("CHE", 2),
                    rule_id: "R1".into(),
                    text: "Rule here.".into(),
                    layer: 3,
                    depth: 1,
                }],
            },
        ];
        let output = render_root_groups("test", &groups);
        assert!(
            !output.contains("Empty Root"),
            "empty group should be skipped:\n{output}"
        );
        assert!(
            output.contains("### CHE-0001. Non-empty Root"),
            "non-empty group should appear:\n{output}"
        );
    }

    #[test]
    fn render_root_groups_multiple_roots_ordering() {
        let groups = vec![
            RootGroup {
                root_id: make_id("COM", 1),
                root_title: "Foundation".into(),
                rules: vec![EmittedRule {
                    adr_id: make_id("COM", 1),
                    rule_id: "R1".into(),
                    text: "Foundation rule.".into(),
                    layer: 1,
                    depth: 0,
                }],
            },
            RootGroup {
                root_id: make_id("CHE", 1),
                root_title: "Domain Root".into(),
                rules: vec![EmittedRule {
                    adr_id: make_id("CHE", 5),
                    rule_id: "R1".into(),
                    text: "Domain rule.".into(),
                    layer: 7,
                    depth: 1,
                }],
            },
        ];
        let output = render_root_groups("test", &groups);
        let com_pos = output
            .find("### COM-0001. Foundation")
            .expect("COM header missing");
        let che_pos = output
            .find("### CHE-0001. Domain Root")
            .expect("CHE header missing");
        assert!(
            com_pos < che_pos,
            "Groups should render in order given:\n{output}"
        );
    }

    #[test]
    fn render_root_groups_all_empty_produces_preamble_only() {
        let groups = vec![RootGroup {
            root_id: make_id("COM", 1),
            root_title: "Empty".into(),
            rules: vec![],
        }];
        let output = render_root_groups("test", &groups);
        assert!(output.contains("# Architecture Rules"));
        assert!(
            !output.contains("###"),
            "no root headers for empty groups:\n{output}"
        );
    }

    #[test]
    fn render_root_groups_multiple_adrs_under_one_root() {
        let groups = vec![RootGroup {
            root_id: make_id("CHE", 1),
            root_title: "Design Priority".into(),
            rules: vec![
                EmittedRule {
                    adr_id: make_id("CHE", 1),
                    rule_id: "R1".into(),
                    text: "Root rule from the root itself.".into(),
                    layer: 2,
                    depth: 0,
                },
                EmittedRule {
                    adr_id: make_id("CHE", 5),
                    rule_id: "R1".into(),
                    text: "Child rule from CHE-0005.".into(),
                    layer: 5,
                    depth: 1,
                },
                EmittedRule {
                    adr_id: make_id("CHE", 10),
                    rule_id: "R1".into(),
                    text: "Grandchild rule from CHE-0010.".into(),
                    layer: 7,
                    depth: 2,
                },
            ],
        }];
        let output = render_root_groups("example-core", &groups);
        // Single root header
        assert!(
            output.contains("### CHE-0001. Design Priority"),
            "root header missing:\n{output}"
        );
        // All three rules present under that header
        assert!(
            output.contains("[CHE-0001:R1:L2]"),
            "root's own rule missing:\n{output}"
        );
        assert!(
            output.contains("[CHE-0005:R1:L5]"),
            "child rule missing:\n{output}"
        );
        assert!(
            output.contains("[CHE-0010:R1:L7]"),
            "grandchild rule missing:\n{output}"
        );
        // Verify ordering: L2 before L5 before L7
        let pos_l2 = output.find("[CHE-0001:R1:L2]").unwrap();
        let pos_l5 = output.find("[CHE-0005:R1:L5]").unwrap();
        let pos_l7 = output.find("[CHE-0010:R1:L7]").unwrap();
        assert!(
            pos_l2 < pos_l5 && pos_l5 < pos_l7,
            "rules should appear in layer order:\n{output}"
        );
    }
}
