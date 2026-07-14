//! Link and relationship rules (L001, L003, L006–L019).
//!
//! - L001 dangling link; L003 supersedes-status consistency; L006
//!   legacy verb (deprecated per AFM-0009); L007 stale reference;
//!   L008 Root self-reference mismatch; L009 Root+References
//!   coexistence.
//! - Tree-structure rules (parent-edge model, advisory): L010 missing
//!   parent; L011 cross-domain parent (suppress via
//!   `Parent-cross-domain:`); L012 non-Accepted parent; L013
//!   parent-edge cycle; L014 unreachable from root; L015/L016
//!   heuristics (flat-tree authoring, weak-tier parent); L017
//!   superseded parent; L018/L019 `Parent-cross-domain` field
//!   mismatch/dangling.
//!
//! Diagnostics are independent — one relationship may emit multiple
//! codes. Cycle dominance: when L013 fires for a record, L011/L012/
//! L014/L016/L017 are suppressed for it ("parent" is undefined inside
//! a cycle); L010 cannot fire for cycle members; L015 still fires
//! (inspects other References slots). Stale-archive ADRs (`is_stale`)
//! are exempt from L010–L017.

use std::collections::HashMap;

use crate::model::{AdrId, AdrRecord, RelVerb, Relationship, Status};
use crate::nav::{compute_parent_edges, walk_parent_chain};
use crate::report::Diagnostic;

pub fn check(records: &[AdrRecord], diags: &mut Vec<Diagnostic>) {
    let by_id: HashMap<&AdrId, &AdrRecord> = records.iter().map(|r| (&r.id, r)).collect();

    for record in records {
        check_root_references_coexistence(record, diags);

        for rel in &record.relationships {
            if rel.verb == RelVerb::Root {
                check_root_self_reference(record, rel, diags);
            }
        }

        for rel in &record.relationships {
            check_legacy_verb(record, rel, diags);
        }

        for rel in &record.relationships {
            check_single_link(record, rel, &by_id, diags);
        }

        check_parent_cross_domain_consistency(record, &by_id, diags);
    }

    check_supersedes_consistency(records, &by_id, diags);

    check_tree_structure(records, &by_id, diags);
}

fn check_single_link(
    source: &AdrRecord,
    rel: &Relationship,
    by_id: &HashMap<&AdrId, &AdrRecord>,
    diags: &mut Vec<Diagnostic>,
) {
    let target_id = &rel.target;

    if rel.verb == RelVerb::Root && rel.target == source.id {
        return;
    }

    if !by_id.contains_key(target_id) {
        diags.push(Diagnostic::warning(
            "L001",
            &source.file_path,
            rel.line,
            format!(
                "{} → {target_id}: dangling link (target ADR not found)",
                source.id,
            ),
        ));
        return;
    }

    if let Some(target_record) = by_id.get(target_id)
        && target_record.is_stale
        && !source.is_stale
        && rel.verb != RelVerb::Supersedes
    {
        diags.push(Diagnostic::warning(
            "L007",
            &source.file_path,
            rel.line,
            format!("{} → {target_id}: reference to stale ADR", source.id),
        ));
    }
}

/// L003: If A has `Supersedes: B`, then B's status must be
/// `Superseded by A`. Warns on inconsistency.
fn check_supersedes_consistency(
    records: &[AdrRecord],
    by_id: &HashMap<&AdrId, &AdrRecord>,
    diags: &mut Vec<Diagnostic>,
) {
    for record in records {
        for rel in &record.relationships {
            if rel.verb != RelVerb::Supersedes {
                continue;
            }

            let target_id = &rel.target;
            if let Some(target_record) = by_id.get(target_id) {
                let status_matches = matches!(
                    &target_record.status,
                    Some(crate::model::Status::SupersededBy(by_id)) if *by_id == record.id
                );

                if !status_matches {
                    diags.push(Diagnostic::warning(
                        "L003",
                        &record.file_path,
                        rel.line,
                        format!(
                            "{} supersedes {target_id}, but {target_id}'s status \
                             is not `Superseded by {}` — update the target's status",
                            record.id, record.id,
                        ),
                    ));
                }
            }
        }
    }
}

/// L008: Root self-reference mismatch.
fn check_root_self_reference(source: &AdrRecord, rel: &Relationship, diags: &mut Vec<Diagnostic>) {
    debug_assert_eq!(rel.verb, RelVerb::Root);
    if rel.target != source.id {
        diags.push(Diagnostic::warning(
            "L008",
            &source.file_path,
            rel.line,
            format!(
                "{}: Root target `{}` does not match own ID — \
                 Root must be a self-reference (`- Root: {}`)",
                source.id, rel.target, source.id,
            ),
        ));
    }
}

/// L006: Legacy relationship verb. AFM-0009 R1 restricts the vocabulary
/// to Root, References, Supersedes; any other parsed verb is legacy
/// and emits a deprecation warning with migration guidance.
///
/// `RelVerb::migration()` in model.rs is the single source of truth
/// for the legacy/permitted partition: it returns `Some(_)` exactly
/// when a verb is legacy. Adding or retiring a verb requires only
/// updating that helper.
fn check_legacy_verb(source: &AdrRecord, rel: &Relationship, diags: &mut Vec<Diagnostic>) {
    if let Some(migration) = rel.verb.migration() {
        diags.push(Diagnostic::warning(
            "L006",
            &source.file_path,
            rel.line,
            format!(
                "{}: legacy relationship verb `{}` → {} — {migration} \
                 (per AFM-0009)",
                source.id, rel.verb, rel.target,
            ),
        ));
    }
}

