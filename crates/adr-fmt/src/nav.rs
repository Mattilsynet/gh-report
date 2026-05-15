//! Computed children index — inverts forward links to produce a
//! reverse-link index on demand.
//!
//! Used by `--refs` mode for reverse-reference resolution.
//!
//! Two distinct projections are exported:
//!
//! - [`compute_children`] — full citation graph. Inverts every forward
//!   relationship (References, Supersedes; legacy non-reverse verbs are
//!   already filtered upstream). Used by `--refs` for inbound queries.
//!
//! - [`compute_parent_edges`] / [`compute_parent_children`] —
//!   structural tree. Each non-root ADR has at most one parent: the
//!   first `References:` target in document order. `Supersedes` and
//!   `Root` are never parent edges. Used by `--tree` and `--context`
//!   for tree rendering and parent-chain assignment.

use std::collections::HashMap;

use crate::model::{AdrId, AdrRecord, RelVerb};

/// A child entry: the verb that the child uses (forward direction) and
/// the child's ADR ID.
#[derive(Debug, Clone)]
pub struct ChildEntry {
    /// The forward verb the child used (e.g., `References`).
    pub verb: RelVerb,
    /// The child ADR ID.
    pub child: AdrId,
}

/// Compute a children index by inverting all forward relationships.
///
/// For each forward link `A -[verb]→ B`, inserts `B → (verb, A)`.
/// Root self-references are skipped (they are tree markers, not edges).
#[must_use]
pub fn compute_children(records: &[AdrRecord]) -> HashMap<AdrId, Vec<ChildEntry>> {
    let mut children: HashMap<AdrId, Vec<ChildEntry>> = HashMap::new();

    for record in records {
        for rel in &record.relationships {
            // Skip legacy reverse verbs and Root self-references
            if rel.verb.is_reverse() {
                continue;
            }
            if rel.verb == RelVerb::Root && rel.target == record.id {
                continue;
            }

            children
                .entry(rel.target.clone())
                .or_default()
                .push(ChildEntry {
                    verb: rel.verb,
                    child: record.id.clone(),
                });
        }
    }

    // Sort children by ID for stable output
    for entries in children.values_mut() {
        entries.sort_by(|a, b| {
            a.child
                .prefix
                .cmp(&b.child.prefix)
                .then(a.child.number.cmp(&b.child.number))
        });
    }

    children
}

/// Compute the parent-edge map for the structural tree projection.
///
/// For each non-root ADR, the parent edge is the **first `References:`
/// target in document order**. Verbs other than `References` are never
/// parent edges:
/// - `Root` is structural metadata, not an edge to a parent.
/// - `Supersedes` points to a retired ADR — not a structural parent.
/// - Legacy and reverse verbs are excluded.
///
/// Returns `child_id → parent_id`. Roots and ADRs without any
/// `References:` target are absent from the map (orphans / roots).
///
/// Cycles in the parent edge map are not detected here — callers
/// must guard traversal with a visited set (see L013 / context).
#[must_use]
pub fn compute_parent_edges(records: &[AdrRecord]) -> HashMap<AdrId, AdrId> {
    let mut edges: HashMap<AdrId, AdrId> = HashMap::new();

    for record in records {
        // Roots have no parent edge — they are forest roots
        if record.is_root() {
            continue;
        }
        // Find the first References target (document order)
        for rel in &record.relationships {
            if rel.verb == RelVerb::References {
                edges.insert(record.id.clone(), rel.target.clone());
                break;
            }
        }
    }

    edges
}

/// Compute the parent → children map by inverting [`compute_parent_edges`].
///
/// Returns `parent_id → [child_id, …]`. Children are sorted by
/// `(prefix, number)` for stable rendering.
///
/// Unlike [`compute_children`], secondary `References:` citations and
/// `Supersedes` edges are NOT included — only the first-position
/// parent edge counts toward tree structure.
#[must_use]
pub fn compute_parent_children(records: &[AdrRecord]) -> HashMap<AdrId, Vec<AdrId>> {
    let edges = compute_parent_edges(records);
    let mut children: HashMap<AdrId, Vec<AdrId>> = HashMap::new();

    for (child, parent) in edges {
        children.entry(parent).or_default().push(child);
    }

    for entries in children.values_mut() {
        entries.sort_by(|a, b| a.prefix.cmp(&b.prefix).then(a.number.cmp(&b.number)));
    }

    children
}

