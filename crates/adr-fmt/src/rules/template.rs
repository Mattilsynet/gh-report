//! Template compliance rules (T002–T020) and structure rules (S004–S007).
//!
//! T002: Date field present
//! T003: Last-reviewed field required (all tiers)
//! T004: Tier field present
//! T005: Status section present
//! T005c: Legacy ## Status section — migrate to Status: preamble field
//! T006: Status value valid (strict keyword, no parentheticals)
//! T007: Related section with at least one relationship (active ADRs only — skipped for stale)
//! T008: Context section present (active ADRs only — skipped for stale)
//! T009: Decision section present (active ADRs only — skipped for stale)
//! T010: Consequences section present (active ADRs only — skipped for stale)
//! T011: Code block exceeds 20 lines (warning)
//! T014: Section ordering — H2 sections in canonical order
//! T015: Section word count range — tier-scaled (configurable base)
//! T016: Tagged rules validation — tier-scaled max count, 7–60 words each (active ADRs only — skipped for stale)
//! T019: Rule-tier tension — rule's layer-derived tier has higher leverage than the ADR tier (`rule_rank` < `adr_rank`)
//! T020: Reference load — tier-scaled max on References: count
//! S004: Stale ADR missing Retirement section
//! S005: Active ADR has Retirement section (location/status mismatch)
//! S006: Terminal-status ADR not in stale directory
//! S007: Stale stub-structure compliance (per AFM-0022) — disallowed
//!       sections or non-lineage relationship verbs in stale stubs

use crate::config::Config;
use crate::model::{AdrRecord, RelVerb, Status, Tier, layer_to_tier};
use crate::report::Diagnostic;

/// Maximum lines in a single fenced code block before T011 fires.
const MAX_CODE_BLOCK_LINES: usize = 20;

/// Default minimum word count for prose sections.
const DEFAULT_MIN_WORDS: u64 = 7;

/// Default maximum word count for prose sections.
const DEFAULT_MAX_WORDS: u64 = 100;

/// Default maximum number of tagged rules per ADR.
const DEFAULT_MAX_RULES: u64 = 10;

/// Default minimum words per tagged rule.
const DEFAULT_MIN_RULE_WORDS: u64 = 7;

/// Default maximum words per tagged rule.
const DEFAULT_MAX_RULE_WORDS: u64 = 60;

/// Canonical H2 section order for active ADRs (with legacy ## Status heading).
const ACTIVE_SECTION_ORDER_WITH_STATUS: &[&str] =
    &["Status", "Related", "Context", "Decision", "Consequences"];

/// Canonical H2 section order for active ADRs (new format — no ## Status heading).
const ACTIVE_SECTION_ORDER: &[&str] = &["Related", "Context", "Decision", "Consequences"];

/// Canonical H2 section order for stale ADRs (with legacy ## Status heading).
const STALE_SECTION_ORDER_WITH_STATUS: &[&str] = &[
    "Status",
    "Related",
    "Context",
    "Decision",
    "Consequences",
    "Retirement",
];

/// Canonical H2 section order for stale ADRs (new format — no ## Status heading).
const STALE_SECTION_ORDER: &[&str] = &[
    "Related",
    "Context",
    "Decision",
    "Consequences",
    "Retirement",
];

/// H2 sections allowed in a stale stub (per AFM-0022 / S007).
///
/// Retired ADRs reduce to preamble + optional `## Related`
/// (containing only `Supersedes:` edges) + `## Retirement`. Any
/// other H2 section is a stub violation.
const STUB_ALLOWED_SECTIONS: &[&str] = &["Related", "Retirement"];

pub fn check(record: &AdrRecord, config: &Config, diags: &mut Vec<Diagnostic>) {
    check_metadata(record, diags);
    check_status_validity(record, diags);
    check_structure(record, diags);
    check_section_order(record, diags);

    // Resolve tier for scaling (default to B when missing — T004 fires separately)
    let tier = record.tier.unwrap_or(Tier::B);

    // T015: Section word count range — tier-scaled
    let base_max_words = config
        .rule_param_u64("T015", "max_words")
        .unwrap_or(DEFAULT_MAX_WORDS);
    let effective_min = tier.min_words();
    // Tier factor ∈ [0.6, 1.5]; base_max_words ≤ low hundreds. Product
    // is well within u64 range; truncation/sign-loss/precision warnings
    // are spurious at these magnitudes. No idiomatic non-cast alternative
    // exists for `f64 → u64` after `.round()`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "tier factor ∈ [0.6,1.5] × low-hundreds; f64.round() → u64 has no idiomatic non-cast alternative"
    )]
    let effective_max = (base_max_words as f64 * tier.factor()).round() as u64;
    check_section_word_counts(record, effective_min, effective_max, tier, diags);

    // T016: Tagged rules in Decision section — tier-scaled max
    let base_max_rules = config
        .rule_param_u64("T016", "max_rules")
        .unwrap_or(DEFAULT_MAX_RULES);
    // See T015 cast rationale above.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "tier factor ∈ [0.6,1.5] × low-hundreds; f64.round() → u64 has no idiomatic non-cast alternative"
    )]
    let effective_max_rules = (base_max_rules as f64 * tier.factor()).round() as u64;
    let min_rule_words = config
        .rule_param_u64("T016", "min_rule_words")
        .unwrap_or(DEFAULT_MIN_RULE_WORDS);
    let max_rule_words = config
        .rule_param_u64("T016", "max_rule_words")
        .unwrap_or(DEFAULT_MAX_RULE_WORDS);
    check_tagged_rules(
        record,
        tier,
        effective_max_rules,
        min_rule_words,
        max_rule_words,
        diags,
    );

    // T019: Rule-tier tension — fire when Meadows layer implies a tier
    // >1 rank from ADR tier (>2 ranks for S-tier ADRs in foundation domains).
    check_rule_tier_tension(record, tier, config, diags);

    // T020: Reference load — tier-scaled limit on References: count
    check_reference_load(record, tier, diags);

    check_stale_lifecycle(record, config, diags);
    check_stale_stub_structure(record, diags);
}

/// T002–T005c: Preamble metadata field checks.
fn check_metadata(record: &AdrRecord, diags: &mut Vec<Diagnostic>) {
    // T002: Date
    if record.date.is_none() {
        diags.push(Diagnostic::warning(
            "T002",
            &record.file_path,
            0,
            "missing `Date:` field".into(),
        ));
    }

    // T003: Last-reviewed — required for all tiers
    if record.last_reviewed.is_none() {
        diags.push(Diagnostic::warning(
            "T003",
            &record.file_path,
            0,
            "missing `Last-reviewed:` field (required for all tiers)".into(),
        ));
    }

    // T004: Tier
    if record.tier.is_none() {
        diags.push(Diagnostic::warning(
            "T004",
            &record.file_path,
            0,
            "missing `Tier:` field".into(),
        ));
    }

    // T005: Status section
    if record.status.is_none() {
        diags.push(Diagnostic::warning(
            "T005",
            &record.file_path,
            0,
            "missing `## Status` section or `Status:` metadata field".into(),
        ));
    }

    // T005c: Legacy ## Status section — migrate to preamble field
    if record.status_from_section {
        diags.push(Diagnostic::warning(
            "T005c",
            &record.file_path,
            record.status_line,
            "status uses legacy `## Status` section — migrate to \
             `Status:` preamble metadata field (e.g., `Status: Accepted`)"
                .into(),
        ));
    }
}