/// L018 / L019: validate the `Parent-cross-domain:` preamble field
/// against the actual References list and the corpus.
///
/// L018: declared ID doesn't match the first `References:` target —
/// either stale (re-ordered References) or misdeclared. `--tree`
/// treats the field as authoritative only on a match, so a mismatch
/// hides the cross-domain link there. L019: declared target ADR is
/// absent from the corpus — L001 only inspects relationship lines,
/// not preamble fields, so this would otherwise pass silently.
///
/// Roots and ADRs without `Parent-cross-domain` declared are skipped.
fn check_parent_cross_domain_consistency(
    record: &AdrRecord,
    by_id: &HashMap<&AdrId, &AdrRecord>,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(declared) = record.parent_cross_domain.as_ref() else {
        return;
    };

    if !by_id.contains_key(declared) {
        diags.push(Diagnostic::warning(
            "L019",
            &record.file_path,
            0,
            format!(
                "{} → {declared}: Parent-cross-domain target does not exist \
                 in the corpus — fix the field or remove it",
                record.id,
            ),
        ));
    }

    let first_ref_target = record
        .relationships
        .iter()
        .find(|r| r.verb == RelVerb::References)
        .map(|r| &r.target);

    match first_ref_target {
        Some(actual) if actual == declared => {}
        Some(actual) => {
            let line = record
                .relationships
                .iter()
                .find(|r| r.verb == RelVerb::References)
                .map_or(0, |r| r.line);
            diags.push(Diagnostic::warning(
                "L018",
                &record.file_path,
                line,
                format!(
                    "{}: Parent-cross-domain declares {declared}, but first \
                     References is {actual} — align the field with the \
                     structural parent or re-order References to put \
                     {declared} first",
                    record.id,
                ),
            ));
        }
        None => {
            if !record.is_root() {
                return;
            }
            diags.push(Diagnostic::warning(
                "L018",
                &record.file_path,
                0,
                format!(
                    "{}: Parent-cross-domain declared on a Root ADR — Roots \
                     have no parent edge; remove the field",
                    record.id,
                ),
            ));
        }
    }
}

/// L009: Root and References cannot coexist in the same Related section.
fn check_root_references_coexistence(source: &AdrRecord, diags: &mut Vec<Diagnostic>) {
    let has_root = source.relationships.iter().any(|r| r.verb == RelVerb::Root);
    let has_references = source
        .relationships
        .iter()
        .any(|r| r.verb == RelVerb::References);

    if has_root && has_references {
        let ref_line = source
            .relationships
            .iter()
            .find(|r| r.verb == RelVerb::References)
            .map_or(0, |r| r.line);

        diags.push(Diagnostic::warning(
            "L009",
            &source.file_path,
            ref_line,
            format!(
                "{}: Root and References cannot coexist — \
                 a root ADR stands alone structurally",
                source.id,
            ),
        ));
    }
}

/// L010–L017: parent-edge tree-structure diagnostics.
///
/// Operates on the parent-edge projection (see `nav::compute_parent_edges`)
/// rather than the full citation graph. Stale source ADRs are excluded
/// from these checks — orphaned ancestry is expected for retired ADRs.
fn check_tree_structure(
    records: &[AdrRecord],
    by_id: &HashMap<&AdrId, &AdrRecord>,
    diags: &mut Vec<Diagnostic>,
) {
    let parent_edges = compute_parent_edges(records);

    let cycle_members = detect_cycle_members(&parent_edges);
    emit_cycle_diagnostics(records, &cycle_members, diags);

    for record in records {
        if should_skip_tree_record(record) {
            continue;
        }
        if emit_missing_parent(record, &parent_edges, diags) {
            continue;
        }

        let Some(parent_id) = parent_edges.get(&record.id) else {
            continue;
        };

        let in_cycle = cycle_members.contains(&record.id);

        emit_cross_domain_parent(record, parent_id, by_id, in_cycle, diags);
        emit_parent_status_and_tier(record, parent_id, by_id, in_cycle, diags);
        emit_root_parent_candidate(record, parent_id, by_id, diags);
    }

    emit_unreachable_chain_diagnostics(records, by_id, &parent_edges, &cycle_members, diags);
}

fn should_skip_tree_record(record: &AdrRecord) -> bool {
    record.is_stale
}

fn emit_missing_parent(
    record: &AdrRecord,
    parent_edges: &HashMap<AdrId, AdrId>,
    diags: &mut Vec<Diagnostic>,
) -> bool {
    if record.is_root() || parent_edges.contains_key(&record.id) {
        return false;
    }

    let line = record
        .relationships
        .first()
        .map_or(record.status_line, |r| r.line);
    diags.push(Diagnostic::warning(
        "L010",
        &record.file_path,
        line,
        format!(
            "{}: non-root ADR has no `References:` target — \
             every non-root ADR needs a structural parent",
            record.id,
        ),
    ));
    true
}

fn parent_rel_line(record: &AdrRecord, parent_id: &AdrId) -> usize {
    record
        .relationships
        .iter()
        .find(|r| r.verb == RelVerb::References && r.target == *parent_id)
        .map_or(0, |r| r.line)
}