/// Walk the parent chain upward from `start`, collecting visited IDs.
///
/// Returns `Ok(root_id)` if the chain reaches an ADR with no parent
/// edge (a root, or an ADR with no `References:`). Returns
/// `Err(visited)` if a cycle is detected — `visited` contains the
/// IDs traversed in walk order, terminating with the last node
/// inserted before the cycle closure was observed. The cycle is
/// closed by the parent-edge from one of the listed entries back
/// to another listed entry. For a degenerate self-cycle (an ADR
/// whose parent edge points to itself), `visited` contains the
/// single entry `[start]`.
///
/// `parent_edges` is the map produced by [`compute_parent_edges`].
///
/// This is the cycle-safe primitive for `--context` parent-chain
/// assignment and L013 cycle detection.
pub fn walk_parent_chain(
    start: &AdrId,
    parent_edges: &HashMap<AdrId, AdrId>,
) -> Result<AdrId, Vec<AdrId>> {
    use std::collections::HashSet;

    let mut visited: HashSet<AdrId> = HashSet::new();
    let mut order: Vec<AdrId> = Vec::new();
    let mut current = start.clone();

    loop {
        if !visited.insert(current.clone()) {
            // Cycle detected — already saw this ID
            return Err(order);
        }
        order.push(current.clone());
        match parent_edges.get(&current) {
            Some(parent) => current = parent.clone(),
            None => return Ok(current),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AdrId, Relationship, Status, Tier};
    use std::path::PathBuf;

    fn make_id(prefix: &str, num: u16) -> AdrId {
        AdrId {
            prefix: prefix.into(),
            number: num,
        }
    }

    fn make_record(prefix: &str, num: u16, rels: Vec<(RelVerb, AdrId)>) -> AdrRecord {
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
            file_path: PathBuf::from(format!("docs/adr/test/{prefix}-{num:04}-test.md")),
            title: Some(format!("Test {prefix}-{num:04}")),
            title_line: 1,
            date: Some("2026-04-25".into()),
            tier: Some(Tier::B),
            status: Some(Status::Accepted),
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
    fn references_produces_child_entry() {
        let records = vec![
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
        ];
        let children = compute_children(&records);

        let che1_children = children.get(&make_id("CHE", 1)).unwrap();
        assert_eq!(che1_children.len(), 1);
        assert_eq!(che1_children[0].child, make_id("CHE", 2));
        assert_eq!(che1_children[0].verb, RelVerb::References);
    }

    #[test]
    fn root_self_reference_not_child() {
        let records = vec![make_record(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let children = compute_children(&records);
        // Root self-reference should NOT create a child entry
        assert!(
            children.is_empty(),
            "Root self-ref should not produce children"
        );
    }

    #[test]
    fn reverse_verbs_are_skipped() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Informs, make_id("CHE", 2))]),
            make_record("CHE", 2, vec![]),
        ];
        let children = compute_children(&records);
        assert!(
            children.is_empty(),
            "reverse verb should not produce children"
        );
    }

    #[test]
    fn empty_records_produce_empty_children() {
        let records: Vec<AdrRecord> = vec![];
        let children = compute_children(&records);
        assert!(children.is_empty());
    }

    #[test]
    fn multiple_children_sorted_by_id() {
        let records = vec![
            make_record("CHE", 3, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 5, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
        ];
        let children = compute_children(&records);
        let che1 = children.get(&make_id("CHE", 1)).unwrap();
        assert_eq!(che1.len(), 3);
        assert_eq!(che1[0].child.number, 2);
        assert_eq!(che1[1].child.number, 3);
        assert_eq!(che1[2].child.number, 5);
    }

    #[test]
    fn supersedes_produces_child_entry() {
        let records = vec![
            make_record("CHE", 2, vec![(RelVerb::Supersedes, make_id("CHE", 1))]),
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
        ];
        let children = compute_children(&records);
        let che1 = children.get(&make_id("CHE", 1)).unwrap();
        assert_eq!(che1.len(), 1);
        assert_eq!(che1[0].verb, RelVerb::Supersedes);
    }

    #[test]
    fn non_self_root_produces_child_entry() {
        // Root: CHE-0005 in CHE-0002's file — semantically invalid (L008
        // will warn) but compute_children should still produce the edge.
        let records = vec![
            make_record("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 5))]),
            make_record("CHE", 5, vec![(RelVerb::Root, make_id("CHE", 5))]),
        ];
        let children = compute_children(&records);
        let che5 = children.get(&make_id("CHE", 5)).unwrap();
        assert_eq!(che5.len(), 1);
        assert_eq!(che5[0].child, make_id("CHE", 2));
        assert_eq!(che5[0].verb, RelVerb::Root);
    }

    // ── Parent-edge projection tests ───────────────────────────────

    #[test]
    fn parent_edge_is_first_references() {
        // CHE-0003 references CHE-0002 then CHE-0001 — parent is CHE-0002
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record(
                "CHE",
                3,
                vec![
                    (RelVerb::References, make_id("CHE", 2)),
                    (RelVerb::References, make_id("CHE", 1)),
                ],
            ),
        ];
        let edges = compute_parent_edges(&records);
        assert_eq!(edges.get(&make_id("CHE", 3)), Some(&make_id("CHE", 2)));
        assert_eq!(edges.get(&make_id("CHE", 2)), Some(&make_id("CHE", 1)));
        assert!(
            !edges.contains_key(&make_id("CHE", 1)),
            "root has no parent edge"
        );
    }

    #[test]
    fn parent_edge_excludes_supersedes() {
        // CHE-0002 supersedes CHE-0001 — Supersedes is NOT a parent edge
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::Supersedes, make_id("CHE", 1))]),
        ];
        let edges = compute_parent_edges(&records);
        assert!(
            !edges.contains_key(&make_id("CHE", 2)),
            "Supersedes is not a parent edge"
        );
    }

    #[test]
    fn parent_edge_excludes_root_self() {
        // Root with self-reference does not produce a parent edge
        let records = vec![make_record(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let edges = compute_parent_edges(&records);
        assert!(edges.is_empty());
    }

    #[test]
    fn parent_edge_excludes_legacy_verbs() {
        // Depends on / Extends / Informs etc are not parent edges
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::DependsOn, make_id("CHE", 1))]),
            make_record("CHE", 3, vec![(RelVerb::Informs, make_id("CHE", 1))]),
        ];
        let edges = compute_parent_edges(&records);
        assert!(
            edges.is_empty(),
            "legacy/reverse verbs are not parent edges"
        );
    }

    #[test]
    fn parent_edge_root_then_references_picks_references() {
        // CHE-0002 has Root: CHE-0001 (mismatch — would warn L008) then
        // References: CHE-0001. Root is never a parent edge regardless
        // of position; first References wins.
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record(
                "CHE",
                2,
                vec![
                    (RelVerb::Root, make_id("CHE", 1)),
                    (RelVerb::References, make_id("CHE", 1)),
                ],
            ),
        ];
        let edges = compute_parent_edges(&records);
        // CHE-0002 is_root checks Root == OWN-ID; here Root: CHE-0001
        // is not self, so CHE-0002 is NOT a root. Parent edge = CHE-0001.
        assert_eq!(edges.get(&make_id("CHE", 2)), Some(&make_id("CHE", 1)));
    }

    #[test]
    fn parent_children_inverts_edges() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 3, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
        ];
        let pc = compute_parent_children(&records);
        let che1 = pc.get(&make_id("CHE", 1)).unwrap();
        assert_eq!(che1.len(), 2);
        // Sorted by (prefix, number)
        assert_eq!(che1[0].number, 2);
        assert_eq!(che1[1].number, 3);
    }

    #[test]
    fn parent_children_excludes_secondary_references() {
        // CHE-0003 references CHE-0002 (parent) and CHE-0001 (secondary).
        // CHE-0001 should NOT see CHE-0003 as a child via parent_children.
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record(
                "CHE",
                3,
                vec![
                    (RelVerb::References, make_id("CHE", 2)),
                    (RelVerb::References, make_id("CHE", 1)),
                ],
            ),
        ];
        let pc = compute_parent_children(&records);
        let che1_children = pc.get(&make_id("CHE", 1)).unwrap();
        assert_eq!(che1_children, &vec![make_id("CHE", 2)]);
        let che2_children = pc.get(&make_id("CHE", 2)).unwrap();
        assert_eq!(che2_children, &vec![make_id("CHE", 3)]);
    }

    #[test]
    fn walk_parent_chain_reaches_root() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 3, vec![(RelVerb::References, make_id("CHE", 2))]),
        ];
        let edges = compute_parent_edges(&records);
        let result = walk_parent_chain(&make_id("CHE", 3), &edges);
        assert_eq!(result, Ok(make_id("CHE", 1)));
    }

    #[test]
    fn walk_parent_chain_detects_cycle() {
        // Manually construct a cycle: CHE-0002 → CHE-0003 → CHE-0002
        // (compute_parent_edges would build this if both ADRs first-ref each other)
        let mut edges: HashMap<AdrId, AdrId> = HashMap::new();
        edges.insert(make_id("CHE", 2), make_id("CHE", 3));
        edges.insert(make_id("CHE", 3), make_id("CHE", 2));

        let result = walk_parent_chain(&make_id("CHE", 2), &edges);
        assert!(result.is_err(), "cycle should be detected");
        let visited = result.unwrap_err();
        assert_eq!(visited.len(), 2);
    }

    #[test]
    fn walk_parent_chain_terminates_on_orphan() {
        // ADR with no parent edge (e.g. broken chain) returns itself
        let edges: HashMap<AdrId, AdrId> = HashMap::new();
        let result = walk_parent_chain(&make_id("CHE", 7), &edges);
        assert_eq!(result, Ok(make_id("CHE", 7)));
    }

    #[test]
    fn walk_parent_chain_detects_self_cycle() {
        // Degenerate cycle: ADR's parent edge points to itself.
        // (compute_parent_edges would reject this since References cannot
        // contain self, but defense-in-depth: walk_parent_chain must
        // still terminate.)
        let mut edges: HashMap<AdrId, AdrId> = HashMap::new();
        edges.insert(make_id("CHE", 5), make_id("CHE", 5));
        let result = walk_parent_chain(&make_id("CHE", 5), &edges);
        assert!(result.is_err(), "self-cycle must be detected");
        let visited = result.unwrap_err();
        assert_eq!(visited, vec![make_id("CHE", 5)]);
    }
}