/// T006–T007: Status value and relationship validity checks.
fn check_status_validity(record: &AdrRecord, diags: &mut Vec<Diagnostic>) {
    // T006: Status value validity — strict keyword
    if let Some(ref raw) = record.status_raw {
        if Status::has_parenthetical(raw) {
            diags.push(Diagnostic::warning(
                "T006",
                &record.file_path,
                record.status_line,
                format!(
                    "status line contains parenthetical annotation: `{raw}` — \
                     remove annotations, use a valid status keyword"
                ),
            ));
        }
        if let Some(Status::Invalid(ref s)) = record.status {
            diags.push(Diagnostic::warning(
                "T006",
                &record.file_path,
                record.status_line,
                format!(
                    "unrecognized status: `{s}` — expected one of: \
                     Draft, Proposed, Accepted, Rejected, Deprecated, \
                     Superseded by PREFIX-NNNN"
                ),
            ));
        }
    }

    // T007: Related section — must have at least one relationship.
    // Skipped for stale stubs (per AFM-0022): a stale stub may legally
    // omit `## Related` entirely (status field already records the
    // successor) or carry a single `Supersedes:` edge. S007 governs
    // the stub form positively.
    if record.is_stale {
        return;
    }
    if !record.has_related {
        diags.push(Diagnostic::warning(
            "T007",
            &record.file_path,
            0,
            "missing `## Related` section".into(),
        ));
    } else if record.relationships.is_empty() {
        diags.push(Diagnostic::warning(
            "T007",
            &record.file_path,
            0,
            "Related section has no relationships — every ADR must \
             have at least one relation (use `Root: ID` for tree roots)"
                .into(),
        ));
    }
}

/// T008–T011: Required sections and code block checks.
///
/// T008/T009/T010 (`## Context`, `## Decision`, `## Consequences`)
/// are skipped for stale ADRs: per AFM-0022, stale stubs reduce to
/// preamble + optional `## Related` + `## Retirement`, so requiring
/// the active-ADR triple would always fire on compliant stubs.
/// S007 enforces stub structure positively. T011 (code block size)
/// applies to every ADR — stubs rarely contain code, so it is
/// effectively dormant on stale.
fn check_structure(record: &AdrRecord, diags: &mut Vec<Diagnostic>) {
    // T008: Context section (active ADRs only)
    if !record.is_stale && !record.has_context {
        diags.push(Diagnostic::warning(
            "T008",
            &record.file_path,
            0,
            "missing `## Context` section".into(),
        ));
    }

    // T009: Decision section (active ADRs only)
    if !record.is_stale && !record.has_decision {
        diags.push(Diagnostic::warning(
            "T009",
            &record.file_path,
            0,
            "missing `## Decision` section".into(),
        ));
    }

    // T010: Consequences section (active ADRs only)
    if !record.is_stale && !record.has_consequences {
        diags.push(Diagnostic::warning(
            "T010",
            &record.file_path,
            0,
            "missing `## Consequences` section".into(),
        ));
    }

    // T011: Code block length
    if record.max_code_block_lines > MAX_CODE_BLOCK_LINES {
        diags.push(Diagnostic::warning(
            "T011",
            &record.file_path,
            record.max_code_block_line,
            format!(
                "code block has {} lines (max {}). \
                 Use signatures or pseudocode; reference source files \
                 for full implementations.",
                record.max_code_block_lines, MAX_CODE_BLOCK_LINES,
            ),
        ));
    }
}

/// S004–S006: Stale/active lifecycle alignment checks.
fn check_stale_lifecycle(record: &AdrRecord, config: &Config, diags: &mut Vec<Diagnostic>) {
    // S004: Stale ADR must have Retirement section
    if record.is_stale && !record.has_retirement {
        diags.push(Diagnostic::warning(
            "S004",
            &record.file_path,
            0,
            "stale ADR missing `## Retirement` section — explain why \
             this ADR was retired"
                .into(),
        ));
    }

    // S005: Active ADR must NOT have Retirement section
    if !record.is_stale && record.has_retirement {
        diags.push(Diagnostic::warning(
            "S005",
            &record.file_path,
            0,
            "active ADR has `## Retirement` section — Retirement is \
             only for stale ADRs"
                .into(),
        ));
    }

    // S006: Terminal-status ADR not in stale directory
    if let Some(ref status) = record.status
        && status.is_terminal()
        && !record.is_stale
    {
        let status_display = match status {
            Status::Rejected => "Rejected".to_string(),
            Status::Deprecated => "Deprecated".to_string(),
            Status::SupersededBy(id) => format!("Superseded by {id}"),
            _ => format!("{status:?}"),
        };
        let min_words = config
            .rule_param_u64("T015", "min_words")
            .unwrap_or(DEFAULT_MIN_WORDS);
        diags.push(Diagnostic::warning(
            "S006",
            &record.file_path,
            record.status_line,
            format!(
                "{} has terminal status '{status_display}' but is not in the \
                 stale directory. Action: move this file to {stale_dir}/ and add a \
                 `## Retirement` section (≥{min_words} words) explaining why this \
                 ADR left active service.",
                record.id,
                stale_dir = config.stale.directory,
            ),
        ));
    }
}

/// S007: Stale stub-structure compliance (per AFM-0022).
///
/// A stale ADR carrying a terminal status (Rejected, Deprecated,
/// or Superseded by …) must reduce to a stub: preamble, optional
/// `## Related` containing only `Supersedes:` edges (forward
/// lineage; the reverse direction is already recorded in the
/// `Status:` field), and a `## Retirement` section. Any other H2
/// section (`## Context`, `## Decision`, `## Consequences`, etc.)
/// or any non-`Supersedes` relationship verb emits one warning per
/// occurrence.
///
/// Missing `## Retirement` is covered by S004 — not duplicated here.
/// Non-terminal stale ADRs (status = Accepted, Draft, Proposed)
/// do not match the rule conditions and produce no S007 diagnostics.
fn check_stale_stub_structure(record: &AdrRecord, diags: &mut Vec<Diagnostic>) {
    if !record.is_stale {
        return;
    }
    let Some(status) = record.status.as_ref() else {
        return;
    };
    if !status.is_terminal() {
        return;
    }

    // Whitelist of H2 sections allowed in a stub.
    for section in &record.section_order {
        if !STUB_ALLOWED_SECTIONS.contains(&section.as_str()) {
            diags.push(Diagnostic::warning(
                "S007",
                &record.file_path,
                0,
                format!(
                    "stale stub must not contain `## {section}` — \
                     terminal-state ADRs reduce to preamble + optional \
                     `## Related` (lineage edges only) + `## Retirement`. \
                     Delete this section; the prior content remains in git \
                     history. (See AFM-0022.)"
                ),
            ));
        }
    }

    // Permitted lineage verb in a stub's `## Related` section.
    for rel in &record.relationships {
        if !matches!(rel.verb, RelVerb::Supersedes) {
            diags.push(Diagnostic::warning(
                "S007",
                &record.file_path,
                rel.line,
                format!(
                    "stale stub `## Related` must contain only `Supersedes:` \
                     edges (the reverse direction is recorded in `Status:`); \
                     found `{verb}: {target}`. Remove this edge — non-lineage \
                     citations on retired ADRs pollute the active reference \
                     graph. (See AFM-0022.)",
                    verb = rel.verb,
                    target = rel.target,
                ),
            ));
        }
    }
}

