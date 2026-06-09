//! Refs mode — single-hop reverse-reference query for an ADR.
//!
//! `--refs CHE-0042` returns every non-stale ADR that cites CHE-0042
//! via `References:` or `Supersedes:` in its `## Related` line. The
//! result is a compact, agent-readable bullet list: one row per
//! referrer, sorted by tier rank → prefix → number → verb.
//!
//! Excluded by design (per AFM-0021 R3, R5):
//!
//! - Stale referrers (referrer's `is_stale` set).
//! - Legacy forward verbs (`DependsOn`, `Extends`, `Illustrates`,
//!   `ContrastsWith`, `ScopedBy`).
//! - Legacy reverse verbs (filtered upstream by `nav::compute_children`).
//! - The lifecycle `Status: Superseded by X` field (not a structural
//!   `## Related` edge).
//! - Self-references (`rel.target == record.id`), including any
//!   ill-formed `Supersedes: SELF`.

use crate::model::{AdrId, AdrRecord, RelVerb, Status, Tier};
use crate::nav;

/// One inbound citation to a target ADR.
#[derive(Debug, Clone)]
pub struct RefEntry {
    pub source_id: AdrId,
    pub verb: RelVerb,
    pub source_tier: Option<Tier>,
    pub source_status: Option<Status>,
    pub source_title: Option<String>,
}

/// Result of a `--refs ADR_ID` query.
#[derive(Debug, Clone)]
pub struct RefsReport {
    pub target_id: AdrId,
    pub target_title: Option<String>,
    pub target_tier: Option<Tier>,
    pub target_status: Option<Status>,
    pub refs: Vec<RefEntry>,
}

/// Find every non-stale ADR that cites `target` via `References:`
/// or `Supersedes:`.
///
/// Sort order: tier rank (missing tier last) → prefix → number → verb
/// (alphabetical, `References` before `Supersedes`).
///
/// # Errors
///
/// Returns `Err` when `target` is not present in the parsed corpus.
pub fn find_refs(target: &AdrId, records: &[AdrRecord]) -> Result<RefsReport, String> {
    let Some(target_record) = records.iter().find(|r| r.id == *target) else {
        return Err(format!("ADR {target} not found"));
    };

    let children = nav::compute_children(records);

    let mut refs: Vec<RefEntry> = Vec::new();

    if let Some(entries) = children.get(target) {
        let by_id: std::collections::HashMap<&AdrId, &AdrRecord> =
            records.iter().map(|r| (&r.id, r)).collect();

        for entry in entries {
            if !matches!(entry.verb, RelVerb::References | RelVerb::Supersedes) {
                continue;
            }
            if entry.child == *target {
                continue;
            }
            let Some(source) = by_id.get(&entry.child) else {
                continue;
            };
            if source.is_stale {
                continue;
            }
            refs.push(RefEntry {
                source_id: source.id.clone(),
                verb: entry.verb,
                source_tier: source.tier,
                source_status: source.status.clone(),
                source_title: source.title.clone(),
            });
        }
    }

    refs.sort_by(|a, b| {
        let ta = a.source_tier.map_or(255, Tier::rank);
        let tb = b.source_tier.map_or(255, Tier::rank);
        ta.cmp(&tb)
            .then(a.source_id.prefix.cmp(&b.source_id.prefix))
            .then(a.source_id.number.cmp(&b.source_id.number))
            .then(verb_sort_key(a.verb).cmp(&verb_sort_key(b.verb)))
    });

    Ok(RefsReport {
        target_id: target_record.id.clone(),
        target_title: target_record.title.clone(),
        target_tier: target_record.tier,
        target_status: target_record.status.clone(),
        refs,
    })
}

