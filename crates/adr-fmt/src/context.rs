//! Context mode — decision rules applicable to a specific crate.
//!
//! `--context example-core` resolves which ADRs apply to a crate
//! and extracts their tagged decision rules, grouped by root ADR subtree.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::config::Config;
use crate::model::{AdrId, AdrRecord, Status};
use crate::nav::{compute_parent_children, compute_parent_edges, walk_parent_chain};
use crate::output::{EmittedRule, RootGroup};

/// Resolve decision rules applicable to a crate, grouped by root ADR subtree.
///
/// Resolution chain:
/// 1. Find domains where `crate_name` ∈ `domain.crates` → candidate domains
/// 2. Within candidates: if any ADR has `crates` field populated, filter
///    to ADRs where `crate_name` ∈ `adr.crates`; else include all domain ADRs
/// 3. Always include all ADRs from `foundation = true` domains
///
/// Assignment uses the parent-edge tree projection: each ADR's structural
/// parent is its first `References:` target. The parent chain is walked
/// upward (cycle-safe via visited set) until a root is reached. Non-Accepted
/// parents (Draft/Proposed) are advisory-only — the chain flows through them
/// per the draft-waypoint policy. Cycle members and chains that do not
/// terminate at any root land in the Unclaimed group.
///
/// Emission: for each root in deterministic order, traverse the parent-edge
/// children downward (not the full citation graph) and emit rules from
/// eligible ADRs assigned to this root. Secondary citations do not pull
/// extra subtrees.
///
/// Returns `RootGroup` entries: foundation roots first (by min layer, then
/// number), then domain roots (same). An unclaimed fallback group is
/// appended when any eligible ADRs were not reached by any root.
///
/// # Errors
///
/// Returns an error if `crate_name` is not found in any domain's crate list.
#[expect(
    clippy::too_many_lines,
    reason = "context resolution is a linear stage pipeline (candidate/foundation identification → grouping → ordering) whose intermediate state would require multi-parameter helper signatures with no simplification gain"
)]
pub fn context_grouped(
    crate_name: &str,
    records: &[AdrRecord],
    config: &Config,
) -> Result<Vec<RootGroup>, String> {
    // ── Step 1: Identify candidate and foundation domains ──────────

    let candidate_domains: Vec<&str> = config
        .domains
        .iter()
        .filter(|d| d.crates.iter().any(|c| c == crate_name))
        .map(|d| d.prefix.as_str())
        .collect();

    if candidate_domains.is_empty() {
        return Err(format!(
            "crate '{crate_name}' not found in any domain's crate list"
        ));
    }

    let foundation_prefixes: Vec<&str> = config
        .domains
        .iter()
        .filter(|d| d.foundation)
        .map(|d| d.prefix.as_str())
        .collect();

    // ── Step 2: Collect emission-eligible ADR IDs ──────────────────

    let mut eligible: HashSet<AdrId> = HashSet::new();
    let mut eligible_records: HashMap<&AdrId, &AdrRecord> = HashMap::new();

    // Foundation ADRs
    for record in records {
        if record.is_stale || record.status.as_ref() != Some(&Status::Accepted) {
            continue;
        }
        if foundation_prefixes.contains(&record.id.prefix.as_str()) {
            if record.decision_rules.is_empty() {
                continue;
            }
            eligible.insert(record.id.clone());
            eligible_records.insert(&record.id, record);
        }
    }

    // Candidate domain ADRs (with per-ADR crate filtering)
    for prefix in &candidate_domains {
        let domain_records: Vec<&AdrRecord> = records
            .iter()
            .filter(|r| {
                !r.is_stale
                    && r.id.prefix == *prefix
                    && r.status.as_ref() == Some(&Status::Accepted)
            })
            .collect();

        let any_has_crates = domain_records.iter().any(|r| !r.crates.is_empty());

        for record in &domain_records {
            if any_has_crates
                && !record.crates.is_empty()
                && !record.crates.iter().any(|c| c == crate_name)
            {
                continue;
            }
            if record.decision_rules.is_empty() {
                continue;
            }
            eligible.insert(record.id.clone());
            eligible_records.insert(&record.id, record);
        }
    }

    // ── Step 3: Build parent-edge projection from ALL records ─────
    //
    // The tree projection includes Draft/Proposed waypoints — non-Accepted
    // parents are advisory-only (L012) and do not break the chain. This
    // lets descendants of a Draft mid-tier ADR still reach the root.

    let parent_edges = compute_parent_edges(records);
    let parent_children = compute_parent_children(records);

    // ── Step 4: Identify root ADRs ────────────────────────────────

    let root_index: HashSet<AdrId> = records
        .iter()
        .filter(|r| r.is_root())
        .map(|r| r.id.clone())
        .collect();

    let record_by_id: HashMap<&AdrId, &AdrRecord> = records.iter().map(|r| (&r.id, r)).collect();

    // ── Step 5: Assign eligible ADRs to roots via parent-chain walk ─
    //
    // Each eligible ADR walks its parent-edge chain upward. If the chain
    // terminates at a root, assign there. If it terminates at a non-root
    // (orphan) or hits a cycle, the ADR remains unassigned and falls to
    // the Unclaimed group.

    let mut assignment: HashMap<AdrId, AdrId> = HashMap::new();

    for id in &eligible {
        if root_index.contains(id) {
            assignment.insert(id.clone(), id.clone());
            continue;
        }
        if let Ok(terminal) = walk_parent_chain(id, &parent_edges)
            && root_index.contains(&terminal)
        {
            assignment.insert(id.clone(), terminal);
        }
        // Ok(t) where t not a root → broken chain → unassigned
        // Err(_): cycle in parent edges → unassigned (L013 already warns)
    }

    // ── Step 6: Determine root processing order ───────────────────
    //
    // Foundation roots first (by min_layer ascending, then number),
    // then domain roots (same sort).

    let foundation_set: HashSet<&str> = foundation_prefixes.iter().copied().collect();

    let mut context_roots: Vec<AdrId> = assignment
        .values()
        .collect::<HashSet<_>>()
        .into_iter()
        .cloned()
        .collect();

    // Sort roots: foundation first, then by min_layer, then by number
    context_roots.sort_by(|a, b| {
        let a_foundation = foundation_set.contains(a.prefix.as_str());
        let b_foundation = foundation_set.contains(b.prefix.as_str());

        // Foundation before domain
        b_foundation
            .cmp(&a_foundation)
            .then_with(|| {
                // Within group: sort by min layer of root's own rules
                let a_min_layer = record_by_id.get(a).map_or(u8::MAX, |r| {
                    r.decision_rules
                        .iter()
                        .map(|rule| rule.layer)
                        .min()
                        .unwrap_or(u8::MAX)
                });
                let b_min_layer = record_by_id.get(b).map_or(u8::MAX, |r| {
                    r.decision_rules
                        .iter()
                        .map(|rule| rule.layer)
                        .min()
                        .unwrap_or(u8::MAX)
                });
                a_min_layer.cmp(&b_min_layer)
            })
            .then_with(|| a.prefix.cmp(&b.prefix))
            .then_with(|| a.number.cmp(&b.number))
    });

    // ── Step 7: BFS emission per root via parent-edge children ────
    //
    // Walk parent-edge children downward from each root. Secondary
    // citations are NOT followed — they don't pull extra subtrees.
    // BFS visited set already provides cycle safety, but parent_edges
    // is a forest by construction (cycle members are excluded from
    // assignment in Step 5), so cycles cannot reach this stage.

    let mut claimed: HashSet<AdrId> = HashSet::new();
    let mut groups: Vec<RootGroup> = Vec::new();

    for root_id in &context_roots {
        let mut rules: Vec<EmittedRule> = Vec::new();

        let mut visited: HashSet<AdrId> = HashSet::new();
        let mut queue: VecDeque<(AdrId, u16)> = VecDeque::new();
        queue.push_back((root_id.clone(), 0));
        visited.insert(root_id.clone());

        while let Some((current_id, depth)) = queue.pop_front() {
            // Emit rules if eligible, assigned to this root, not yet claimed
            if eligible.contains(&current_id)
                && assignment.get(&current_id) == Some(root_id)
                && !claimed.contains(&current_id)
            {
                if let Some(record) = eligible_records.get(&current_id) {
                    for rule in &record.decision_rules {
                        rules.push(EmittedRule {
                            adr_id: current_id.clone(),
                            rule_id: rule.id.clone(),
                            text: rule.text.clone(),
                            layer: rule.layer,
                            depth,
                        });
                    }
                }
                claimed.insert(current_id.clone());
            }

            // Enqueue parent-edge children only
            if let Some(children) = parent_children.get(&current_id) {
                for child in children {
                    if !visited.contains(child) {
                        visited.insert(child.clone());
                        queue.push_back((child.clone(), depth + 1));
                    }
                }
            }
        }

        // Sort: layer asc → depth asc → ADR number asc → rule_id asc
        rules.sort_by(|a, b| {
            a.layer
                .cmp(&b.layer)
                .then(a.depth.cmp(&b.depth))
                .then(a.adr_id.prefix.cmp(&b.adr_id.prefix))
                .then(a.adr_id.number.cmp(&b.adr_id.number))
                .then(a.rule_id.cmp(&b.rule_id))
        });

        let root_title = record_by_id
            .get(root_id)
            .and_then(|r| r.title.as_deref())
            .unwrap_or("(untitled)")
            .to_string();

        groups.push(RootGroup {
            root_id: root_id.clone(),
            root_title,
            rules,
        });
    }

    // ── Step 8: Unclaimed fallback ─────────────────────────────────

    let unclaimed: Vec<&AdrId> = eligible
        .iter()
        .filter(|id| !claimed.contains(*id))
        .collect();

    if !unclaimed.is_empty() {
        let mut rules: Vec<EmittedRule> = Vec::new();
        for id in &unclaimed {
            if let Some(record) = eligible_records.get(id) {
                for rule in &record.decision_rules {
                    rules.push(EmittedRule {
                        adr_id: (*id).clone(),
                        rule_id: rule.id.clone(),
                        text: rule.text.clone(),
                        layer: rule.layer,
                        depth: u16::MAX,
                    });
                }
            }
        }
        rules.sort_by(|a, b| {
            a.layer
                .cmp(&b.layer)
                .then(a.adr_id.prefix.cmp(&b.adr_id.prefix))
                .then(a.adr_id.number.cmp(&b.adr_id.number))
                .then(a.rule_id.cmp(&b.rule_id))
        });
        groups.push(RootGroup {
            root_id: AdrId {
                prefix: String::new(),
                number: 0,
            },
            root_title: "Unclaimed Rules".to_string(),
            rules,
        });
    }

    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AdrId, AdrRecord, RelVerb, Relationship, Status, TaggedRule, Tier};
    use std::path::PathBuf;

    fn make_id(prefix: &str, num: u16) -> AdrId {
        AdrId {
            prefix: prefix.into(),
            number: num,
        }
    }

    fn make_config() -> Config {
        toml::from_str(
            r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "COM"
name = "Common"
directory = "common"
description = "Cross-cutting"
crates = []
foundation = true

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Architecture"
crates = ["example-core", "example-gateway"]

[[rules]]
id = "T020"
category = "template"
description = "test"
"#,
        )
        .unwrap()
    }

    fn make_record(
        prefix: &str,
        num: u16,
        crates: Vec<&str>,
        rules: Vec<(&str, u8, &str)>,
        rels: Vec<(RelVerb, &str, u16)>,
    ) -> AdrRecord {
        let id = make_id(prefix, num);
        AdrRecord {
            id: id.clone(),
            file_path: PathBuf::from(format!("{prefix}-{num:04}-test.md")),
            title: Some(format!("Test {prefix}-{num:04}")),
            title_line: 1,
            tier: Some(Tier::B),
            status: Some(Status::Accepted),
            status_raw: Some("Accepted".into()),
            has_related: true,
            has_context: true,
            has_decision: true,
            has_consequences: true,
            crates: crates
                .into_iter()
                .map(std::borrow::ToOwned::to_owned)
                .collect(),
            decision_rules: rules
                .into_iter()
                .map(|(rule_id, layer, text)| TaggedRule {
                    id: rule_id.into(),
                    text: text.into(),
                    line: 0,
                    layer,
                })
                .collect(),
            relationships: rels
                .into_iter()
                .enumerate()
                .map(|(i, (verb, p, n))| Relationship {
                    verb,
                    target: make_id(p, n),
                    line: 10 + i,
                })
                .collect(),
            ..AdrRecord::default()
        }
    }

    /// Collect all unique ADR IDs that emitted rules across all groups.
    fn all_emitted_adr_ids(groups: &[RootGroup]) -> Vec<AdrId> {
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for g in groups {
            for r in &g.rules {
                if seen.insert(r.adr_id.clone()) {
                    ids.push(r.adr_id.clone());
                }
            }
        }
        ids
    }

    /// Count total rules across all groups.
    fn total_rule_count(groups: &[RootGroup]) -> usize {
        groups.iter().map(|g| g.rules.len()).sum()
    }

    // ── Eligibility tests ──────────────────────────────────────────

    #[test]
    fn includes_foundation_and_domain() {
        let records = vec![
            make_record(
                "COM",
                1,
                vec![],
                vec![("R1", 2, "Foundation rule")],
                vec![(RelVerb::Root, "COM", 1)],
            ),
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Cherry rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        let prefixes: Vec<&str> = ids.iter().map(|id| id.prefix.as_str()).collect();
        assert!(prefixes.contains(&"COM"), "should include foundation");
        assert!(prefixes.contains(&"CHE"), "should include domain");
    }

    #[test]
    fn excludes_draft() {
        let mut draft = make_record(
            "CHE",
            2,
            vec![],
            vec![("R1", 5, "Draft rule")],
            vec![(RelVerb::References, "CHE", 1)],
        );
        draft.status = Some(Status::Draft);

        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Active rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            draft,
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        assert!(
            ids.contains(&make_id("CHE", 1)),
            "accepted should be included"
        );
        assert!(
            !ids.contains(&make_id("CHE", 2)),
            "draft should be excluded"
        );
    }

    #[test]
    fn excludes_rejected() {
        let mut rejected = make_record(
            "CHE",
            2,
            vec![],
            vec![("R1", 5, "Rejected rule")],
            vec![(RelVerb::References, "CHE", 1)],
        );
        rejected.status = Some(Status::Rejected);

        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Active rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            rejected,
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        assert!(ids.contains(&make_id("CHE", 1)));
        assert!(
            !ids.contains(&make_id("CHE", 2)),
            "rejected should be excluded"
        );
    }

    #[test]
    fn excludes_proposed_foundation() {
        let mut proposed = make_record(
            "COM",
            1,
            vec![],
            vec![("R1", 2, "Proposed rule")],
            vec![(RelVerb::Root, "COM", 1)],
        );
        proposed.status = Some(Status::Proposed);

        let records = vec![
            proposed,
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Active rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        assert!(
            !ids.contains(&make_id("COM", 1)),
            "proposed foundation excluded"
        );
        assert!(ids.contains(&make_id("CHE", 1)));
    }

    #[test]
    fn filters_by_per_adr_crates() {
        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Root rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            make_record(
                "CHE",
                2,
                vec!["example-core"],
                vec![("R1", 5, "Core rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
            make_record(
                "CHE",
                3,
                vec!["example-gateway"],
                vec![("R1", 5, "Gateway rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        assert!(
            ids.contains(&make_id("CHE", 2)),
            "core ADR should be included"
        );
        assert!(
            !ids.contains(&make_id("CHE", 3)),
            "gateway ADR should be excluded"
        );
    }

    #[test]
    fn excludes_stale() {
        let mut stale = make_record(
            "CHE",
            2,
            vec![],
            vec![("R1", 5, "Stale rule")],
            vec![(RelVerb::References, "CHE", 1)],
        );
        stale.is_stale = true;

        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Active rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            stale,
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        assert!(ids.contains(&make_id("CHE", 1)));
        assert!(
            !ids.contains(&make_id("CHE", 2)),
            "stale should be excluded"
        );
    }

    #[test]
    fn unknown_crate_returns_error() {
        let records = vec![make_record(
            "CHE",
            1,
            vec![],
            vec![("R1", 5, "Rule")],
            vec![(RelVerb::Root, "CHE", 1)],
        )];
        let config = make_config();
        let result = context_grouped("nonexistent-crate", &records, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found in any domain"));
    }

    // ── Assignment tests ───────────────────────────────────────────

    #[test]
    fn parent_chain_assigns_to_first_references_root() {
        // CHE-0002 references both CHE-0001 and CHE-0004 (roots).
        // CHE-0001 listed first → CHE-0002 assigned to CHE-0001.
        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 2, "Root 1 rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            make_record(
                "CHE",
                4,
                vec![],
                vec![("R1", 5, "Root 4 rule")],
                vec![(RelVerb::Root, "CHE", 4)],
            ),
            make_record(
                "CHE",
                2,
                vec![],
                vec![("R1", 5, "Child rule")],
                vec![
                    (RelVerb::References, "CHE", 1),
                    (RelVerb::References, "CHE", 4),
                ],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        // CHE-0002 should appear under CHE-0001's group
        let che1_group = groups
            .iter()
            .find(|g| g.root_id == make_id("CHE", 1))
            .unwrap();
        let che1_adr_ids: Vec<&AdrId> = che1_group.rules.iter().map(|r| &r.adr_id).collect();
        assert!(
            che1_adr_ids.contains(&&make_id("CHE", 2)),
            "CHE-0002 should be under CHE-0001"
        );

        let che4_group = groups
            .iter()
            .find(|g| g.root_id == make_id("CHE", 4))
            .unwrap();
        let che4_adr_ids: Vec<&AdrId> = che4_group.rules.iter().map(|r| &r.adr_id).collect();
        assert!(
            !che4_adr_ids.contains(&&make_id("CHE", 2)),
            "CHE-0002 should NOT be under CHE-0004"
        );
    }

    #[test]
    fn parent_chain_walks_through_intermediates() {
        // CHE-0003 references CHE-0002 (not a root). CHE-0002 references CHE-0001 (root).
        // CHE-0003 should be assigned to CHE-0001 via fallback.
        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 2, "Root rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            make_record(
                "CHE",
                2,
                vec![],
                vec![("R1", 5, "Middle rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
            make_record(
                "CHE",
                3,
                vec![],
                vec![("R1", 5, "Leaf rule")],
                vec![(RelVerb::References, "CHE", 2)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let che1_group = groups
            .iter()
            .find(|g| g.root_id == make_id("CHE", 1))
            .unwrap();
        let adr_ids: Vec<&AdrId> = che1_group.rules.iter().map(|r| &r.adr_id).collect();
        assert!(
            adr_ids.contains(&&make_id("CHE", 3)),
            "CHE-0003 should reach CHE-0001 via fallback"
        );
    }

    #[test]
    fn no_rule_appears_twice() {
        // Two roots with overlapping subtree: CHE-0003 references both.
        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 2, "Root 1")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            make_record(
                "CHE",
                4,
                vec![],
                vec![("R1", 5, "Root 4")],
                vec![(RelVerb::Root, "CHE", 4)],
            ),
            make_record(
                "CHE",
                3,
                vec![],
                vec![("R1", 5, "Shared rule")],
                vec![
                    (RelVerb::References, "CHE", 1),
                    (RelVerb::References, "CHE", 4),
                ],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        // Count total occurrences of CHE-0003's rules
        let che3_count: usize = groups
            .iter()
            .flat_map(|g| &g.rules)
            .filter(|r| r.adr_id == make_id("CHE", 3))
            .count();
        assert_eq!(che3_count, 1, "CHE-0003 rule should appear exactly once");
    }

    // ── BFS cycle safety ───────────────────────────────────────────

    #[test]
    fn cycle_does_not_loop() {
        // CHE-0002 ↔ CHE-0003 form a cycle, both reference root CHE-0001
        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 2, "Root rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            make_record(
                "CHE",
                2,
                vec![],
                vec![("R1", 5, "Cycle A")],
                vec![
                    (RelVerb::References, "CHE", 1),
                    (RelVerb::References, "CHE", 3),
                ],
            ),
            make_record(
                "CHE",
                3,
                vec![],
                vec![("R1", 5, "Cycle B")],
                vec![
                    (RelVerb::References, "CHE", 1),
                    (RelVerb::References, "CHE", 2),
                ],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        // Both should be present, each exactly once
        assert_eq!(total_rule_count(&groups), 3);
    }

    // ── Ordering tests ─────────────────────────────────────────────

    #[test]
    fn foundation_roots_before_domain_roots() {
        let records = vec![
            make_record(
                "COM",
                1,
                vec![],
                vec![("R1", 2, "Foundation root")],
                vec![(RelVerb::Root, "COM", 1)],
            ),
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 5, "Domain root")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        // Foundation (COM) should appear before domain (CHE)
        let root_ids: Vec<&AdrId> = groups.iter().map(|g| &g.root_id).collect();
        let com_pos = root_ids.iter().position(|id| id.prefix == "COM").unwrap();
        let che_pos = root_ids.iter().position(|id| id.prefix == "CHE").unwrap();
        assert!(com_pos < che_pos, "COM should appear before CHE");
    }

    #[test]
    fn within_root_rules_sorted_by_layer() {
        let records = vec![
            make_record("CHE", 1, vec![], vec![], vec![(RelVerb::Root, "CHE", 1)]),
            make_record(
                "CHE",
                2,
                vec![],
                vec![("R1", 9, "D-tier rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
            make_record(
                "CHE",
                3,
                vec![],
                vec![("R1", 2, "S-tier rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
            make_record(
                "CHE",
                4,
                vec![],
                vec![("R1", 5, "B-tier rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let che_group = groups
            .iter()
            .find(|g| g.root_id == make_id("CHE", 1))
            .unwrap();
        let layers: Vec<u8> = che_group.rules.iter().map(|r| r.layer).collect();
        assert_eq!(
            layers,
            vec![2, 5, 9],
            "rules should be sorted by layer ascending"
        );
    }

    #[test]
    fn within_same_layer_depth_then_number() {
        // CHE-0002 is depth 1, CHE-0003 is depth 2 (via CHE-0002), both at layer 5
        let records = vec![
            make_record("CHE", 1, vec![], vec![], vec![(RelVerb::Root, "CHE", 1)]),
            make_record(
                "CHE",
                2,
                vec![],
                vec![("R1", 5, "Depth 1 rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
            make_record(
                "CHE",
                3,
                vec![],
                vec![("R1", 5, "Depth 2 rule")],
                vec![(RelVerb::References, "CHE", 2)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let che_group = groups
            .iter()
            .find(|g| g.root_id == make_id("CHE", 1))
            .unwrap();
        let adr_nums: Vec<u16> = che_group.rules.iter().map(|r| r.adr_id.number).collect();
        assert_eq!(
            adr_nums,
            vec![2, 3],
            "depth 1 (CHE-0002) before depth 2 (CHE-0003)"
        );
    }

    // ── Edge cases ─────────────────────────────────────────────────

    #[test]
    fn root_with_no_rules_but_has_children() {
        // Root CHE-0001 has no rules, but child CHE-0002 does
        let records = vec![
            make_record("CHE", 1, vec![], vec![], vec![(RelVerb::Root, "CHE", 1)]),
            make_record(
                "CHE",
                2,
                vec![],
                vec![("R1", 5, "Child rule")],
                vec![(RelVerb::References, "CHE", 1)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let che_group = groups
            .iter()
            .find(|g| g.root_id == make_id("CHE", 1))
            .unwrap();
        assert_eq!(
            che_group.rules.len(),
            1,
            "children's rules should appear under root"
        );
    }

    #[test]
    fn empty_root_group_still_created() {
        // Root CHE-0001 has no rules and no eligible children
        let records = vec![make_record(
            "CHE",
            1,
            vec![],
            vec![],
            vec![(RelVerb::Root, "CHE", 1)],
        )];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        // Root should not produce a group (no eligible rules)
        assert!(
            groups.is_empty(),
            "root with no rules and no children → no group"
        );
    }

    #[test]
    fn non_accepted_waypoint_allows_reachability() {
        // CHE-0002 is Draft but links CHE-0001 → CHE-0003 in graph.
        // CHE-0003 references CHE-0002 (Draft). CHE-0003 should still
        // be assigned via fallback: CHE-0003 → CHE-0002 → CHE-0001.
        let mut draft = make_record(
            "CHE",
            2,
            vec![],
            vec![("R1", 5, "Draft rule")],
            vec![(RelVerb::References, "CHE", 1)],
        );
        draft.status = Some(Status::Draft);

        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 2, "Root rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            draft,
            make_record(
                "CHE",
                3,
                vec![],
                vec![("R1", 5, "Leaf rule")],
                vec![(RelVerb::References, "CHE", 2)],
            ),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let ids = all_emitted_adr_ids(&groups);
        assert!(ids.contains(&make_id("CHE", 1)), "root should be included");
        assert!(
            !ids.contains(&make_id("CHE", 2)),
            "draft should not emit rules"
        );
        assert!(
            ids.contains(&make_id("CHE", 3)),
            "leaf should be reachable via draft waypoint"
        );
    }

    #[test]
    fn unclaimed_fallback_when_unreachable() {
        // CHE-0002 references nothing — unreachable from any root
        let records = vec![
            make_record(
                "CHE",
                1,
                vec![],
                vec![("R1", 2, "Root rule")],
                vec![(RelVerb::Root, "CHE", 1)],
            ),
            make_record("CHE", 2, vec![], vec![("R1", 5, "Orphan rule")], vec![]),
        ];
        let config = make_config();
        let groups = context_grouped("example-core", &records, &config).unwrap();

        let unclaimed = groups.iter().find(|g| g.root_title == "Unclaimed Rules");
        assert!(unclaimed.is_some(), "should have unclaimed section");
        let unclaimed_ids: Vec<&AdrId> =
            unclaimed.unwrap().rules.iter().map(|r| &r.adr_id).collect();
        assert!(unclaimed_ids.contains(&&make_id("CHE", 2)));
    }

    #[test]
    fn root_processing_order_deterministic() {
        // Same records in different input order should produce same output
        let r1 = make_record(
            "CHE",
            1,
            vec![],
            vec![("R1", 2, "Root 1")],
            vec![(RelVerb::Root, "CHE", 1)],
        );
        let r4 = make_record(
            "CHE",
            4,
            vec![],
            vec![("R1", 5, "Root 4")],
            vec![(RelVerb::Root, "CHE", 4)],
        );
        let r2 = make_record(
            "CHE",
            2,
            vec![],
            vec![("R1", 5, "Child")],
            vec![
                (RelVerb::References, "CHE", 1),
                (RelVerb::References, "CHE", 4),
            ],
        );

        let config = make_config();

        let groups_a = context_grouped(
            "example-core",
            &[r1.clone(), r4.clone(), r2.clone()],
            &config,
        )
        .unwrap();
        // Note: can't easily clone AdrRecord, so create fresh for order B
        let r1b = make_record(
            "CHE",
            1,
            vec![],
            vec![("R1", 2, "Root 1")],
            vec![(RelVerb::Root, "CHE", 1)],
        );
        let r4b = make_record(
            "CHE",
            4,
            vec![],
            vec![("R1", 5, "Root 4")],
            vec![(RelVerb::Root, "CHE", 4)],
        );
        let r2b = make_record(
            "CHE",
            2,
            vec![],
            vec![("R1", 5, "Child")],
            vec![
                (RelVerb::References, "CHE", 1),
                (RelVerb::References, "CHE", 4),
            ],
        );

        let groups_b = context_grouped("example-core", &[r4b, r2b, r1b], &config).unwrap();

        let roots_a: Vec<&AdrId> = groups_a.iter().map(|g| &g.root_id).collect();
        let roots_b: Vec<&AdrId> = groups_b.iter().map(|g| &g.root_id).collect();
        assert_eq!(roots_a, roots_b, "root order should be deterministic");

        let count_a = total_rule_count(&groups_a);
        let count_b = total_rule_count(&groups_b);
        assert_eq!(count_a, count_b, "rule count should be deterministic");
    }
}