/// T014: H2 sections must appear in canonical order.
///
/// Only validates the relative ordering of known canonical sections.
/// Extra subsections (e.g., `### Rules`) within a section are ignored.
/// Dynamically selects expected order based on whether `## Status` is
/// present (legacy format) or absent (new metadata-field format).
fn check_section_order(record: &AdrRecord, diags: &mut Vec<Diagnostic>) {
    let has_status_section = record.section_order.iter().any(|s| s == "Status");

    let expected: &[&str] = match (record.is_stale, has_status_section) {
        (true, true) => STALE_SECTION_ORDER_WITH_STATUS,
        (true, false) => STALE_SECTION_ORDER,
        (false, true) => ACTIVE_SECTION_ORDER_WITH_STATUS,
        (false, false) => ACTIVE_SECTION_ORDER,
    };

    // Filter section_order to only canonical sections
    let actual: Vec<&str> = record
        .section_order
        .iter()
        .map(String::as_str)
        .filter(|s| expected.contains(s))
        .collect();

    // Check that canonical sections appear in order
    let mut expected_iter = expected.iter();
    for actual_section in &actual {
        // Advance expected_iter to find this section
        let mut found = false;
        for expected_section in expected_iter.by_ref() {
            if actual_section == expected_section {
                found = true;
                break;
            }
        }
        if !found {
            diags.push(Diagnostic::warning(
                "T014",
                &record.file_path,
                0,
                format!(
                    "section `## {actual_section}` is out of canonical order — \
                     expected: {}",
                    expected.join(" → "),
                ),
            ));
            return; // One diagnostic is enough
        }
    }
}

/// T015: Prose sections must meet word count range.
///
/// Applies to Context, Consequences, and Retirement only.
/// Decision section is validated by T016 (rule count, not word count).
/// Min and max are tier-scaled: higher-tier ADRs need more substance,
/// lower-tier ADRs should be tighter.
fn check_section_word_counts(
    record: &AdrRecord,
    min_words: u64,
    max_words: u64,
    tier: Tier,
    diags: &mut Vec<Diagnostic>,
) {
    let prose_sections = ["Context", "Consequences"];

    for section in &prose_sections {
        if let Some(&count) = record.section_word_counts.get(*section) {
            if (count as u64) < min_words {
                diags.push(Diagnostic::warning(
                    "T015",
                    &record.file_path,
                    0,
                    format!(
                        "`## {section}` has {count} word(s) ({tier}-tier minimum {min_words}) — \
                         provide more context"
                    ),
                ));
            } else if (count as u64) > max_words {
                diags.push(Diagnostic::warning(
                    "T015",
                    &record.file_path,
                    0,
                    format!(
                        "`## {section}` has {count} word(s) ({tier}-tier limit {max_words}) — \
                         consider tightening prose, splitting, or re-tiering"
                    ),
                ));
            }
        }
    }

    // Retirement section word count range (if present)
    if record.has_retirement
        && let Some(&count) = record.section_word_counts.get("Retirement")
    {
        if (count as u64) < min_words {
            diags.push(Diagnostic::warning(
                "S004",
                &record.file_path,
                0,
                format!(
                    "`## Retirement` has {count} word(s) ({tier}-tier minimum {min_words}) — \
                     explain why this ADR was retired"
                ),
            ));
        } else if (count as u64) > max_words {
            diags.push(Diagnostic::warning(
                "T015",
                &record.file_path,
                0,
                format!(
                    "`## Retirement` has {count} word(s) ({tier}-tier limit {max_words}) — \
                     be concise"
                ),
            ));
        }
    }
}

/// T016: Tagged rules validation in Decision section.
///
/// Checks:
/// - At least one tagged rule present (all statuses)
/// - Sequential IDs (R1, R2, R3 — no gaps)
/// - Maximum rule count (tier-scaled)
/// - Word count per rule (default 7-60)
/// - Layer range: 1-12 (Meadows leverage points)
///
/// Skipped for stale stubs (per AFM-0022): stubs have no `## Decision`
/// section, so requiring tagged rules would always fire on compliant
/// stubs. The original tagged rules remain accessible via git history.
fn check_tagged_rules(
    record: &AdrRecord,
    tier: Tier,
    max_rules: u64,
    min_rule_words: u64,
    max_rule_words: u64,
    diags: &mut Vec<Diagnostic>,
) {
    if record.is_stale {
        return;
    }
    // Check for missing tagged rules
    if record.decision_rules.is_empty() {
        diags.push(Diagnostic::warning(
            "T016",
            &record.file_path,
            0,
            "Decision section lacks tagged rules (RN [L]: pattern)".into(),
        ));
        return;
    }

    // Check maximum rule count (tier-scaled)
    if record.decision_rules.len() as u64 > max_rules {
        diags.push(Diagnostic::warning(
            "T016",
            &record.file_path,
            0,
            format!(
                "Decision section has {} tagged rules ({tier}-tier limit {max_rules}) — \
                 some tension is expected; consider splitting or re-tiering if scope is broad",
                record.decision_rules.len(),
            ),
        ));
    }

    // Check per-rule word bounds and layer validity
    for rule in &record.decision_rules {
        let word_count = rule.text.split_whitespace().count() as u64;
        if word_count < min_rule_words {
            diags.push(Diagnostic::warning(
                "T016",
                &record.file_path,
                rule.line,
                format!(
                    "Rule {id} has {word_count} word(s) (minimum {min_rule_words})",
                    id = rule.id,
                ),
            ));
        } else if word_count > max_rule_words {
            diags.push(Diagnostic::warning(
                "T016",
                &record.file_path,
                rule.line,
                format!(
                    "Rule {id} has {word_count} word(s) (maximum {max_rule_words}) — be concise",
                    id = rule.id,
                ),
            ));
        }

        // Layer range validation: must be 1-12 (Meadows leverage points)
        if rule.layer == 0 || rule.layer > 12 {
            diags.push(Diagnostic::warning(
                "T016",
                &record.file_path,
                rule.line,
                format!(
                    "Rule {id} has layer {layer} (must be 1-12, Meadows leverage points)",
                    id = rule.id,
                    layer = rule.layer,
                ),
            ));
        }
    }

    // Check for non-sequential IDs
    let mut nums: Vec<u32> = Vec::new();
    for rule in &record.decision_rules {
        if let Some(num_str) = rule.id.strip_prefix('R')
            && let Ok(num) = num_str.parse::<u32>()
        {
            nums.push(num);
        }
    }

    nums.sort_unstable();
    for (i, &num) in nums.iter().enumerate() {
        let expected = u32::try_from(i).expect("rule count fits u32") + 1;
        if num != expected {
            let prev = if i > 0 {
                format!("R{}", nums[i - 1])
            } else {
                "start".into()
            };
            diags.push(Diagnostic::warning(
                "T016",
                &record.file_path,
                0,
                format!("Tagged rule IDs not sequential (gap after {prev})"),
            ));
            return;
        }
    }
}

/// T019: Rule-tier tension — fires iff the rule's layer-derived tier has higher
/// leverage than the ADR's tier (`rule_rank < adr_rank`).
///
/// Under the asymmetric rule: a rule operating at a *higher* leverage layer
/// than its ADR warrants is a structural mismatch (e.g. an S-tier rule in a
/// D-tier ADR signals the rule should live in a more authoritative document).
/// A rule at equal or lower leverage than its ADR tier is intentional and passes
/// silently — lower-leverage enforcement within a higher-leverage decision is
/// expected (e.g. a C-tier rule in an S-tier ADR).
///
/// No domain-specific carve-outs; the asymmetric bound applies uniformly.
fn check_rule_tier_tension(
    record: &AdrRecord,
    adr_tier: Tier,
    config: &Config,
    diags: &mut Vec<Diagnostic>,
) {
    let _ = config; // config not needed for asymmetric rule; retained for signature stability
    let adr_rank = adr_tier.rank();

    for rule in &record.decision_rules {
        let Some(rule_tier) = layer_to_tier(rule.layer) else {
            continue; // Invalid layer already caught by T016
        };
        let rule_rank = rule_tier.rank();
        if rule_rank < adr_rank {
            let distance = adr_rank - rule_rank;
            diags.push(Diagnostic::warning(
                "T019",
                &record.file_path,
                rule.line,
                format!(
                    "Rule {} at layer {} ({rule_tier:?}-tier) is {distance} tiers \
                     from ADR tier {adr_tier} — tension may be intentional; \
                     consider adjusting layer, splitting rule to a {rule_tier:?}-tier ADR, \
                     or re-tiering this ADR",
                    rule.id, rule.layer,
                ),
            ));
        }
    }
}