/// Stable sort key for the two structural verbs. `References` <
/// `Supersedes` so duplicate-edge rows (same source, both verbs)
/// render in a deterministic order.
fn verb_sort_key(verb: RelVerb) -> u8 {
    match verb {
        RelVerb::References => 0,
        RelVerb::Supersedes => 1,
        _ => 255,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AdrId, AdrRecord, RelVerb, Relationship, Status, Tier};
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
            file_path: PathBuf::from(format!("nonexistent/{prefix}-{num:04}-test.md")),
            title: Some(format!("Test {prefix}-{num:04}")),
            title_line: 1,
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
    fn multiple_references_sorted() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 5, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]),
            make_record("CHE", 3, vec![(RelVerb::References, make_id("CHE", 1))]),
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        let nums: Vec<u16> = report.refs.iter().map(|r| r.source_id.number).collect();
        assert_eq!(nums, vec![2, 3, 5]);
    }

    #[test]
    fn supersedes_counts() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::Supersedes, make_id("CHE", 1))]),
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].verb, RelVerb::Supersedes);
    }

    #[test]
    fn mixed_verbs_same_source_deterministic() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record(
                "CHE",
                2,
                vec![
                    (RelVerb::Supersedes, make_id("CHE", 1)),
                    (RelVerb::References, make_id("CHE", 1)),
                ],
            ),
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert_eq!(report.refs.len(), 2);
        assert_eq!(report.refs[0].verb, RelVerb::References);
        assert_eq!(report.refs[1].verb, RelVerb::Supersedes);
    }

    #[test]
    fn zero_inbound_is_ok_empty() {
        let records = vec![make_record(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(report.refs.is_empty());
        assert_eq!(report.target_id, make_id("CHE", 1));
    }

    #[test]
    fn legacy_forward_verb_excluded() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::DependsOn, make_id("CHE", 1))]),
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(report.refs.is_empty(), "DependsOn must not contribute");
    }

    #[test]
    fn legacy_reverse_verb_excluded() {
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            make_record("CHE", 2, vec![(RelVerb::SupersededBy, make_id("CHE", 1))]),
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(
            report.refs.is_empty(),
            "reverse legacy verb must not contribute"
        );
    }

    #[test]
    fn status_supersededby_excluded() {
        let mut deprecated = make_record("CHE", 2, vec![(RelVerb::Root, make_id("CHE", 2))]);
        deprecated.status = Some(Status::SupersededBy(make_id("CHE", 1)));
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            deprecated,
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(
            report.refs.is_empty(),
            "lifecycle Status::SupersededBy must not appear as a structural inbound ref"
        );
    }

    #[test]
    fn stale_referrer_excluded() {
        let mut stale = make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        stale.is_stale = true;
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            stale,
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(report.refs.is_empty(), "stale referrer must be excluded");
    }

    #[test]
    fn stale_target_returns_live_referrers() {
        let mut stale_target = make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]);
        stale_target.is_stale = true;
        let mut stale_ref = make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        stale_ref.is_stale = true;
        let live_ref = make_record("CHE", 3, vec![(RelVerb::References, make_id("CHE", 1))]);
        let records = vec![stale_target, stale_ref, live_ref];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert_eq!(report.refs.len(), 1);
        assert_eq!(report.refs[0].source_id, make_id("CHE", 3));
    }

    #[test]
    fn unknown_target_returns_err() {
        let records = vec![make_record(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let result = find_refs(&make_id("CHE", 99), &records);
        match result {
            Err(e) => assert!(e.contains("not found"), "unexpected error: {e}"),
            Ok(_) => panic!("expected Err for unknown target"),
        }
    }

    #[test]
    fn sort_order_tier_prefix_number_verb() {
        let mut s_record = make_record("ZZZ", 9, vec![(RelVerb::References, make_id("CHE", 1))]);
        s_record.tier = Some(Tier::S);
        let mut a_record = make_record("AAA", 5, vec![(RelVerb::References, make_id("CHE", 1))]);
        a_record.tier = Some(Tier::A);
        let mut b_record_high_num =
            make_record("CHE", 10, vec![(RelVerb::References, make_id("CHE", 1))]);
        b_record_high_num.tier = Some(Tier::B);
        let mut b_record_low_num =
            make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        b_record_low_num.tier = Some(Tier::B);
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            s_record,
            a_record,
            b_record_high_num,
            b_record_low_num,
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        let order: Vec<(String, u16)> = report
            .refs
            .iter()
            .map(|r| (r.source_id.prefix.clone(), r.source_id.number))
            .collect();
        assert_eq!(
            order,
            vec![
                ("ZZZ".into(), 9),
                ("AAA".into(), 5),
                ("CHE".into(), 2),
                ("CHE".into(), 10),
            ]
        );
    }

    #[test]
    fn root_self_reference_excluded() {
        let records = vec![make_record(
            "CHE",
            1,
            vec![(RelVerb::Root, make_id("CHE", 1))],
        )];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(report.refs.is_empty(), "Root self-ref must not appear");
    }

    #[test]
    fn self_supersedes_excluded() {
        let records = vec![make_record(
            "CHE",
            1,
            vec![(RelVerb::Supersedes, make_id("CHE", 1))],
        )];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert!(report.refs.is_empty(), "self-Supersedes must not appear");
    }

    #[test]
    fn missing_tier_sorts_last_and_title_none_preserved() {
        let mut tier_b = make_record("CHE", 2, vec![(RelVerb::References, make_id("CHE", 1))]);
        tier_b.tier = Some(Tier::B);
        let mut no_tier = make_record("CHE", 3, vec![(RelVerb::References, make_id("CHE", 1))]);
        no_tier.tier = None;
        no_tier.title = None;
        let records = vec![
            make_record("CHE", 1, vec![(RelVerb::Root, make_id("CHE", 1))]),
            no_tier,
            tier_b,
        ];
        let report = find_refs(&make_id("CHE", 1), &records).unwrap();
        assert_eq!(report.refs.len(), 2);
        assert_eq!(report.refs[0].source_id.number, 2, "B-tier first");
        assert_eq!(report.refs[1].source_id.number, 3, "missing-tier last");
        assert!(report.refs[1].source_title.is_none());
    }
}