fn emit_cross_domain_parent(
    record: &AdrRecord,
    parent_id: &AdrId,
    by_id: &HashMap<&AdrId, &AdrRecord>,
    in_cycle: bool,
    diags: &mut Vec<Diagnostic>,
) {
    if in_cycle || parent_id.prefix == record.id.prefix || !by_id.contains_key(parent_id) {
        return;
    }
    let suppressed = record
        .parent_cross_domain
        .as_ref()
        .is_some_and(|allowed| allowed == parent_id);
    if suppressed {
        return;
    }
    diags.push(Diagnostic::warning(
        "L011",
        &record.file_path,
        parent_rel_line(record, parent_id),
        format!(
            "{} → {parent_id}: cross-domain parent edge — \
             add `Parent-cross-domain: {parent_id} — <reason>` \
             to the preamble to suppress, or pick a same-domain parent",
            record.id,
        ),
    ));
}

fn emit_parent_status_and_tier(
    record: &AdrRecord,
    parent_id: &AdrId,
    by_id: &HashMap<&AdrId, &AdrRecord>,
    in_cycle: bool,
    diags: &mut Vec<Diagnostic>,
) {
    if in_cycle {
        return;
    }
    if let Some(parent_record) = by_id.get(parent_id) {
        emit_parent_status(record, parent_id, parent_record, diags);
        emit_parent_tier(record, parent_id, parent_record, diags);
    }
}

fn emit_parent_status(
    record: &AdrRecord,
    parent_id: &AdrId,
    parent_record: &AdrRecord,
    diags: &mut Vec<Diagnostic>,
) {
    match &parent_record.status {
        Some(Status::Accepted) => {}
        Some(Status::SupersededBy(succ)) => {
            diags.push(Diagnostic::warning(
                "L017",
                &record.file_path,
                parent_rel_line(record, parent_id),
                format!(
                    "{} → {parent_id}: parent edge points at a superseded ADR \
                     (succeeded by {succ}) — redirect to the successor",
                    record.id,
                ),
            ));
        }
        Some(other) => {
            diags.push(Diagnostic::warning(
                "L012",
                &record.file_path,
                parent_rel_line(record, parent_id),
                format!(
                    "{} → {parent_id}: parent edge target is `{}`, not `Accepted` — \
                     advisory only; chain still flows through",
                    record.id,
                    other.short_display(),
                ),
            ));
        }
        None => {
            diags.push(Diagnostic::warning(
                "L012",
                &record.file_path,
                parent_rel_line(record, parent_id),
                format!(
                    "{} → {parent_id}: parent edge target has no status — \
                     advisory only; chain still flows through",
                    record.id,
                ),
            ));
        }
    }
}

fn emit_parent_tier(
    record: &AdrRecord,
    parent_id: &AdrId,
    parent_record: &AdrRecord,
    diags: &mut Vec<Diagnostic>,
) {
    if let (Some(parent_tier), Some(child_tier)) = (parent_record.tier, record.tier)
        && parent_tier.rank() > child_tier.rank()
    {
        diags.push(Diagnostic::warning(
            "L016",
            &record.file_path,
            parent_rel_line(record, parent_id),
            format!(
                "{} ({}) → {parent_id} ({}): parent tier is weaker leverage \
                 than child — heuristic, may be intentional",
                record.id, child_tier, parent_tier,
            ),
        ));
    }
}

fn emit_root_parent_candidate(
    record: &AdrRecord,
    parent_id: &AdrId,
    by_id: &HashMap<&AdrId, &AdrRecord>,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(parent_record) = by_id.get(parent_id) else {
        return;
    };
    if !parent_record.is_root() || !has_better_parent_candidate(record, parent_id, by_id) {
        return;
    }
    diags.push(Diagnostic::warning(
        "L015",
        &record.file_path,
        parent_rel_line(record, parent_id),
        format!(
            "{} → {parent_id}: first reference is a root while later \
             References include same-domain non-root candidates — \
             consider promoting one to first position",
            record.id,
        ),
    ));
}

fn has_better_parent_candidate(
    record: &AdrRecord,
    parent_id: &AdrId,
    by_id: &HashMap<&AdrId, &AdrRecord>,
) -> bool {
    record
        .relationships
        .iter()
        .filter(|r| r.verb == RelVerb::References && r.target != *parent_id)
        .any(|r| {
            by_id.get(&r.target).is_some_and(|cand| {
                cand.id.prefix == record.id.prefix
                    && !cand.is_root()
                    && cand.status.as_ref() == Some(&Status::Accepted)
            })
        })
}

fn emit_unreachable_chain_diagnostics(
    records: &[AdrRecord],
    by_id: &HashMap<&AdrId, &AdrRecord>,
    parent_edges: &HashMap<AdrId, AdrId>,
    cycle_members: &std::collections::HashSet<AdrId>,
    diags: &mut Vec<Diagnostic>,
) {
    for record in records {
        if record.is_stale
            || record.is_root()
            || !parent_edges.contains_key(&record.id)
            || cycle_members.contains(&record.id)
        {
            continue;
        }
        if let Ok(terminal) = walk_parent_chain(&record.id, parent_edges) {
            emit_unreachable_chain(record, &terminal, by_id, diags);
        }
    }
}