/// T020: Reference load — tier-scaled limit on `References:` count.
///
/// Only `References:` targets count toward load. `Root:` and `Supersedes:`
/// are structural relationships, not content dependencies. High reference
/// count signals broad scope that may warrant splitting.
fn check_reference_load(record: &AdrRecord, tier: Tier, diags: &mut Vec<Diagnostic>) {
    use crate::model::RelVerb;

    let ref_count = record
        .relationships
        .iter()
        .filter(|r| r.verb == RelVerb::References)
        .count();

    let max_refs = tier.max_refs();
    if ref_count > max_refs {
        diags.push(Diagnostic::warning(
            "T020",
            &record.file_path,
            0,
            format!(
                "{ref_count} references ({tier}-tier limit {max_refs}) — \
                 may indicate broad scope; consider splitting or promoting to a higher tier",
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AdrId, Tier};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_config() -> Config {
        toml::from_str(
            r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []

[[domains]]
prefix = "GND"
name = "Ground"
directory = "ground"
description = "Foundation test"
crates = []
foundation = true

[[rules]]
id = "T015"
params = { min_words = 7, max_words = 50 }

[[rules]]
id = "T016"
params = { max_rules = 10, min_rule_words = 7, max_rule_words = 60 }
"#,
        )
        .unwrap()
    }

    fn make_record() -> AdrRecord {
        let mut word_counts = HashMap::new();
        word_counts.insert("Context".into(), 15);
        word_counts.insert("Decision".into(), 15);
        word_counts.insert("Consequences".into(), 15);

        AdrRecord {
            id: AdrId {
                prefix: "CHE".into(),
                number: 1,
            },
            file_path: PathBuf::from("test.md"),
            title: Some("Test".into()),
            title_line: 1,
            date: Some("2026-04-25".into()),
            last_reviewed: Some("2026-04-25".into()),
            tier: Some(Tier::S),
            status: Some(Status::Accepted),
            status_line: 8,
            status_raw: Some("Accepted".into()),
            has_related: true,
            has_context: true,
            has_decision: true,
            has_consequences: true,
            section_order: vec![
                "Related".into(),
                "Context".into(),
                "Decision".into(),
                "Consequences".into(),
            ],
            section_word_counts: word_counts,
            ..AdrRecord::default()
        }
    }

    #[test]
    fn valid_record_produces_no_diagnostics() {
        use crate::model::{RelVerb, Relationship, TaggedRule};
        let mut record = make_record();
        record.tier = Some(Tier::B); // B-tier so layer 5 aligns (no T019)
        record.relationships = vec![Relationship {
            verb: RelVerb::Root,
            target: record.id.clone(),
            line: 10,
        }];
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers".into(),
            line: 10,
            layer: 5,
        }];

        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(diags.is_empty(), "expected no diags, got: {diags:?}");
    }

    #[test]
    fn missing_tier_produces_t004() {
        let mut record = make_record();
        record.tier = None;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(diags.iter().any(|d| d.rule == "T004"));
    }

    #[test]
    fn missing_last_reviewed_all_tiers_is_warning() {
        for tier in [Tier::S, Tier::A, Tier::B, Tier::C, Tier::D] {
            let mut record = make_record();
            record.tier = Some(tier);
            record.last_reviewed = None;
            let config = make_config();
            let mut diags = Vec::new();
            check(&record, &config, &mut diags);
            assert!(
                diags.iter().any(|d| d.rule == "T003"),
                "expected T003 for tier {tier:?}"
            );
        }
    }

    #[test]
    fn parenthetical_status_produces_t006() {
        let mut record = make_record();
        record.status_raw = Some("Accepted (supersedes original u64 design)".into());
        record.status = Some(Status::Invalid(
            "Accepted (supersedes original u64 design)".into(),
        ));
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T006"),
            "expected T006, got: {diags:?}"
        );
    }

    #[test]
    fn amended_status_produces_t006() {
        let mut record = make_record();
        record.status_raw = Some("Amended 2026-04-25 — note".into());
        record.status = Some(Status::Invalid("Amended 2026-04-25 — note".into()));
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T006"),
            "Amended should trigger T006 as invalid, got: {diags:?}"
        );
    }

    #[test]
    fn empty_related_produces_t007() {
        let mut record = make_record();
        record.has_related = true;
        record.relationships = vec![];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T007"),
            "empty Related should trigger T007, got: {diags:?}"
        );
    }

    #[test]
    fn related_with_relationship_no_t007() {
        use crate::model::{RelVerb, Relationship};
        let mut record = make_record();
        record.relationships = vec![Relationship {
            verb: RelVerb::Root,
            target: record.id.clone(),
            line: 10,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T007"),
            "Related with relationship should not trigger T007"
        );
    }

    #[test]
    fn code_block_at_limit_no_t011() {
        let mut record = make_record();
        record.max_code_block_lines = 20;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T011"),
            "20 lines should not trigger T011"
        );
    }

    #[test]
    fn code_block_over_limit_produces_t011() {
        let mut record = make_record();
        record.max_code_block_lines = 21;
        record.max_code_block_line = 42;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t011 = diags.iter().find(|d| d.rule == "T011");
        assert!(t011.is_some(), "expected T011, got: {diags:?}");
        assert_eq!(t011.unwrap().line, 42, "T011 should point to opening fence");
    }

    #[test]
    fn section_out_of_order_produces_t014() {
        let mut record = make_record();
        record.section_order = vec![
            "Context".into(), // out of order — Related should come first
            "Related".into(),
            "Decision".into(),
            "Consequences".into(),
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T014"),
            "out-of-order sections should trigger T014, got: {diags:?}"
        );
    }

    #[test]
    fn section_correct_order_no_t014() {
        let record = make_record();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T014"),
            "correct order should not trigger T014, got: {diags:?}"
        );
    }

    #[test]
    fn section_correct_order_with_legacy_status_no_t014() {
        let mut record = make_record();
        record.section_order = vec![
            "Status".into(),
            "Related".into(),
            "Context".into(),
            "Decision".into(),
            "Consequences".into(),
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T014"),
            "correct legacy order should not trigger T014, got: {diags:?}"
        );
    }

    #[test]
    fn section_too_few_words_produces_t015() {
        let mut record = make_record();
        record.section_word_counts.insert("Context".into(), 3);
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T015"),
            "3 words should trigger T015, got: {diags:?}"
        );
    }

    #[test]
    fn section_too_many_words_produces_t015() {
        let mut record = make_record();
        record.tier = Some(Tier::B); // B-tier: factor 1.0, max=50
        record.section_word_counts.insert("Context".into(), 60);
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t015 = diags
            .iter()
            .find(|d| d.rule == "T015" && d.message.contains("limit"));
        assert!(
            t015.is_some(),
            "60 words should trigger T015 max, got: {diags:?}"
        );
    }

    #[test]
    fn section_within_range_no_t015() {
        let record = make_record(); // all sections have 15 words
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T015"),
            "15 words should not trigger T015, got: {diags:?}"
        );
    }

    #[test]
    fn stale_adr_without_retirement_produces_s004() {
        let mut record = make_record();
        record.is_stale = true;
        record.has_retirement = false;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "S004"),
            "stale without Retirement should trigger S004, got: {diags:?}"
        );
    }

    #[test]
    fn stale_adr_with_retirement_no_s004() {
        let mut record = make_record();
        record.is_stale = true;
        record.has_retirement = true;
        record.section_word_counts.insert("Retirement".into(), 15);
        record.section_order = vec![
            "Related".into(),
            "Context".into(),
            "Decision".into(),
            "Consequences".into(),
            "Retirement".into(),
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s004_existence: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "S004" && d.message.contains("missing"))
            .collect();
        assert!(
            s004_existence.is_empty(),
            "stale with Retirement should not trigger S004 existence check"
        );
    }

    #[test]
    fn active_adr_with_retirement_produces_s005() {
        let mut record = make_record();
        record.is_stale = false;
        record.has_retirement = true;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "S005"),
            "active with Retirement should trigger S005, got: {diags:?}"
        );
    }

    #[test]
    fn rejected_in_active_dir_produces_s006() {
        let mut record = make_record();
        record.status = Some(Status::Rejected);
        record.status_raw = Some("Rejected".into());
        record.is_stale = false;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s006 = diags.iter().find(|d| d.rule == "S006");
        assert!(s006.is_some(), "Rejected in active dir should trigger S006");
        assert!(
            s006.unwrap().message.contains("Action:"),
            "S006 message must contain actionable instructions"
        );
    }

    #[test]
    fn superseded_in_active_dir_produces_s006() {
        let mut record = make_record();
        record.status = Some(Status::SupersededBy(AdrId {
            prefix: "CHE".into(),
            number: 99,
        }));
        record.status_raw = Some("Superseded by CHE-0099".into());
        record.is_stale = false;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s006 = diags.iter().find(|d| d.rule == "S006");
        assert!(
            s006.is_some(),
            "Superseded in active dir should trigger S006"
        );
    }

    #[test]
    fn accepted_in_active_dir_no_s006() {
        let record = make_record();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "S006"),
            "Accepted in active dir should NOT trigger S006"
        );
    }

    #[test]
    fn tagged_rules_present_no_t016() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.decision_rules = vec![
            TaggedRule {
                id: "R1".into(),
                text: "All events must be versioned with semantic version numbers always".into(),
                line: 10,
                layer: 5,
            },
            TaggedRule {
                id: "R2".into(),
                text: "Snapshots are created at one hundred event intervals minimum always".into(),
                line: 11,
                layer: 5,
            },
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T016"),
            "tagged rules should not trigger T016, got: {diags:?}"
        );
    }

    #[test]
    fn no_tagged_rules_produces_t016() {
        let mut record = make_record();
        record.decision_rules = vec![];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T016"),
            "missing tagged rules should trigger T016, got: {diags:?}"
        );
    }

    #[test]
    fn empty_rules_produces_t016() {
        let mut record = make_record();
        record.decision_rules = vec![];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T016"),
            "empty rules should trigger T016, got: {diags:?}"
        );
    }

    #[test]
    fn draft_not_exempt_from_t016() {
        let mut record = make_record();
        record.status = Some(Status::Draft);
        record.status_raw = Some("Draft".into());
        record.decision_rules = vec![];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T016"),
            "Draft should NOT be exempt from T016, got: {diags:?}"
        );
    }

    #[test]
    fn too_many_rules_produces_t016() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::B); // B-tier: factor 1.0, max_rules=10
        record.decision_rules = (1..=11)
            .map(|i| TaggedRule {
                id: format!("R{i}"),
                text: "This rule has enough words to pass the minimum check here".into(),
                line: 10 + i,
                layer: 5, // B-tier layer — no T019 tension
            })
            .collect();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016_max = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("limit"));
        assert!(
            t016_max.is_some(),
            "11 rules should trigger T016 max (B-tier limit 10), got: {diags:?}"
        );
    }

    #[test]
    fn ten_rules_within_limit() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::B); // B-tier: factor 1.0, max_rules=10
        record.decision_rules = (1..=10)
            .map(|i| TaggedRule {
                id: format!("R{i}"),
                text: "This rule has enough words to pass the minimum check here".into(),
                line: 10 + i,
                layer: 5, // B-tier layer — no T019 tension
            })
            .collect();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016_max = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("limit"));
        assert!(
            t016_max.is_none(),
            "10 rules should not trigger T016 max, got: {diags:?}"
        );
    }

    #[test]
    fn rule_too_few_words_produces_t016() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "Too short".into(), // 2 words
            line: 10,
            layer: 5,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016 = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("minimum"));
        assert!(
            t016.is_some(),
            "2-word rule should trigger T016 min, got: {diags:?}"
        );
    }

    #[test]
    fn rule_too_many_words_produces_t016() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        let long_text = (0..61).map(|_| "word").collect::<Vec<_>>().join(" ");
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: long_text,
            line: 10,
            layer: 5,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016 = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("maximum"));
        assert!(
            t016.is_some(),
            "61-word rule should trigger T016 max (limit 60), got: {diags:?}"
        );
    }

    #[test]
    fn sixty_word_rule_within_limit() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        let text = (0..60).map(|_| "word").collect::<Vec<_>>().join(" ");
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text,
            line: 10,
            layer: 5,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016 = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("maximum"));
        assert!(
            t016.is_none(),
            "60-word rule should not trigger T016 max, got: {diags:?}"
        );
    }

    #[test]
    fn non_sequential_ids_produces_t016() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.decision_rules = vec![
            TaggedRule {
                id: "R1".into(),
                text: "This rule has enough words to pass the minimum check here".into(),
                line: 10,
                layer: 5,
            },
            TaggedRule {
                id: "R3".into(),
                text: "This rule also has enough words to pass the minimum check".into(),
                line: 12,
                layer: 5,
            },
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016 = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("gap"));
        assert!(t016.is_some(), "gap in IDs should trigger T016");
    }

    #[test]
    fn legacy_status_section_produces_t005c() {
        let mut record = make_record();
        record.status_from_section = true;
        record.section_order = vec![
            "Status".into(),
            "Related".into(),
            "Context".into(),
            "Decision".into(),
            "Consequences".into(),
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t005c = diags.iter().find(|d| d.rule == "T005c");
        assert!(
            t005c.is_some(),
            "legacy ## Status section should produce T005c, got: {diags:?}"
        );
        assert!(
            t005c.unwrap().message.contains("migrate"),
            "T005c message should mention migration"
        );
    }

    #[test]
    fn metadata_status_field_no_t005c() {
        use crate::model::{RelVerb, Relationship, TaggedRule};
        let mut record = make_record();
        record.status_from_section = false;
        record.relationships = vec![Relationship {
            verb: RelVerb::Root,
            target: record.id.clone(),
            line: 10,
        }];
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers".into(),
            line: 10,
            layer: 5,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T005c"),
            "metadata field format should not produce T005c, got: {diags:?}"
        );
    }

    #[test]
    fn no_status_anywhere_no_t005c() {
        let mut record = make_record();
        record.status = None;
        record.status_raw = None;
        record.status_from_section = false;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T005"),
            "missing status should produce T005, got: {diags:?}"
        );
        assert!(
            !diags.iter().any(|d| d.rule == "T005c"),
            "missing status should not produce T005c, got: {diags:?}"
        );
    }

    // ── T015 tier-scaling tests ────────────────────────────────────

    #[test]
    fn t015_s_tier_allows_more_words() {
        let mut record = make_record();
        record.tier = Some(Tier::S); // factor 1.5, max=75
        record.section_word_counts.insert("Context".into(), 70);
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T015"),
            "70 words should be within S-tier limit (75), got: {diags:?}"
        );
    }

    #[test]
    fn t015_d_tier_tighter_limit() {
        let mut record = make_record();
        record.tier = Some(Tier::D); // factor 0.6, max=30
        record.section_word_counts.insert("Context".into(), 35);
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t015 = diags
            .iter()
            .find(|d| d.rule == "T015" && d.message.contains("D-tier"));
        assert!(
            t015.is_some(),
            "35 words should trigger T015 at D-tier (limit 30), got: {diags:?}"
        );
    }

    #[test]
    fn t015_s_tier_higher_minimum() {
        let mut record = make_record();
        record.tier = Some(Tier::S); // min_words=15
        record.section_word_counts.insert("Context".into(), 10);
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t015 = diags
            .iter()
            .find(|d| d.rule == "T015" && d.message.contains("S-tier minimum"));
        assert!(
            t015.is_some(),
            "10 words should trigger T015 min at S-tier (min 15), got: {diags:?}"
        );
    }

    // ── T016 tier-scaling tests ────────────────────────────────────

    #[test]
    fn t016_d_tier_fewer_rules_allowed() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::D); // factor 0.6, max_rules=6
        record.decision_rules = (1..=7)
            .map(|i| TaggedRule {
                id: format!("R{i}"),
                text: "This rule has enough words to pass the minimum check here".into(),
                line: 10 + i,
                layer: 10, // D-tier layer — no T019 tension
            })
            .collect();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t016 = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("D-tier"));
        assert!(
            t016.is_some(),
            "7 rules should trigger T016 at D-tier (limit 6), got: {diags:?}"
        );
    }

    // ── T016 layer validation tests ────────────────────────────────

    #[test]
    fn t016_layer_zero_is_warning() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 0,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let layer_diag = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("layer 0"));
        assert!(
            layer_diag.is_some(),
            "layer=0 should produce T016 warning, got: {diags:?}"
        );
        assert_eq!(
            layer_diag.unwrap().severity,
            crate::report::Severity::Warning,
            "layer validation must be warning severity per AFM-0003"
        );
    }

    #[test]
    fn t016_layer_thirteen_is_warning() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 13,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let layer_diag = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("layer 13"));
        assert!(
            layer_diag.is_some(),
            "layer=13 should produce T016 warning, got: {diags:?}"
        );
        assert_eq!(
            layer_diag.unwrap().severity,
            crate::report::Severity::Warning,
            "layer validation must be warning severity per AFM-0003"
        );
    }

    #[test]
    fn t016_layer_valid_no_error() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 5,
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let layer_err = diags
            .iter()
            .find(|d| d.rule == "T016" && d.message.contains("layer"));
        assert!(
            layer_err.is_none(),
            "layer=5 should not produce layer error, got: {diags:?}"
        );
    }

    #[test]
    fn t016_layer_boundary_one_and_twelve_pass() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        // Use D-tier so layers 1 and 12 don't both trigger T019;
        // layer 12 → D (distance 0), layer 1 → S (distance 4 fires T019,
        // but we only check T016 layer errors, not T019).
        record.tier = Some(Tier::D);
        record.decision_rules = vec![
            TaggedRule {
                id: "R1".into(),
                text: "All events must be versioned with semantic version numbers always".into(),
                line: 10,
                layer: 1,
            },
            TaggedRule {
                id: "R2".into(),
                text: "All events must be versioned with semantic version numbers always".into(),
                line: 11,
                layer: 12,
            },
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let layer_errs: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "T016" && d.message.contains("layer"))
            .collect();
        assert!(
            layer_errs.is_empty(),
            "layers 1 and 12 are valid boundaries, got: {layer_errs:?}"
        );
    }

    // ── T019 rule-tier tension tests ───────────────────────────────

    #[test]
    fn t019_aligned_rules_no_warning() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::B);
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 5, // B-tier layer, B-tier ADR → distance 0
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T019"),
            "aligned rules should not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_equal_tier_rank_passes() {
        // New asymmetric rule: rule_rank < adr_rank triggers. Equal rank passes.
        // A-tier ADR (rank 1) + L4/A-tier rule (rank 1): 1 < 1? No → passes.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::A); // rank 1
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 4, // A-tier layer, rank 1 → 1 < 1? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T019"),
            "equal-tier must not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_lower_leverage_rule_passes() {
        // rule_rank > adr_rank (rule at lower leverage than ADR) → passes.
        // A-tier ADR (rank 1) + L7/C-tier rule (rank 3): 3 < 1? No → passes.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::A); // rank 1
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 7, // C-tier layer, rank 3 → 3 < 1? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T019"),
            "lower-leverage rule must not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_higher_leverage_rule_fires() {
        // rule_rank < adr_rank (rule at higher leverage than ADR) → fires.
        // D-tier ADR (rank 4) + L1/S-tier rule (rank 0): 0 < 4? Yes → fires.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::D); // rank 4
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 1, // S-tier layer, rank 0 → 0 < 4? Yes → fires
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T019"),
            "higher-leverage rule must trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_adjacent_tier_higher_leverage_fires() {
        // A-tier ADR (rank 1) + L4/A-tier rule (rank 1): 1 < 1? No → passes.
        // A-tier ADR (rank 1) + L1/S-tier rule (rank 0): 0 < 1? Yes → fires.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::B); // rank 2
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 4, // A-tier layer, rank 1 → 1 < 2? YES → fires
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T019"),
            "rule at higher leverage than ADR tier must trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_large_distance_produces_warning() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::D); // rank 4
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 1, // S-tier layer, rank 0 → distance 4
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t019 = diags.iter().find(|d| d.rule == "T019");
        assert!(
            t019.is_some(),
            "distance 4 should trigger T019, got: {diags:?}"
        );
        assert!(
            t019.unwrap().message.contains("4 tiers"),
            "message should mention distance: {}",
            t019.unwrap().message
        );
    }

    #[test]
    fn t019_distance_two_lower_leverage_no_warning() {
        // S-tier ADR (rank 0) + L5/B-tier rule (rank 2): 2 < 0? No → passes.
        // Rule at lower leverage than ADR tier is allowed under asymmetric rule.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = Some(Tier::S); // rank 0
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 5, // B-tier layer, rank 2 → 2 < 0? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T019"),
            "S-tier ADR with lower-leverage rule must not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_foundation_s_tier_lower_leverage_no_warning() {
        // Under asymmetric rule, foundation carve-out is gone.
        // S-tier ADR (rank 0) + L5/B-tier rule (rank 2): 2 < 0? No → passes.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.id = AdrId {
            prefix: "GND".into(),
            number: 1,
        };
        record.tier = Some(Tier::S); // rank 0
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 5, // B-tier layer, rank 2 → 2 < 0? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t019 = diags.iter().find(|d| d.rule == "T019");
        assert!(
            t019.is_none(),
            "foundation S-tier at lower leverage must not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_s_tier_rule_in_c_tier_foundation_adr_lower_leverage_no_warning() {
        // Under asymmetric rule, a C-tier foundation ADR + L7/C-tier rule passes
        // (C rank 3, rule rank 3: 3 < 3? No). Any rule at equal or lower leverage passes.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.id = AdrId {
            prefix: "GND".into(),
            number: 1,
        };
        record.tier = Some(Tier::C); // rank 3
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 9, // D-tier layer, rank 4 → 4 < 3? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t019 = diags.iter().find(|d| d.rule == "T019");
        assert!(
            t019.is_none(),
            "C-tier foundation ADR with lower-leverage rule must not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_equal_tier_no_warning() {
        // A-tier ADR (rank 1) + L4/A-tier rule (rank 1): 1 < 1? No → passes.
        // Equal leverage is not a violation.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.id = AdrId {
            prefix: "GND".into(),
            number: 1,
        };
        record.tier = Some(Tier::A); // rank 1
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 4, // A-tier layer, rank 1 → 1 < 1? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t019 = diags.iter().find(|d| d.rule == "T019");
        assert!(
            t019.is_none(),
            "equal-tier rule must not trigger T019, got: {diags:?}"
        );
    }

    #[test]
    fn t019_unknown_prefix_no_carve_out_needed() {
        // Under asymmetric rule, domain lookup is irrelevant — no carve-outs.
        // S-tier ADR (rank 0) + L5/B-tier rule (rank 2): 2 < 0? No → passes.
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.id = AdrId {
            prefix: "ZZZ".into(),
            number: 1,
        };
        record.tier = Some(Tier::S); // rank 0
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 5, // B-tier layer, rank 2 → 2 < 0? No → passes
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t019 = diags.iter().find(|d| d.rule == "T019");
        assert!(
            t019.is_none(),
            "S-tier ADR with lower-leverage rule must not trigger T019, got: {diags:?}"
        );
    }

    // ── T020 reference load tests ──────────────────────────────────

    #[test]
    fn t020_within_limit_no_warning() {
        use crate::model::{AdrId, RelVerb, Relationship};
        let mut record = make_record();
        record.tier = Some(Tier::B); // max_refs=7
        record.relationships = (1..=7)
            .map(|i| Relationship {
                verb: RelVerb::References,
                target: AdrId {
                    prefix: "CHE".into(),
                    number: i,
                },
                line: 10 + i as usize,
            })
            .collect();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T020"),
            "7 refs at B-tier (limit 7) should not trigger T020, got: {diags:?}"
        );
    }

    #[test]
    fn t020_over_limit_produces_warning() {
        use crate::model::{AdrId, RelVerb, Relationship};
        let mut record = make_record();
        record.tier = Some(Tier::B); // max_refs=7
        record.relationships = (1..=8)
            .map(|i| Relationship {
                verb: RelVerb::References,
                target: AdrId {
                    prefix: "CHE".into(),
                    number: i,
                },
                line: 10 + i as usize,
            })
            .collect();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t020 = diags.iter().find(|d| d.rule == "T020");
        assert!(
            t020.is_some(),
            "8 refs at B-tier (limit 7) should trigger T020, got: {diags:?}"
        );
    }

    #[test]
    fn t020_root_and_supersedes_not_counted() {
        use crate::model::{AdrId, RelVerb, Relationship};
        let mut record = make_record();
        record.tier = Some(Tier::S); // max_refs=3
        record.relationships = vec![
            Relationship {
                verb: RelVerb::Root,
                target: record.id.clone(),
                line: 10,
            },
            Relationship {
                verb: RelVerb::Supersedes,
                target: AdrId {
                    prefix: "CHE".into(),
                    number: 99,
                },
                line: 11,
            },
            Relationship {
                verb: RelVerb::References,
                target: AdrId {
                    prefix: "CHE".into(),
                    number: 2,
                },
                line: 12,
            },
        ];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T020"),
            "only 1 References: should not trigger T020 at S-tier (limit 3), got: {diags:?}"
        );
    }

    #[test]
    fn t020_s_tier_tight_limit() {
        use crate::model::{AdrId, RelVerb, Relationship};
        let mut record = make_record();
        record.tier = Some(Tier::S); // max_refs=3
        record.relationships = (1..=4)
            .map(|i| Relationship {
                verb: RelVerb::References,
                target: AdrId {
                    prefix: "COM".into(),
                    number: i,
                },
                line: 10 + i as usize,
            })
            .collect();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t020 = diags.iter().find(|d| d.rule == "T020");
        assert!(
            t020.is_some(),
            "4 refs at S-tier (limit 3) should trigger T020, got: {diags:?}"
        );
        assert!(
            t020.unwrap().message.contains("S-tier"),
            "message should mention tier"
        );
    }

    // ── Rounding edge case tests ───────────────────────────────────

    #[test]
    fn t015_fractional_rounding_uses_round_not_floor() {
        // base_max_words=33, D-tier factor=0.6 → 33*0.6=19.8 → round=20
        let mut record = make_record();
        record.tier = Some(Tier::D);
        // 20 words should be within limit (rounded up from 19.8)
        record.section_word_counts.insert("Context".into(), 20);
        let config: Config = toml::from_str(
            r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []

[[rules]]
id = "T015"
params = { min_words = 7, max_words = 33 }

[[rules]]
id = "T016"
params = { max_rules = 10, min_rule_words = 7, max_rule_words = 60 }
"#,
        )
        .unwrap();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags
                .iter()
                .any(|d| d.rule == "T015" && d.message.contains("limit")),
            "20 words should be within D-tier limit (33*0.6=19.8→20), got: {diags:?}"
        );
    }

    #[test]
    fn t015_fractional_rounding_boundary_plus_one_fires() {
        // base_max_words=33, D-tier factor=0.6 → round(19.8)=20 → 21 exceeds
        let mut record = make_record();
        record.tier = Some(Tier::D);
        record.section_word_counts.insert("Context".into(), 21);
        let config: Config = toml::from_str(
            r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Cherry"
directory = "cherry"
description = "Test"
crates = []

[[rules]]
id = "T015"
params = { min_words = 7, max_words = 33 }

[[rules]]
id = "T016"
params = { max_rules = 10, min_rule_words = 7, max_rule_words = 60 }
"#,
        )
        .unwrap();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags
                .iter()
                .any(|d| d.rule == "T015" && d.message.contains("D-tier limit 20")),
            "21 words should trigger T015 at D-tier (limit 20), got: {diags:?}"
        );
    }

    // ── T019 missing tier fallback test ─────────────────────────────

    #[test]
    fn t019_missing_tier_defaults_to_b() {
        use crate::model::TaggedRule;
        let mut record = make_record();
        record.tier = None; // defaults to B (rank 2)
        record.decision_rules = vec![TaggedRule {
            id: "R1".into(),
            text: "All events must be versioned with semantic version numbers always".into(),
            line: 10,
            layer: 1, // S-tier (rank 0) → distance 2 from B → fires
        }];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let t019 = diags.iter().find(|d| d.rule == "T019");
        assert!(
            t019.is_some(),
            "S-tier rule in missing-tier (default B) ADR should trigger T019, got: {diags:?}"
        );
        assert!(
            t019.unwrap().message.contains("2 tiers"),
            "distance should be 2 (B→S): {}",
            t019.unwrap().message
        );
    }

    // ── S007 stale stub-structure tests (per AFM-0022) ──────────────

    /// Build a stub-shaped stale record with terminal status + lineage edge.
    fn make_stale_stub() -> AdrRecord {
        use crate::model::{RelVerb, Relationship};
        let mut record = make_record();
        record.is_stale = true;
        record.has_retirement = true;
        record.has_context = false;
        record.has_decision = false;
        record.has_consequences = false;
        record.section_order = vec!["Related".into(), "Retirement".into()];
        record.section_word_counts.clear();
        record.section_word_counts.insert("Retirement".into(), 30);
        record.status = Some(Status::SupersededBy(AdrId {
            prefix: "CHE".into(),
            number: 2,
        }));
        record.status_raw = Some("Superseded by CHE-0002".into());
        // Per AFM-0022: stub `## Related` carries only `Supersedes:` edges
        // (forward direction). The reverse (Superseded by) lives in Status.
        record.relationships = vec![Relationship {
            verb: RelVerb::Supersedes,
            target: AdrId {
                prefix: "CHE".into(),
                number: 99,
            },
            line: 10,
        }];
        record
    }

    #[test]
    fn s007_clean_stub_produces_no_s007() {
        let record = make_stale_stub();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "S007"),
            "clean stub should not trigger S007, got: {diags:?}"
        );
    }

    #[test]
    fn s007_multiple_supersedes_edges_no_fire() {
        use crate::model::{RelVerb, Relationship};
        let mut record = make_stale_stub();
        // Stub may carry multiple Supersedes edges (chain of retirements).
        record.relationships.push(Relationship {
            verb: RelVerb::Supersedes,
            target: AdrId {
                prefix: "CHE".into(),
                number: 7,
            },
            line: 11,
        });
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "S007"),
            "multiple Supersedes edges are permitted, got: {diags:?}"
        );
    }

    #[test]
    fn s007_superseded_by_verb_fires() {
        use crate::model::{RelVerb, Relationship};
        let mut record = make_stale_stub();
        // Reverse direction (legacy verb) is forbidden in stub Related —
        // it duplicates the Status: field and is a legacy form.
        record.relationships.push(Relationship {
            verb: RelVerb::SupersededBy,
            target: AdrId {
                prefix: "CHE".into(),
                number: 2,
            },
            line: 12,
        });
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s007_verb: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "S007" && d.message.contains("Superseded by"))
            .collect();
        assert_eq!(
            s007_verb.len(),
            1,
            "Superseded-by in stub Related should trigger S007, got: {diags:?}"
        );
    }

    #[test]
    fn s007_disallowed_section_fires() {
        let mut record = make_stale_stub();
        record.section_order = vec!["Related".into(), "Context".into(), "Retirement".into()];
        record.has_context = true;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s007: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "S007" && d.message.contains("## Context"))
            .collect();
        assert_eq!(
            s007.len(),
            1,
            "Context in stub should trigger one S007, got: {diags:?}"
        );
        assert!(
            s007[0].message.contains("AFM-0022"),
            "S007 message must cite AFM-0022: {}",
            s007[0].message
        );
    }

    #[test]
    fn s007_multiple_disallowed_sections_fire_per_section() {
        let mut record = make_stale_stub();
        record.section_order = vec![
            "Related".into(),
            "Context".into(),
            "Decision".into(),
            "Consequences".into(),
            "Retirement".into(),
        ];
        record.has_context = true;
        record.has_decision = true;
        record.has_consequences = true;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s007_section: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "S007" && d.message.contains("must not contain"))
            .collect();
        assert_eq!(
            s007_section.len(),
            3,
            "expected one S007 per disallowed section (Context/Decision/Consequences), got: {diags:?}"
        );
    }

    #[test]
    fn s007_non_canonical_section_name_fires() {
        // Whitelist semantics: any H2 not in {Related, Retirement}
        // triggers S007, not just the well-known Context/Decision/
        // Consequences set. Pins the rule against accidental
        // refactor to a deny-list.
        let mut record = make_stale_stub();
        record.section_order = vec!["Related".into(), "Notes".into(), "Retirement".into()];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s007: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "S007" && d.message.contains("## Notes"))
            .collect();
        assert_eq!(
            s007.len(),
            1,
            "non-canonical `## Notes` in stub should trigger S007, got: {diags:?}"
        );
    }

    #[test]
    fn s007_non_lineage_verb_fires() {
        use crate::model::{RelVerb, Relationship};
        let mut record = make_stale_stub();
        record.relationships.push(Relationship {
            verb: RelVerb::References,
            target: AdrId {
                prefix: "CHE".into(),
                number: 50,
            },
            line: 12,
        });
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        let s007_verb: Vec<_> = diags
            .iter()
            .filter(|d| d.rule == "S007" && d.message.contains("References"))
            .collect();
        assert_eq!(
            s007_verb.len(),
            1,
            "References in stub `## Related` should trigger one S007, got: {diags:?}"
        );
        assert_eq!(
            s007_verb[0].line, 12,
            "diagnostic must point at the relationship's line"
        );
    }

    #[test]
    fn s007_stale_with_accepted_status_no_fire() {
        let mut record = make_stale_stub();
        // Non-terminal status — rule conditions unmet, even with bad sections.
        record.status = Some(Status::Accepted);
        record.status_raw = Some("Accepted".into());
        record.section_order = vec!["Related".into(), "Context".into(), "Retirement".into()];
        record.has_context = true;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "S007"),
            "stale + non-terminal status must not trigger S007 (other rules cover that mismatch), got: {diags:?}"
        );
    }

    #[test]
    fn s007_active_with_terminal_status_no_fire() {
        let mut record = make_record();
        // Terminal status outside stale dir — S006 fires, not S007.
        record.is_stale = false;
        record.status = Some(Status::SupersededBy(AdrId {
            prefix: "CHE".into(),
            number: 2,
        }));
        record.status_raw = Some("Superseded by CHE-0002".into());
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "S007"),
            "active dir + terminal status must not trigger S007 (S006 covers it), got: {diags:?}"
        );
    }

    // ── T-rule stale-skip pin tests (per AFM-0022) ──────────────────

    #[test]
    fn t007_t008_t009_t010_t016_skipped_for_stale_stub() {
        let record = make_stale_stub();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        for rule in ["T007", "T008", "T009", "T010", "T016"] {
            assert!(
                !diags.iter().any(|d| d.rule == rule),
                "{rule} must not fire on stale stub (per AFM-0022), got: {diags:?}"
            );
        }
    }

    #[test]
    fn t007_skipped_for_stale_with_no_related_section() {
        let mut record = make_stale_stub();
        record.has_related = false;
        record.relationships.clear();
        record.section_order = vec!["Retirement".into()];
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "T007"),
            "T007 must not fire on stale stub missing `## Related` (per AFM-0022), got: {diags:?}"
        );
    }

    #[test]
    fn t008_t009_t010_still_fire_on_active_missing_sections() {
        let mut record = make_record();
        record.is_stale = false;
        record.has_context = false;
        record.has_decision = false;
        record.has_consequences = false;
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        for rule in ["T008", "T009", "T010"] {
            assert!(
                diags.iter().any(|d| d.rule == rule),
                "{rule} must still fire on active ADR missing the section, got: {diags:?}"
            );
        }
    }

    #[test]
    fn t007_still_fires_on_active_missing_related() {
        let mut record = make_record();
        record.is_stale = false;
        record.has_related = false;
        record.relationships.clear();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T007"),
            "T007 must still fire on active ADR missing Related, got: {diags:?}"
        );
    }

    #[test]
    fn t016_still_fires_on_active_missing_tagged_rules() {
        let mut record = make_record();
        record.is_stale = false;
        record.decision_rules.clear();
        let config = make_config();
        let mut diags = Vec::new();
        check(&record, &config, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "T016"),
            "T016 must still fire on active ADR missing tagged rules, got: {diags:?}"
        );
    }
}