fn emit_unreachable_chain(
    record: &AdrRecord,
    terminal: &AdrId,
    by_id: &HashMap<&AdrId, &AdrRecord>,
    diags: &mut Vec<Diagnostic>,
) {
    if !by_id.contains_key(terminal) || by_id.get(terminal).is_some_and(|t| t.is_root()) {
        return;
    }
    let line = record
        .relationships
        .first()
        .map_or(record.status_line, |r| r.line);
    diags.push(Diagnostic::warning(
        "L014",
        &record.file_path,
        line,
        format!(
            "{}: parent chain ends at {terminal}, which is not a root — \
             non-root ADR unreachable from any root",
            record.id,
        ),
    ));
}

/// Identify all ADR IDs participating in a parent-edge cycle.
///
/// Walks each child once with a visited-set. Members of any detected
/// cycle are added to the returned set.
fn detect_cycle_members(parent_edges: &HashMap<AdrId, AdrId>) -> std::collections::HashSet<AdrId> {
    use std::collections::HashSet;

    let mut cycle_set: HashSet<AdrId> = HashSet::new();
    let mut globally_seen: HashSet<AdrId> = HashSet::new();

    for start in parent_edges.keys() {
        if globally_seen.contains(start) {
            continue;
        }
        let mut path: Vec<AdrId> = Vec::new();
        let mut path_set: HashSet<AdrId> = HashSet::new();
        let mut current = start.clone();
        loop {
            if path_set.contains(&current) {
                if let Some(start_idx) = path.iter().position(|id| id == &current) {
                    for id in &path[start_idx..] {
                        cycle_set.insert(id.clone());
                    }
                }
                break;
            }
            if globally_seen.contains(&current) {
                break;
            }
            path.push(current.clone());
            path_set.insert(current.clone());
            match parent_edges.get(&current) {
                Some(parent) => current = parent.clone(),
                None => break,
            }
        }
        for id in path {
            globally_seen.insert(id);
        }
    }

    cycle_set
}

fn emit_cycle_diagnostics(
    records: &[AdrRecord],
    cycle_members: &std::collections::HashSet<AdrId>,
    diags: &mut Vec<Diagnostic>,
) {
    if cycle_members.is_empty() {
        return;
    }
    for record in records {
        if !cycle_members.contains(&record.id) || record.is_stale {
            continue;
        }
        let line = record
            .relationships
            .iter()
            .find(|r| r.verb == RelVerb::References)
            .map_or(record.status_line, |r| r.line);
        diags.push(Diagnostic::warning(
            "L013",
            &record.file_path,
            line,
            format!(
                "{}: parent-edge graph contains a cycle through this ADR — \
                 break the cycle by re-rooting one of the participants",
                record.id,
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AdrId, Status, Tier};
    use std::path::PathBuf;

    fn make_id(prefix: &str, num: u16) -> AdrId {
        AdrId {
            prefix: prefix.into(),
            number: num,
        }
    }

    fn make_record_with_rels(prefix: &str, num: u16, rels: Vec<(RelVerb, AdrId)>) -> AdrRecord {
        let id = make_id(prefix, num);
        let relationships: Vec<Relationship> = rels
            .into_iter()
            .enumerate()
            .map(|(i, (verb, target))| Relationship {
                verb,
                target,
                line: 10 + i,
            })
            .collect();

        AdrRecord {
            id,
            file_path: PathBuf::from(format!("docs/adr/cherry/{prefix}-{num:04}-test.md")),
            title: Some("Test".into()),
            title_line: 1,
            date: Some("2026-04-25".into()),
            last_reviewed: Some("2026-04-25".into()),
            tier: Some(Tier::B),
            status: Some(Status::Accepted),
            status_line: 8,
            status_raw: Some("Accepted".into()),
            relationships,
            has_related: true,
            has_context: true,
            has_decision: true,
            has_consequences: true,
            ..AdrRecord::default()
        }
    }

    #[test]
    fn forward_link_no_errors() {
        let records = vec![
            make_record_with_rels("CHE", 1, vec![(RelVerb::References, make_id("CHE", 2))]),
            make_record_with_rels("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]),
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(diags.is_empty(), "expected no diags, got: {diags:?}");
    }

    #[test]
    fn dangling_link_produces_l001() {
        let records = vec![make_record_with_rels(
            "CHE",
            1,
            vec![(RelVerb::References, make_id("CHE", 99))],
        )];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L001"),
            "expected L001, got: {diags:?}"
        );
    }

    #[test]
    fn root_self_reference_match_no_l008() {
        let records = vec![make_record_with_rels(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L008"),
            "correct Root self-ref should not trigger L008"
        );
    }

    #[test]
    fn root_wrong_id_produces_l008() {
        let records = vec![make_record_with_rels(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 2))],
        )];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L008"),
            "Root pointing to wrong ID should trigger L008, got: {diags:?}"
        );
    }

    #[test]
    fn root_and_references_produces_l009() {
        let records = vec![
            make_record_with_rels(
                "CHE",
                1,
                vec![
                    (RelVerb::Root, make_id("CHE", 1)),
                    (RelVerb::References, make_id("CHE", 2)),
                ],
            ),
            make_record_with_rels("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]),
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L009"),
            "Root + References should trigger L009, got: {diags:?}"
        );
    }

    #[test]
    fn root_and_supersedes_no_l009() {
        let record = make_record_with_rels(
            "CHE",
            2,
            vec![
                (RelVerb::Root, make_id("CHE", 2)),
                (RelVerb::Supersedes, make_id("CHE", 1)),
            ],
        );
        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.status = Some(Status::SupersededBy(make_id("CHE", 2)));
        target.status_raw = Some("Superseded by CHE-0002".into());

        let records = vec![record, target];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L009"),
            "Root + Supersedes should not trigger L009, got: {diags:?}"
        );
    }

    #[test]
    fn supersedes_without_target_status_produces_l003() {
        let records = vec![
            make_record_with_rels("CHE", 2, vec![(RelVerb::Supersedes, make_id("CHE", 1))]),
            make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L003"),
            "expected L003, got: {diags:?}"
        );
    }

    #[test]
    fn supersedes_with_correct_target_status_no_l003() {
        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.status = Some(Status::SupersededBy(make_id("CHE", 2)));
        target.status_raw = Some("Superseded by CHE-0002".into());

        let records = vec![
            make_record_with_rels("CHE", 2, vec![(RelVerb::Supersedes, make_id("CHE", 1))]),
            target,
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L003"),
            "correct supersedes-status should not trigger L003, got: {diags:?}"
        );
    }

    #[test]
    fn stale_reference_produces_l007() {
        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.is_stale = true;

        let records = vec![
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            target,
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L007"),
            "expected L007, got: {diags:?}"
        );
    }

    #[test]
    fn supersedes_stale_no_l007() {
        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.is_stale = true;
        target.status = Some(Status::SupersededBy(make_id("CHE", 2)));
        target.status_raw = Some("Superseded by CHE-0002".into());

        let records = vec![
            make_record_with_rels("CHE", 2, vec![(RelVerb::Supersedes, make_id("CHE", 1))]),
            target,
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L007"),
            "Supersedes→stale should not trigger L007, got: {diags:?}"
        );
    }

    #[test]
    fn stale_source_references_stale_no_l007() {
        let mut source =
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        source.is_stale = true;

        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.is_stale = true;

        let records = vec![source, target];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L007"),
            "stale→stale should not trigger L007, got: {diags:?}"
        );
    }

    #[test]
    fn legacy_forward_verb_produces_l006() {
        let records = vec![
            make_record_with_rels("CHE", 1, vec![(RelVerb::DependsOn, make_id("CHE", 2))]),
            make_record_with_rels("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]),
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        let l006: Vec<_> = diags.iter().filter(|d| d.rule == "L006").collect();
        assert_eq!(l006.len(), 1, "expected exactly one L006, got: {diags:?}");
        assert!(
            l006[0].message.contains("Depends on"),
            "L006 message should name the legacy verb, got: {}",
            l006[0].message
        );
        assert!(
            l006[0].message.contains("use References"),
            "L006 message should include migration guidance, got: {}",
            l006[0].message
        );
    }

    #[test]
    fn legacy_reverse_verb_produces_l006_with_remove_guidance() {
        let records = vec![
            make_record_with_rels("CHE", 1, vec![(RelVerb::Informs, make_id("CHE", 2))]),
            make_record_with_rels("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]),
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        let l006: Vec<_> = diags.iter().filter(|d| d.rule == "L006").collect();
        assert_eq!(l006.len(), 1, "expected exactly one L006, got: {diags:?}");
        assert!(
            l006[0].message.contains("remove (reverse verb)"),
            "reverse verb should suggest removal, got: {}",
            l006[0].message
        );
    }

    #[test]
    fn permitted_verbs_no_l006() {
        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.status = Some(Status::SupersededBy(make_id("CHE", 3)));
        target.status_raw = Some("Superseded by CHE-0003".into());

        let records = vec![
            make_record_with_rels(
                "CHE",
                3,
                vec![
                    (RelVerb::Root, make_id("CHE", 3)),
                    (RelVerb::Supersedes, make_id("CHE", 1)),
                ],
            ),
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 3))]),
            target,
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L006"),
            "permitted verbs should not trigger L006, got: {diags:?}"
        );
    }

    #[test]
    fn legacy_verb_with_dangling_target_emits_both_l006_and_l001() {
        let records = vec![make_record_with_rels(
            "CHE",
            1,
            vec![(RelVerb::Extends, make_id("CHE", 999))],
        )];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L006"),
            "expected L006 (legacy verb), got: {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.rule == "L001"),
            "expected L001 (dangling), got: {diags:?}"
        );
    }

    #[test]
    fn legacy_verb_to_stale_target_emits_l006_and_l007() {
        let mut target = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        target.is_stale = true;

        let records = vec![
            make_record_with_rels("CHE", 2, vec![(RelVerb::DependsOn, make_id("CHE", 1))]),
            target,
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L006"),
            "expected L006 (legacy verb), got: {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.rule == "L007"),
            "expected L007 (stale ref), got: {diags:?}"
        );
    }

    #[test]
    fn every_legacy_verb_triggers_l006() {
        for &verb in RelVerb::legacy() {
            let records = vec![
                make_record_with_rels("CHE", 1, vec![(verb, make_id("CHE", 2))]),
                make_record_with_rels("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]),
            ];
            let mut diags = Vec::new();
            check(&records, &mut diags);
            assert!(
                diags.iter().any(|d| d.rule == "L006"),
                "legacy verb {verb:?} should trigger L006, got: {diags:?}"
            );
        }
    }

    #[test]
    fn no_permitted_verb_triggers_l006() {
        for &verb in RelVerb::permitted() {
            let target = if verb == RelVerb::Root {
                make_id("CHE", 1)
            } else {
                make_id("CHE", 2)
            };
            let mut other =
                make_record_with_rels("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]);
            if verb == RelVerb::Supersedes {
                other.status = Some(Status::SupersededBy(make_id("CHE", 1)));
                other.status_raw = Some("Superseded by CHE-0001".into());
            }
            let records = vec![make_record_with_rels("CHE", 1, vec![(verb, target)]), other];
            let mut diags = Vec::new();
            check(&records, &mut diags);
            assert!(
                !diags.iter().any(|d| d.rule == "L006"),
                "permitted verb {verb:?} should not trigger L006, got: {diags:?}"
            );
        }
    }

    #[test]
    fn non_root_without_references_produces_l010() {
        let records = vec![
            make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record_with_rels("CHE", 2, vec![]),
        ];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L010"),
            "expected L010, got: {diags:?}"
        );
    }

    #[test]
    fn root_without_references_no_l010() {
        let records = vec![make_record_with_rels(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L010"),
            "root should be exempt from L010, got: {diags:?}"
        );
    }

    #[test]
    fn root_with_supersedes_only_no_l010() {
        let mut predecessor =
            make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        predecessor.status = Some(Status::SupersededBy(make_id("CHE", 2)));
        predecessor.status_raw = Some("Superseded by CHE-0002".into());

        let new_root = make_record_with_rels(
            "CHE",
            2,
            vec![
                (RelVerb::Root, make_id("CHE", 2)),
                (RelVerb::Supersedes, make_id("CHE", 1)),
            ],
        );

        let records = vec![predecessor, new_root];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L010"),
            "Root + Supersedes should not trigger L010, got: {diags:?}"
        );
    }

    #[test]
    fn cross_domain_parent_produces_l011() {
        let mut com_root =
            make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        com_root.file_path = PathBuf::from("docs/adr/common/COM-0001-test.md");

        let che = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("COM", 1))]);

        let records = vec![com_root, che];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L011"),
            "expected L011, got: {diags:?}"
        );
    }

    #[test]
    fn cross_domain_parent_suppressed_by_field() {
        let mut com_root =
            make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        com_root.file_path = PathBuf::from("docs/adr/common/COM-0001-test.md");

        let mut che =
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("COM", 1))]);
        che.parent_cross_domain = Some(make_id("COM", 1));
        che.parent_cross_domain_reason = "boundary ADR".into();

        let records = vec![com_root, che];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L011"),
            "Parent-cross-domain field should suppress L011, got: {diags:?}"
        );
    }

    #[test]
    fn cross_domain_suppression_only_for_named_target() {
        let mut com1 = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        com1.file_path = PathBuf::from("docs/adr/common/COM-0001-test.md");
        let mut com2 =
            make_record_with_rels("COM", 2, vec![(RelVerb::References, make_id("COM", 1))]);
        com2.file_path = PathBuf::from("docs/adr/common/COM-0002-test.md");

        let mut che =
            make_record_with_rels("CHE", 5, vec![(RelVerb::References, make_id("COM", 2))]);
        che.parent_cross_domain = Some(make_id("COM", 1));
        let records = vec![com1, com2, che];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L011"),
            "suppression must match actual parent target, got: {diags:?}"
        );
    }

    #[test]
    fn non_accepted_parent_produces_l012() {
        let mut parent = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        parent.status = Some(Status::Draft);
        parent.status_raw = Some("Draft".into());

        let child = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        let records = vec![parent, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L012"),
            "expected L012 for Draft parent, got: {diags:?}"
        );
    }

    #[test]
    fn superseded_parent_produces_l017_not_l012() {
        let mut parent = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        parent.status = Some(Status::SupersededBy(make_id("CHE", 9)));
        parent.status_raw = Some("Superseded by CHE-0009".into());

        let mut succ = make_record_with_rels(
            "CHE",
            9,
            vec![
                (RelVerb::Root, make_id("CHE", 9)),
                (RelVerb::Supersedes, make_id("CHE", 1)),
            ],
        );
        succ.status = Some(Status::Accepted);

        let child = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        let records = vec![parent, succ, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L017"),
            "expected L017, got: {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.rule == "L012"),
            "L017 supersedes L012 for superseded parent, got: {diags:?}"
        );
    }

    #[test]
    fn parent_edge_cycle_produces_l013() {
        let a = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 3))]);
        let b = make_record_with_rels("CHE", 3, vec![(RelVerb::References, make_id("CHE", 2))]);
        let mut diags = Vec::new();
        check(&[a, b], &mut diags);
        let l013_count = diags.iter().filter(|d| d.rule == "L013").count();
        assert_eq!(
            l013_count, 2,
            "expected L013 for both cycle members, got: {diags:?}"
        );
    }

    #[test]
    fn secondary_reference_cycle_does_not_trigger_l013() {
        let root = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        let a = make_record_with_rels(
            "CHE",
            2,
            vec![
                (RelVerb::References, make_id("CHE", 1)),
                (RelVerb::References, make_id("CHE", 3)),
            ],
        );
        let b = make_record_with_rels(
            "CHE",
            3,
            vec![
                (RelVerb::References, make_id("CHE", 1)),
                (RelVerb::References, make_id("CHE", 2)),
            ],
        );
        let mut diags = Vec::new();
        check(&[root, a, b], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L013"),
            "secondary cycles must not trigger L013, got: {diags:?}"
        );
    }

    #[test]
    fn unreachable_from_root_produces_l014() {
        let a = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 3))]);
        let b = make_record_with_rels("CHE", 3, vec![(RelVerb::References, make_id("CHE", 4))]);
        let c = make_record_with_rels("CHE", 4, vec![(RelVerb::Supersedes, make_id("CHE", 99))]);
        let mut diags = Vec::new();
        check(&[a, b, c], &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L014"),
            "expected L014, got: {diags:?}"
        );
    }

    #[test]
    fn dangling_terminal_does_not_double_report_l014() {
        let a = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 99))]);
        let mut diags = Vec::new();
        check(&[a], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L014"),
            "L014 must not fire on dangling terminal, got: {diags:?}"
        );
    }

    #[test]
    fn dangling_cross_domain_parent_does_not_double_report_l011() {
        let a = make_record_with_rels("PAR", 1, vec![(RelVerb::References, make_id("CHE", 99))]);
        let mut diags = Vec::new();
        check(&[a], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L011"),
            "L011 must not fire on dangling cross-domain target, got: {diags:?}"
        );
    }

    #[test]
    fn reachable_from_root_no_l014() {
        let root = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        let mid = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        let leaf = make_record_with_rels("CHE", 3, vec![(RelVerb::References, make_id("CHE", 2))]);
        let mut diags = Vec::new();
        check(&[root, mid, leaf], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L014"),
            "chain reaching root must not trigger L014, got: {diags:?}"
        );
    }

    #[test]
    fn root_first_with_local_candidate_produces_l015() {
        let root = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        let mid = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        let leaf = make_record_with_rels(
            "CHE",
            3,
            vec![
                (RelVerb::References, make_id("CHE", 1)),
                (RelVerb::References, make_id("CHE", 2)),
            ],
        );
        let mut diags = Vec::new();
        check(&[root, mid, leaf], &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L015"),
            "expected L015, got: {diags:?}"
        );
    }

    #[test]
    fn root_first_no_other_candidates_no_l015() {
        let root = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        let leaf = make_record_with_rels("CHE", 3, vec![(RelVerb::References, make_id("CHE", 1))]);
        let mut diags = Vec::new();
        check(&[root, leaf], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L015"),
            "no other candidates means no L015, got: {diags:?}"
        );
    }

    #[test]
    fn l015_ignores_non_accepted_candidates() {
        let root = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        let mut mid =
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        mid.status = Some(Status::Draft);
        let leaf = make_record_with_rels(
            "CHE",
            3,
            vec![
                (RelVerb::References, make_id("CHE", 1)),
                (RelVerb::References, make_id("CHE", 2)),
            ],
        );
        let mut diags = Vec::new();
        check(&[root, mid, leaf], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L015"),
            "Draft candidate must not trigger L015, got: {diags:?}"
        );
    }

    #[test]
    fn lower_tier_parent_produces_l016() {
        let mut parent = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        parent.tier = Some(Tier::D);
        let mut child =
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        child.tier = Some(Tier::B);
        let mut diags = Vec::new();
        check(&[parent, child], &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L016"),
            "expected L016, got: {diags:?}"
        );
    }

    #[test]
    fn same_or_higher_tier_parent_no_l016() {
        let mut parent = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        parent.tier = Some(Tier::S);
        let mut child =
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        child.tier = Some(Tier::B);
        let mut diags = Vec::new();
        check(&[parent, child], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L016"),
            "higher-tier parent should not trigger L016, got: {diags:?}"
        );
    }

    #[test]
    fn l012_l007_co_emission_for_stale_non_accepted_parent() {
        let mut parent = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        parent.status = Some(Status::Draft);
        parent.is_stale = true;

        let child = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        let mut diags = Vec::new();
        check(&[parent, child], &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L007"),
            "expected L007 for stale ref, got: {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.rule == "L012"),
            "expected L012 for non-Accepted parent, got: {diags:?}"
        );
    }

    #[test]
    fn stale_source_skipped_for_tree_structure() {
        let mut stale = make_record_with_rels("CHE", 2, vec![]);
        stale.is_stale = true;
        let mut diags = Vec::new();
        check(&[stale], &mut diags);
        assert!(
            !diags.iter().any(|d| matches!(
                d.rule,
                "L010" | "L011" | "L012" | "L013" | "L014" | "L015" | "L016" | "L017"
            )),
            "stale source should be exempt from tree-structure rules, got: {diags:?}"
        );
    }

    #[test]
    fn cross_domain_suppression_independent_of_reason_text() {
        let root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        let mut child =
            make_record_with_rels("CHE", 5, vec![(RelVerb::References, make_id("COM", 1))]);
        child.parent_cross_domain = Some(make_id("COM", 1));
        child.parent_cross_domain_reason = String::new();
        let mut diags = Vec::new();
        check(&[root, child], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L011"),
            "empty-reason Parent-cross-domain must still suppress L011, got: {diags:?}"
        );
    }

    #[test]
    fn l017_takes_precedence_over_l012_in_cycle() {
        let mut a = make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 3))]);
        a.status = Some(Status::Accepted);
        let mut b = make_record_with_rels("CHE", 3, vec![(RelVerb::References, make_id("CHE", 2))]);
        b.status = Some(Status::SupersededBy(make_id("CHE", 99)));
        let mut diags = Vec::new();
        check(&[a, b], &mut diags);
        let l013s: Vec<_> = diags.iter().filter(|d| d.rule == "L013").collect();
        assert_eq!(l013s.len(), 2, "expected 2× L013, got: {diags:?}");
        assert!(
            !diags.iter().any(|d| d.rule == "L017"),
            "L017 should not fire for cycle-member parents, got: {diags:?}"
        );
    }

    #[test]
    fn l015_does_not_fire_when_no_root_first() {
        let parent =
            make_record_with_rels("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        let candidate =
            make_record_with_rels("CHE", 7, vec![(RelVerb::References, make_id("CHE", 1))]);
        let child = make_record_with_rels(
            "CHE",
            5,
            vec![
                (RelVerb::References, make_id("CHE", 2)),
                (RelVerb::References, make_id("CHE", 7)),
            ],
        );
        let root = make_record_with_rels("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        let mut diags = Vec::new();
        check(&[root, parent, candidate, child], &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L015"),
            "L015 must not fire when first ref is not a Root, got: {diags:?}"
        );
    }

    #[test]
    fn l018_fires_on_mismatch_between_field_and_first_reference() {
        let com_root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        let gnd_root = make_record_with_rels("GND", 6, vec![(RelVerb::Root, make_id("GND", 6))]);

        let mut child = make_record_with_rels(
            "COM",
            8,
            vec![
                (RelVerb::References, make_id("COM", 1)),
                (RelVerb::References, make_id("GND", 6)),
            ],
        );
        child.parent_cross_domain = Some(make_id("GND", 6));

        let records = vec![com_root, gnd_root, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L018"),
            "expected L018 for mismatch, got: {diags:?}"
        );
    }

    #[test]
    fn l018_silent_when_field_matches_first_reference() {
        let com_root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        let gnd_root = make_record_with_rels("GND", 6, vec![(RelVerb::Root, make_id("GND", 6))]);

        let mut child = make_record_with_rels(
            "COM",
            8,
            vec![
                (RelVerb::References, make_id("GND", 6)),
                (RelVerb::References, make_id("COM", 1)),
            ],
        );
        child.parent_cross_domain = Some(make_id("GND", 6));

        let records = vec![com_root, gnd_root, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L018"),
            "L018 must be silent on consistent field, got: {diags:?}"
        );
    }

    #[test]
    fn l019_fires_when_declared_target_does_not_exist() {
        let com_root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);

        let mut child =
            make_record_with_rels("COM", 8, vec![(RelVerb::References, make_id("COM", 1))]);
        child.parent_cross_domain = Some(make_id("GND", 99));

        let records = vec![com_root, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L019"),
            "expected L019 for dangling Parent-cross-domain, got: {diags:?}"
        );
    }

    #[test]
    fn l019_silent_when_target_exists() {
        let com_root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        let gnd_root = make_record_with_rels("GND", 6, vec![(RelVerb::Root, make_id("GND", 6))]);

        let mut child =
            make_record_with_rels("COM", 8, vec![(RelVerb::References, make_id("GND", 6))]);
        child.parent_cross_domain = Some(make_id("GND", 6));

        let records = vec![com_root, gnd_root, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "L019"),
            "L019 must be silent when target exists, got: {diags:?}"
        );
    }

    #[test]
    fn l018_silent_when_no_field_declared() {
        let com_root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        let child = make_record_with_rels("COM", 8, vec![(RelVerb::References, make_id("COM", 1))]);

        let records = vec![com_root, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            !diags.iter().any(|d| matches!(d.rule, "L018" | "L019")),
            "L018/L019 must not fire without Parent-cross-domain, got: {diags:?}"
        );
    }

    #[test]
    fn l018_fires_on_root_with_field() {
        let mut com_root =
            make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        com_root.parent_cross_domain = Some(make_id("GND", 1));

        let gnd_root = make_record_with_rels("GND", 1, vec![(RelVerb::Root, make_id("GND", 1))]);

        let records = vec![com_root, gnd_root];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L018"),
            "L018 must fire when a Root declares Parent-cross-domain, got: {diags:?}"
        );
    }

    #[test]
    fn l018_and_l011_co_emit_on_mismatched_field() {
        let com_root = make_record_with_rels("COM", 1, vec![(RelVerb::Root, make_id("COM", 1))]);
        let gnd_a = make_record_with_rels("GND", 1, vec![(RelVerb::Root, make_id("GND", 1))]);
        let gnd_b = make_record_with_rels("GND", 6, vec![(RelVerb::Root, make_id("GND", 6))]);

        let mut child =
            make_record_with_rels("COM", 8, vec![(RelVerb::References, make_id("GND", 1))]);
        child.parent_cross_domain = Some(make_id("GND", 6));

        let records = vec![com_root, gnd_a, gnd_b, child];
        let mut diags = Vec::new();
        check(&records, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "L018"),
            "L018 expected on field/References mismatch, got: {diags:?}"
        );
        assert!(
            diags.iter().any(|d| d.rule == "L011"),
            "L011 expected on un-suppressed cross-domain edge, got: {diags:?}"
        );
    }
}
