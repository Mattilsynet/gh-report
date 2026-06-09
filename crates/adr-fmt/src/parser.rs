//! ADR file parser — extracts metadata from markdown files.
//!
//! Per AFM-0017, parser-stage failures are surfaced through the
//! `P###` diagnostic namespace:
//! - `P001` — unreadable file (filename matches the prefix pattern
//!   but `fs::read_to_string` fails for a non-infrastructure reason
//!   such as transient I/O). The ADR is excluded from rule checks.
//! - `P002` — missing or malformed H1 title (filename matches but
//!   no `# PREFIX-NNNN. Title` header is found). Excluded from rules.
//!
//! Infrastructure failures (unreadable domain directory after
//! containment passes, e.g. permission denied on a canonicalized
//! path) bubble as `Err(String)` for `eprintln!` + `process::exit(1)`
//! per AFM-0003 R1.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::config::Config;
use crate::model::{
    AdrId, AdrRecord, DomainDir, RelVerb, Relationship, Status, TaggedRule, Tier, parse_adr_id,
};
use crate::report::Diagnostic;

/// Records and diagnostics produced by parsing a directory of ADRs.
///
/// `records` contains successfully parsed ADRs ready for rule evaluation.
/// `diagnostics` carries per-file `P###` warnings (per AFM-0017) for
/// files whose filenames matched the prefix pattern but whose contents
/// could not be parsed. Infrastructure failures (unreadable directory)
/// surface as `Err(String)` from the calling parser entrypoints.
#[derive(Debug, Default)]
pub struct ParseOutcome {
    pub records: Vec<AdrRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse all ADR files in a domain directory.
///
/// Returns `Err` only when the domain directory itself cannot be
/// read; per-file failures are reported as `P###` diagnostics on
/// the returned `ParseOutcome`.
pub fn parse_domain(dir: &DomainDir) -> Result<ParseOutcome, String> {
    let mut outcome = ParseOutcome::default();

    let entries = fs::read_dir(&dir.path).map_err(|e| {
        format!(
            "cannot read domain directory {}: {}",
            dir.path.display().to_string().escape_debug(),
            e.to_string().escape_debug()
        )
    })?;

    let filename_re = Regex::new(&format!(
        r"^{}-(\d{{4}})-[a-z0-9]+(?:-[a-z0-9]+)*\.md$",
        regex::escape(&dir.prefix)
    ))
    .expect("valid regex");

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if !filename_re.is_match(&name) {
            continue;
        }

        match parse_adr_file(&entry.path(), &dir.prefix, false) {
            Ok(file_outcome) => {
                if let Some(record) = file_outcome.record {
                    outcome.records.push(record);
                }
                outcome.diagnostics.extend(file_outcome.diagnostics);
            }
            Err(e) => {
                outcome.diagnostics.push(Diagnostic::warning(
                    "P001",
                    &entry.path(),
                    0,
                    format!("cannot read ADR file: {}", e.escape_debug()),
                ));
            }
        }
    }

    outcome.records.sort_by_key(|r| r.id.number);
    Ok(outcome)
}

/// Parse all ADR files in the stale directory.
///
/// Stale files may belong to any domain, so we try all configured
/// domain prefixes. Returns `Err` only when the stale directory
/// cannot be read.
pub fn parse_stale(stale_dir: &Path, config: &Config) -> Result<ParseOutcome, String> {
    let mut outcome = ParseOutcome::default();

    let entries = fs::read_dir(stale_dir).map_err(|e| {
        format!(
            "cannot read stale directory {}: {}",
            stale_dir.display().to_string().escape_debug(),
            e.to_string().escape_debug()
        )
    })?;

    let prefixes: Vec<(&str, Regex)> = config
        .domains
        .iter()
        .map(|d| {
            let pattern = format!(
                r"^{}-\d{{4}}-[a-z0-9]+(?:-[a-z0-9]+)*\.md$",
                regex::escape(&d.prefix),
            );
            (
                d.prefix.as_str(),
                Regex::new(&pattern).expect("valid regex"),
            )
        })
        .collect();

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if !name.ends_with(".md") || name == "README.md" {
            continue;
        }

        for (prefix, re) in &prefixes {
            if re.is_match(&name) {
                match parse_adr_file(&entry.path(), prefix, true) {
                    Ok(file_outcome) => {
                        if let Some(record) = file_outcome.record {
                            outcome.records.push(record);
                        }
                        outcome.diagnostics.extend(file_outcome.diagnostics);
                    }
                    Err(e) => {
                        outcome.diagnostics.push(Diagnostic::warning(
                            "P001",
                            &entry.path(),
                            0,
                            format!("cannot read ADR file: {}", e.escape_debug()),
                        ));
                    }
                }
                break;
            }
        }
    }

    outcome.records.sort_by_key(|r| r.id.number);
    Ok(outcome)
}

/// Records and diagnostics produced by parsing a single ADR file.
///
/// `record` is `None` when the file could be read but the H1 title
/// is missing or malformed (a P002 diagnostic accompanies it).
#[derive(Debug, Default)]
pub struct ParseFileOutcome {
    pub record: Option<AdrRecord>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse a single ADR file.
///
/// Returns `Err` when the file cannot be read (caller maps to P001).
/// Returns `Ok` with an empty `record` and a P002 diagnostic when the
/// H1 title is missing or malformed.
pub fn parse_adr_file(
    path: &Path,
    expected_prefix: &str,
    is_stale: bool,
) -> Result<ParseFileOutcome, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let lines: Vec<&str> = content.lines().collect();

    let mut outcome = ParseFileOutcome::default();

    if lines.is_empty() {
        outcome.diagnostics.push(Diagnostic::warning(
            "P002",
            path,
            0,
            format!(
                "ADR file is empty; expected `# {}-NNNN. Title` H1",
                expected_prefix.escape_debug()
            ),
        ));
        return Ok(outcome);
    }

    let Some((id, title, title_line)) = parse_title(&lines, expected_prefix) else {
        outcome.diagnostics.push(Diagnostic::warning(
            "P002",
            path,
            0,
            format!(
                "missing or malformed H1 title; expected `# {}-NNNN. Title text`",
                expected_prefix.escape_debug()
            ),
        ));
        return Ok(outcome);
    };

    let (date, _) = find_field(&lines, "Date:");
    let (last_reviewed, _) = find_field(&lines, "Last-reviewed:");
    let (tier, _) = find_tier_field(&lines);

    let (status, status_line, status_raw) = find_status_field(&lines);
    let status_from_section = status.is_none() && has_heading(&lines, "Status");

    let (relationships, has_related, _related_has_placeholder) = find_relationships(&lines);

    let has_context = has_heading(&lines, "Context");
    let has_decision = has_heading(&lines, "Decision");
    let has_consequences = has_heading(&lines, "Consequences");
    let has_retirement = has_heading(&lines, "Retirement");

    let (section_order, section_word_counts) = analyze_sections(&lines);

    let crates = find_crates_field(&lines);

    let (parent_cross_domain, parent_cross_domain_reason) = find_parent_cross_domain_field(&lines);

    let decision_rules = extract_tagged_rules(&lines);

    let (max_code_block_lines, _code_block_count, max_code_block_line) =
        measure_code_blocks(&lines);

    outcome.record = Some(AdrRecord {
        id,
        file_path: path.to_owned(),
        title: Some(title),
        title_line,
        date,
        last_reviewed,
        tier,
        status,
        status_line,
        status_raw,
        relationships,
        has_related,
        has_context,
        has_decision,
        has_consequences,
        has_retirement,
        is_stale,
        status_from_section,
        max_code_block_lines,
        max_code_block_line,
        section_order,
        section_word_counts,
        crates,
        decision_rules,
        parent_cross_domain,
        parent_cross_domain_reason,
    });
    Ok(outcome)
}

/// Parse the H1 title line: `# PREFIX-NNNN. Title text`.
fn parse_title(lines: &[&str], expected_prefix: &str) -> Option<(AdrId, String, usize)> {
    for (i, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix("# ")
            && let Some(dot_pos) = rest.find(". ")
            && let Some(id) = parse_adr_id(&rest[..dot_pos])
            && id.prefix == expected_prefix
        {
            let title = rest[dot_pos + 2..].to_owned();
            return Some((id, title, i + 1));
        }
    }
    None
}

/// Find a simple `Key: value` field in the preamble (before first H2 heading).
fn find_field(lines: &[&str], key: &str) -> (Option<String>, usize) {
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("## ") {
            break;
        }
        if let Some(value) = line.strip_prefix(key) {
            let value = value.trim();
            if !value.is_empty() {
                return (Some(value.to_owned()), i + 1);
            }
        }
    }
    (None, 0)
}

/// Find `Tier:` field in the preamble (before first H2 heading).
fn find_tier_field(lines: &[&str]) -> (Option<Tier>, usize) {
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("## ") {
            break;
        }
        if let Some(value) = line.strip_prefix("Tier:") {
            let value = value.trim();
            return (Tier::parse(value), i + 1);
        }
    }
    (None, 0)
}

/// Find `Status:` metadata field in the preamble (before first H2 heading).
fn find_status_field(lines: &[&str]) -> (Option<Status>, usize, Option<String>) {
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("## ") {
            break;
        }
        if let Some(value) = line.strip_prefix("Status:") {
            let value = value.trim();
            if !value.is_empty() {
                let raw = value.to_owned();
                let status = Status::parse(value);
                return (Some(status), i + 1, Some(raw));
            }
        }
    }
    (None, 0, None)
}

/// Find all relationships in the `## Related` section.
///
/// Parses pipe-separated format: `Verb: TARGET1, TARGET2 | Verb: TARGET3`
/// Each segment is `Verb: targets` where targets are comma-separated ADR IDs.
/// A single segment without pipes is also valid: `Root: OWN-ID`.
fn find_relationships(lines: &[&str]) -> (Vec<Relationship>, bool, bool) {
    let mut rels = Vec::new();
    let mut in_related = false;
    let mut found_section = false;
    let mut has_placeholder = false;

    for (i, line) in lines.iter().enumerate() {
        if *line == "## Related" {
            in_related = true;
            found_section = true;
            continue;
        }
        if in_related {
            if line.is_empty() {
                continue;
            }
            if line.starts_with("## ") {
                break;
            }
            let trimmed = line.trim();
            if trimmed == "—" || trimmed == "- —" {
                has_placeholder = true;
                continue;
            }
            let segments: Vec<&str> = trimmed.split(" | ").collect();
            for segment in &segments {
                let segment = segment.trim();
                if segment.is_empty() {
                    continue;
                }
                if let Some(colon_pos) = segment.find(": ") {
                    let verb_str = &segment[..colon_pos];
                    let targets_str = &segment[colon_pos + 2..];

                    if let Some(verb) = RelVerb::parse(verb_str) {
                        for target_str in targets_str.split(", ") {
                            let clean = strip_annotation(target_str);
                            if let Some(target_id) = parse_adr_id(clean) {
                                rels.push(Relationship {
                                    verb,
                                    target: target_id,
                                    line: i + 1,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    (rels, found_section, has_placeholder)
}

/// Analyze H2 sections: extract ordering and word counts.
///
/// Word counts exclude fenced code blocks.
fn analyze_sections(lines: &[&str]) -> (Vec<String>, HashMap<String, usize>) {
    let mut order = Vec::new();
    let mut word_counts: HashMap<String, usize> = HashMap::new();

    let mut current_section: Option<String> = None;
    let mut current_words = 0usize;
    let mut in_code_block = false;

    for line in lines {
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }

        if in_code_block {
            continue;
        }

        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(ref section) = current_section {
                word_counts.insert(section.clone(), current_words);
            }

            let name = heading.trim().to_owned();
            order.push(name.clone());
            current_section = Some(name);
            current_words = 0;
        } else if current_section.is_some() && !line.is_empty() {
            current_words += line.split_whitespace().count();
        }
    }

    if let Some(ref section) = current_section {
        word_counts.insert(section.clone(), current_words);
    }

    (order, word_counts)
}

/// Parse `Crates: crate-a, crate-b` from the metadata preamble.
///
/// Returns an empty vec if the field is absent or empty.
fn find_crates_field(lines: &[&str]) -> Vec<String> {
    for line in lines {
        if let Some(value) = line.strip_prefix("Crates:") {
            let value = value.trim();
            if value.is_empty() {
                return Vec::new();
            }
            return value.split(',').map(|s| s.trim().to_owned()).collect();
        }
        if line.starts_with("## ") {
            break;
        }
    }
    Vec::new()
}

/// Parse `Parent-cross-domain: PREFIX-NNNN — reason` from the metadata
/// preamble. Returns `(Some(id), reason)` when a valid ID is parsed,
/// `(None, String::new())` otherwise.
///
/// Accepts both em-dash (`—`) and ASCII hyphen-with-spaces (` - `) as
/// separators between the ID and the reason. The reason is optional;
/// the field may contain just the ID.
fn find_parent_cross_domain_field(lines: &[&str]) -> (Option<AdrId>, String) {
    for line in lines {
        if line.starts_with("## ") {
            break;
        }
        if let Some(value) = line.strip_prefix("Parent-cross-domain:") {
            let value = value.trim();
            if value.is_empty() {
                return (None, String::new());
            }
            let (id_part, reason) = split_id_and_reason(value);
            if let Some(id) = parse_adr_id(id_part.trim()) {
                return (Some(id), reason.trim().to_owned());
            }
            return (None, String::new());
        }
    }
    (None, String::new())
}

/// Split a `PREFIX-NNNN — reason` string into ID and reason parts.
fn split_id_and_reason(value: &str) -> (&str, &str) {
    if let Some(idx) = value.find('—') {
        return (&value[..idx], &value[idx + '—'.len_utf8()..]);
    }
    if let Some(idx) = value.find(" - ") {
        return (&value[..idx], &value[idx + 3..]);
    }
    if let Some(idx) = value.find(char::is_whitespace) {
        return (&value[..idx], &value[idx..]);
    }
    (value, "")
}

///
/// Matches `RN [L]: text` pattern within the Decision section where
/// N is the sequential rule number and L is the Meadows layer (1-12).
/// Continuation lines (indented ≥2 spaces, not a new tagged rule)
/// are joined to the current rule with a space.
/// A blank line terminates continuation.
///
/// Returns an empty vec when no tagged rules are found.
/// Regex for tagged rules: `R1 [5]: Rule text here`
static TAGGED_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^R(\d+)\s*\[(\d+)\]:\s*(.+)").expect("valid regex"));

fn extract_tagged_rules(lines: &[&str]) -> Vec<TaggedRule> {
    let mut rules = Vec::new();
    let mut in_decision = false;

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line == "## Decision" {
            in_decision = true;
            i += 1;
            continue;
        }
        if !in_decision {
            i += 1;
            continue;
        }
        if line.starts_with("## ") {
            break;
        }
        if let Some(caps) = TAGGED_RULE_RE.captures(line) {
            let num = caps.get(1).unwrap().as_str();
            let layer_str = caps.get(2).unwrap().as_str();
            let layer: u8 = layer_str.parse().unwrap_or(0);
            let mut text = caps.get(3).unwrap().as_str().trim().to_owned();
            let rule_line = i + 1;

            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if next.trim().is_empty() {
                    break;
                }
                if next.starts_with("## ") {
                    break;
                }
                if TAGGED_RULE_RE.is_match(next) {
                    break;
                }
                if next.len() >= 2 && next.starts_with("  ") {
                    text.push(' ');
                    text.push_str(next.trim());
                    i += 1;
                } else {
                    break;
                }
            }

            rules.push(TaggedRule {
                id: format!("R{num}"),
                text,
                layer,
                line: rule_line,
            });
        } else {
            i += 1;
        }
    }

    rules
}

/// Strip trailing annotations like ` (indirect)` or `` (`#[non_exhaustive]`) ``.
fn strip_annotation(s: &str) -> &str {
    let s = s.trim();
    if let Some(paren_start) = s.find(" (") {
        s[..paren_start].trim()
    } else {
        s
    }
}

/// Measure fenced code blocks (triple-backtick delimiters).
///
/// Returns `(max_lines, block_count, max_block_start_line)` where
/// `max_block_start_line` is the 1-indexed line number of the opening
/// fence of the largest block (0 if no blocks or all blocks are empty).
/// Fence lines themselves are excluded from the count. Language
/// annotations on opening fences (e.g., ` ```rust `) are ignored.
///
/// Known limitation: nested backticks (markdown documenting markdown)
/// cause false open/close toggling. Acceptable for ADR content.
fn measure_code_blocks(lines: &[&str]) -> (usize, usize, usize) {
    let mut in_block = false;
    let mut current_lines = 0usize;
    let mut current_start = 0usize;
    let mut max_lines = 0usize;
    let mut max_start = 0usize;
    let mut block_count = 0usize;

    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("```") {
            if in_block {
                if current_lines > max_lines {
                    max_lines = current_lines;
                    max_start = current_start;
                }
                in_block = false;
            } else {
                in_block = true;
                current_lines = 0;
                current_start = i + 1;
                block_count += 1;
            }
        } else if in_block {
            current_lines += 1;
        }
    }

    if in_block && current_lines > max_lines {
        max_lines = current_lines;
        max_start = current_start;
    }

    (max_lines, block_count, max_start)
}

/// Check if a `## Heading` exists.
fn has_heading(lines: &[&str], name: &str) -> bool {
    let target = format!("## {name}");
    lines.iter().any(|line| *line == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_title_extracts_id_and_text() {
        let lines = vec![
            "# CHE-0042. Event Envelope Construction Invariants",
            "",
            "Date: 2026-04-25",
        ];
        let (id, title, line) = parse_title(&lines, "CHE").unwrap();
        assert_eq!(id.prefix, "CHE");
        assert_eq!(id.number, 42);
        assert_eq!(title, "Event Envelope Construction Invariants");
        assert_eq!(line, 1);
    }

    #[test]
    fn parse_title_wrong_prefix_returns_none() {
        let lines = vec!["# PAR-0001. Some Title"];
        assert!(parse_title(&lines, "CHE").is_none());
    }

    #[test]
    fn find_field_extracts_date() {
        let lines = vec![
            "# Title",
            "",
            "Date: 2026-04-25",
            "Last-reviewed: 2026-04-25",
        ];
        let (date, line) = find_field(&lines, "Date:");
        assert_eq!(date.as_deref(), Some("2026-04-25"));
        assert_eq!(line, 3);
    }

    #[test]
    fn find_relationships_parses_multi_target() {
        let lines = vec![
            "## Related",
            "",
            "References: CHE-0006, CHE-0032 | Supersedes: CHE-0015",
            "",
            "## Context",
        ];
        let (rels, found, _placeholder) = find_relationships(&lines);
        assert!(found);
        assert_eq!(rels.len(), 3);
        assert_eq!(rels[0].verb, RelVerb::References);
        assert_eq!(rels[0].target, parse_adr_id("CHE-0006").unwrap());
        assert_eq!(rels[1].target, parse_adr_id("CHE-0032").unwrap());
        assert_eq!(rels[2].verb, RelVerb::Supersedes);
        assert_eq!(rels[2].target, parse_adr_id("CHE-0015").unwrap());
    }

    #[test]
    fn find_relationships_parses_root_verb() {
        let lines = vec!["## Related", "", "Root: CHE-0001", "", "## Context"];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].verb, RelVerb::Root);
        assert_eq!(rels[0].target.prefix, "CHE");
        assert_eq!(rels[0].target.number, 1);
    }

    #[test]
    fn strip_annotation_removes_parenthetical() {
        assert_eq!(strip_annotation("CHE-0036 (indirect)"), "CHE-0036");
        assert_eq!(strip_annotation("CHE-0021"), "CHE-0021");
        assert_eq!(
            strip_annotation("CHE-0021 (`#[non_exhaustive]`)"),
            "CHE-0021"
        );
    }

    #[test]
    fn has_heading_finds_section() {
        let lines = vec![
            "## Status",
            "",
            "Accepted",
            "",
            "## Context",
            "",
            "Some text.",
        ];
        assert!(has_heading(&lines, "Context"));
        assert!(!has_heading(&lines, "Decision"));
    }

    #[test]
    fn has_heading_finds_retirement() {
        let lines = vec!["## Retirement", "", "Deprecated because reasons."];
        assert!(has_heading(&lines, "Retirement"));
    }

    #[test]
    fn measure_code_blocks_counts_lines() {
        let lines = vec![
            "some text",
            "```rust",
            "fn main() {}",
            "let x = 1;",
            "let y = 2;",
            "```",
            "more text",
        ];
        let (max, count, start) = measure_code_blocks(&lines);
        assert_eq!(max, 3, "3 lines between fences");
        assert_eq!(count, 1);
        assert_eq!(start, 2, "opening fence is line 2 (1-indexed)");
    }

    #[test]
    fn measure_code_blocks_multiple_blocks() {
        let lines = vec![
            "```", "line1", "```", "text", "```rust", "a", "b", "c", "d", "e", "```",
        ];
        let (max, count, start) = measure_code_blocks(&lines);
        assert_eq!(max, 5, "second block has 5 lines");
        assert_eq!(count, 2);
        assert_eq!(start, 5, "second block opens at line 5 (1-indexed)");
    }

    #[test]
    fn measure_code_blocks_empty_block() {
        let lines = vec!["```", "```"];
        let (max, count, start) = measure_code_blocks(&lines);
        assert_eq!(max, 0);
        assert_eq!(count, 1);
        assert_eq!(start, 0);
    }

    #[test]
    fn measure_code_blocks_no_blocks() {
        let lines = vec!["some text", "more text"];
        let (max, count, start) = measure_code_blocks(&lines);
        assert_eq!(max, 0);
        assert_eq!(count, 0);
        assert_eq!(start, 0, "no blocks means start line 0");
    }

    #[test]
    fn measure_code_blocks_fence_lines_excluded() {
        let lines = vec!["```", "only_this", "```"];
        let (max, count, start) = measure_code_blocks(&lines);
        assert_eq!(max, 1, "only content line counted, not fences");
        assert_eq!(count, 1);
        assert_eq!(start, 1);
    }

    #[test]
    fn measure_code_blocks_unclosed_block() {
        let lines = vec!["text", "```rust", "fn main() {}", "let x = 1;"];
        let (max, count, start) = measure_code_blocks(&lines);
        assert_eq!(max, 2, "unclosed block has 2 content lines");
        assert_eq!(count, 1);
        assert_eq!(start, 2, "opening fence at line 2 (1-indexed)");
    }

    #[test]
    fn find_relationships_detects_placeholder() {
        let lines = vec!["## Related", "", "- —", "", "## Context"];
        let (rels, found, placeholder) = find_relationships(&lines);
        assert!(found);
        assert!(rels.is_empty());
        assert!(placeholder, "should detect `- —` as placeholder");
    }

    #[test]
    fn find_relationships_detects_bare_dash_placeholder() {
        let lines = vec!["## Related", "", "—", "", "## Context"];
        let (rels, found, placeholder) = find_relationships(&lines);
        assert!(found);
        assert!(rels.is_empty());
        assert!(placeholder, "should detect bare `—` as placeholder");
    }

    #[test]
    fn find_relationships_no_placeholder_with_rels() {
        let lines = vec!["## Related", "", "References: CHE-0001", "", "## Context"];
        let (rels, found, placeholder) = find_relationships(&lines);
        assert!(found);
        assert_eq!(rels.len(), 1);
        assert!(
            !placeholder,
            "should not detect placeholder when rels exist"
        );
    }

    #[test]
    fn analyze_sections_correct_order() {
        let lines = vec![
            "# CHE-0001. Title",
            "",
            "## Related",
            "",
            "Root: CHE-0001",
            "",
            "## Context",
            "",
            "This is the context with enough words to pass validation easily.",
            "",
            "## Decision",
            "",
            "We decided to do this thing because it makes sense to us.",
            "",
            "## Consequences",
            "",
            "This makes testing easier and code more maintainable overall.",
        ];
        let (order, counts) = analyze_sections(&lines);
        assert_eq!(
            order,
            vec!["Related", "Context", "Decision", "Consequences"]
        );
        assert_eq!(counts["Context"], 11);
        assert_eq!(counts["Decision"], 12);
        assert_eq!(counts["Consequences"], 9);
    }

    #[test]
    fn analyze_sections_excludes_code_blocks() {
        let lines = vec![
            "## Decision",
            "",
            "We decided to use this approach.",
            "",
            "```rust",
            "fn main() {",
            "    println!(\"hello\");",
            "}",
            "```",
            "",
            "That is all.",
        ];
        let (_, counts) = analyze_sections(&lines);
        assert_eq!(counts["Decision"], 9);
    }

    #[test]
    fn analyze_sections_with_retirement() {
        let lines = vec![
            "## Status",
            "",
            "Deprecated",
            "",
            "## Retirement",
            "",
            "Deprecated because the transport layer moved to a different protocol entirely.",
        ];
        let (order, counts) = analyze_sections(&lines);
        assert!(order.contains(&"Retirement".to_owned()));
        assert_eq!(counts["Retirement"], 11);
    }

    #[test]
    fn self_referencing_detected() {
        let lines = vec!["## Related", "", "Root: CHE-0001", "", "## Context"];
        let (rels, _, _) = find_relationships(&lines);
        let id = AdrId {
            prefix: "CHE".into(),
            number: 1,
        };
        let is_self_ref = rels
            .iter()
            .any(|rel| rel.verb == RelVerb::Root && rel.target == id);
        assert!(is_self_ref);
    }

    #[test]
    fn self_referencing_wrong_id_not_detected() {
        let lines = vec!["## Related", "", "Root: CHE-0002", "", "## Context"];
        let (rels, _, _) = find_relationships(&lines);
        let id = AdrId {
            prefix: "CHE".into(),
            number: 1,
        };
        let is_self_ref = rels
            .iter()
            .any(|rel| rel.verb == RelVerb::Root && rel.target == id);
        assert!(!is_self_ref);
    }

    #[test]
    fn find_crates_field_present() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Date: 2026-04-25",
            "Crates: example-core, example-gateway",
            "Tier: A",
            "",
            "## Status",
        ];
        let crates = find_crates_field(&lines);
        assert_eq!(crates, vec!["example-core", "example-gateway"]);
    }

    #[test]
    fn find_crates_field_empty() {
        let lines = vec!["# CHE-0042. Title", "", "Crates:", "", "## Status"];
        let crates = find_crates_field(&lines);
        assert!(crates.is_empty());
    }

    #[test]
    fn find_crates_field_absent() {
        let lines = vec!["# CHE-0042. Title", "", "Date: 2026-04-25", "", "## Status"];
        let crates = find_crates_field(&lines);
        assert!(crates.is_empty());
    }

    #[test]
    fn find_parent_cross_domain_em_dash() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Parent-cross-domain: COM-0001 — bridges principle to architecture",
            "",
            "## Status",
        ];
        let (id, reason) = find_parent_cross_domain_field(&lines);
        assert_eq!(id.as_ref().unwrap().prefix, "COM");
        assert_eq!(id.unwrap().number, 1);
        assert_eq!(reason, "bridges principle to architecture");
    }

    #[test]
    fn find_parent_cross_domain_ascii_hyphen() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Parent-cross-domain: COM-0001 - reason text",
            "",
            "## Status",
        ];
        let (id, reason) = find_parent_cross_domain_field(&lines);
        assert!(id.is_some());
        assert_eq!(reason, "reason text");
    }

    #[test]
    fn find_parent_cross_domain_id_only() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Parent-cross-domain: COM-0001",
            "",
            "## Status",
        ];
        let (id, reason) = find_parent_cross_domain_field(&lines);
        assert!(id.is_some());
        assert_eq!(reason, "");
    }

    #[test]
    fn find_parent_cross_domain_absent() {
        let lines = vec!["# CHE-0042. Title", "", "Date: 2026-04-25", "", "## Status"];
        let (id, reason) = find_parent_cross_domain_field(&lines);
        assert!(id.is_none());
        assert_eq!(reason, "");
    }

    #[test]
    fn find_parent_cross_domain_invalid_id_returns_none() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Parent-cross-domain: not-an-id",
            "",
            "## Status",
        ];
        let (id, reason) = find_parent_cross_domain_field(&lines);
        assert!(id.is_none());
        assert_eq!(reason, "");
    }

    #[test]
    fn find_parent_cross_domain_stops_at_h2() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "## Status",
            "",
            "Parent-cross-domain: COM-0001 — late field",
        ];
        let (id, _) = find_parent_cross_domain_field(&lines);
        assert!(id.is_none());
    }

    #[test]
    fn find_parent_cross_domain_garbage_after_colon() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Parent-cross-domain: !!!@@@",
            "",
            "## Status",
        ];
        let (id, reason) = find_parent_cross_domain_field(&lines);
        assert!(id.is_none(), "garbage value must yield None ID");
        assert_eq!(reason, "");
    }

    #[test]
    fn find_parent_cross_domain_lowercase_prefix_rejected() {
        let lines = vec![
            "# CHE-0042. Title",
            "",
            "Parent-cross-domain: com-0001 — bad case",
            "",
            "## Status",
        ];
        let (id, _) = find_parent_cross_domain_field(&lines);
        assert!(id.is_none(), "lowercase prefix must be rejected");
    }

    #[test]
    fn extract_tagged_rules_normal() {
        let lines = vec![
            "## Decision",
            "",
            "R1 [5]: All events must be versioned",
            "R2 [5]: Snapshots at 100-event intervals",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id, "R1");
        assert_eq!(rules[0].text, "All events must be versioned");
        assert_eq!(rules[0].layer, 5);
        assert_eq!(rules[1].id, "R2");
        assert_eq!(rules[1].text, "Snapshots at 100-event intervals");
        assert_eq!(rules[1].layer, 5);
    }

    #[test]
    fn extract_tagged_rules_no_rules_returns_empty() {
        let lines = vec![
            "## Decision",
            "",
            "We use event sourcing for persistence.",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert!(rules.is_empty());
    }

    #[test]
    fn extract_tagged_rules_mixed_with_prose() {
        let lines = vec![
            "## Decision",
            "",
            "We adopt the following rules:",
            "",
            "R1 [6]: Events are append-only",
            "Some prose between rules.",
            "R2 [6]: Snapshots are optional",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id, "R1");
        assert_eq!(rules[1].id, "R2");
    }

    #[test]
    fn extract_tagged_rules_malformed_ignored() {
        let lines = vec![
            "## Decision",
            "",
            "Rfoo [5]: Not a valid rule tag",
            "R1 [5]: Valid rule",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "R1");
    }

    #[test]
    fn extract_tagged_rules_multiline() {
        let lines = vec![
            "## Decision",
            "",
            "R1 [5]: Construct EventEnvelope exclusively through",
            "  EventEnvelope::new(), which validates non-nil event_id",
            "  and returns Result<Self, EnvelopeError>",
            "R2 [5]: Use NonZeroU64 for the sequence field",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].id, "R1");
        assert_eq!(
            rules[0].text,
            "Construct EventEnvelope exclusively through \
             EventEnvelope::new(), which validates non-nil event_id \
             and returns Result<Self, EnvelopeError>"
        );
        assert_eq!(rules[1].id, "R2");
        assert_eq!(rules[1].text, "Use NonZeroU64 for the sequence field");
    }

    #[test]
    fn extract_tagged_rules_blank_line_terminates_continuation() {
        let lines = vec![
            "## Decision",
            "",
            "R1 [5]: First part of rule",
            "",
            "  This should NOT be continuation (after blank line)",
            "R2 [5]: Second rule",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].text, "First part of rule");
        assert_eq!(rules[1].text, "Second rule");
    }

    #[test]
    fn extract_tagged_rules_backtick_content_preserved() {
        let lines = vec![
            "## Decision",
            "",
            "R1 [5]: Construct via `EventEnvelope::new()` which",
            "  returns `Result<Self, EnvelopeError>`",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 1);
        assert_eq!(
            rules[0].text,
            "Construct via `EventEnvelope::new()` which \
             returns `Result<Self, EnvelopeError>`"
        );
    }

    #[test]
    fn extract_tagged_rules_layer_parsed() {
        let lines = vec![
            "## Decision",
            "",
            "R1 [1]: Paradigm-level rule",
            "R2 [12]: Parameter-level rule",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].layer, 1);
        assert_eq!(rules[1].layer, 12);
    }

    #[test]
    fn extract_tagged_rules_old_format_not_matched() {
        let lines = vec![
            "## Decision",
            "",
            "- **R1**: Old format rule",
            "- **R2**: Another old format rule",
            "",
            "## Consequences",
        ];
        let rules = extract_tagged_rules(&lines);
        assert!(rules.is_empty(), "old format should not be parsed");
    }

    #[test]
    fn parse_adr_file_empty_file_emits_p002() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("CHE-0001-empty.md");
        fs::write(&path, "").expect("write empty file");

        let outcome = parse_adr_file(&path, "CHE", false).expect("read should succeed");
        assert!(outcome.record.is_none(), "no record from empty file");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].rule, "P002");
        assert!(
            outcome.diagnostics[0].message.contains("empty"),
            "message should mention empty"
        );
    }

    #[test]
    fn parse_adr_file_missing_h1_emits_p002() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("CHE-0001-no-h1.md");
        fs::write(
            &path,
            "Some prose without an H1 title.\n\n## Status\n\nAccepted\n",
        )
        .expect("write file");

        let outcome = parse_adr_file(&path, "CHE", false).expect("read should succeed");
        assert!(outcome.record.is_none(), "no record without H1");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].rule, "P002");
        assert!(
            outcome.diagnostics[0]
                .message
                .contains("missing or malformed"),
            "message should mention malformed title"
        );
    }

    #[test]
    fn parse_adr_file_wrong_prefix_h1_emits_p002() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("CHE-0001-wrong-prefix.md");
        fs::write(&path, "# PAR-0001. Wrong prefix\n").expect("write file");

        let outcome = parse_adr_file(&path, "CHE", false).expect("read should succeed");
        assert!(
            outcome.record.is_none(),
            "no record when prefix doesn't match expected"
        );
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].rule, "P002");
    }

    #[test]
    fn parse_adr_file_h1_with_trailing_space_emits_p002() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("CHE-0001-trailing-space.md");
        fs::write(&path, "# CHE-0001 . Title\n").expect("write file");

        let outcome = parse_adr_file(&path, "CHE", false).expect("read should succeed");
        assert!(outcome.record.is_none(), "malformed H1 yields no record");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert_eq!(outcome.diagnostics[0].rule, "P002");
    }

    #[test]
    fn parse_adr_file_valid_h1_emits_no_diagnostics() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("CHE-0001-valid.md");
        fs::write(
            &path,
            "# CHE-0001. Valid Title\n\nDate: 2026-04-29\nTier: B\nStatus: Accepted\n\n## Related\n\nRoot: CHE-0001\n\n## Context\n\nProse.\n",
        )
        .expect("write file");

        let outcome = parse_adr_file(&path, "CHE", false).expect("read should succeed");
        assert!(outcome.record.is_some(), "record should be parsed");
        assert!(
            outcome.diagnostics.is_empty(),
            "valid file should emit no diagnostics"
        );
    }

    #[test]
    fn parse_adr_file_unreadable_returns_err() {
        let outcome = parse_adr_file(
            Path::new("/nonexistent/path/that/should/never/exist/CHE-0001.md"),
            "CHE",
            false,
        );
        assert!(outcome.is_err(), "missing file should bubble as Err");
    }

    #[test]
    fn parse_domain_unreadable_dir_returns_err() {
        let domain_dir = DomainDir {
            prefix: "CHE".to_string(),
            name: "Cherry-pit Test".to_string(),
            path: std::path::PathBuf::from(
                "/nonexistent/path/that/should/never/exist/for/parse-domain-test",
            ),
        };
        let outcome = parse_domain(&domain_dir);
        assert!(
            outcome.is_err(),
            "unreadable domain directory should bubble as Err per AFM-0017 R4"
        );
        let msg = outcome.unwrap_err();
        assert!(
            msg.contains("cannot read domain directory"),
            "error should describe the failure: {msg}"
        );
    }

    #[test]
    fn parse_domain_with_unreadable_file_emits_p001() {
        let dir = tempfile::tempdir().expect("tempdir");
        let valid_path = dir.path().join("CHE-0001-valid.md");
        fs::write(
            &valid_path,
            "# CHE-0001. Valid\n\nDate: 2026-04-29\nTier: B\nStatus: Accepted\n\n## Related\n\nRoot: CHE-0001\n\n## Context\n\nProse.\n",
        )
        .expect("write valid");

        fs::create_dir(dir.path().join("CHE-0002-actually-a-dir.md"))
            .expect("create masquerading dir");

        let domain_dir = DomainDir {
            prefix: "CHE".to_string(),
            name: "Cherry-pit Test".to_string(),
            path: dir.path().to_owned(),
        };
        let outcome = parse_domain(&domain_dir).expect("read_dir should succeed");
        assert_eq!(outcome.records.len(), 1, "one valid record");
        assert_eq!(outcome.diagnostics.len(), 1, "one P001 diagnostic");
        assert_eq!(outcome.diagnostics[0].rule, "P001");
        assert!(
            outcome.diagnostics[0]
                .message
                .contains("cannot read ADR file"),
            "P001 message should describe failure"
        );
    }

    #[test]
    fn parse_stale_with_unreadable_file_emits_p001() {
        let dir = tempfile::tempdir().expect("tempdir");

        fs::create_dir(dir.path().join("CHE-0099-actually-a-dir.md"))
            .expect("create masquerading dir");

        let config_toml = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "CHE"
name = "Example"
directory = "example"
description = "Test domain"
crates = []
"#;
        let config: Config = toml::from_str(config_toml).expect("parse config");

        let outcome = parse_stale(dir.path(), &config).expect("read_dir should succeed");
        assert!(outcome.records.is_empty(), "no valid records");
        assert_eq!(outcome.diagnostics.len(), 1, "one P001 diagnostic");
        assert_eq!(outcome.diagnostics[0].rule, "P001");
        assert!(
            outcome.diagnostics[0]
                .message
                .contains("cannot read ADR file"),
            "P001 message should describe failure"
        );
    }

    #[test]
    fn find_status_field_parses_superseded_with_target() {
        let lines = vec![
            "# CHE-0001. Title",
            "Date: 2026-04-27",
            "Tier: B",
            "Status: Superseded by CHE-0099",
            "",
            "## Related",
        ];
        let (status, _, raw) = find_status_field(&lines);
        assert!(matches!(status, Some(Status::SupersededBy(_))));
        assert_eq!(raw.as_deref(), Some("Superseded by CHE-0099"));
    }

    #[test]
    fn find_status_field_none_when_absent() {
        let lines = vec![
            "# CHE-0001. Title",
            "Date: 2026-04-27",
            "Tier: B",
            "",
            "## Related",
        ];
        let (status, _, _) = find_status_field(&lines);
        assert_eq!(status, None);
    }

    #[test]
    fn find_status_field_stops_at_h2() {
        let lines = vec![
            "# CHE-0001. Title",
            "Date: 2026-04-27",
            "Tier: B",
            "",
            "## Context",
            "Status: Accepted",
        ];
        let (status, _, _) = find_status_field(&lines);
        assert_eq!(status, None);
    }

    #[test]
    fn find_relationships_empty_section_returns_no_rels() {
        let lines = vec!["## Related", "", "", "## Context"];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert!(rels.is_empty());
    }

    #[test]
    fn find_relationships_old_bullet_format_not_parsed() {
        let lines = vec![
            "## Related",
            "",
            "- Root: CHE-0001",
            "- References: CHE-0002",
            "",
            "## Context",
        ];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert!(rels.is_empty());
    }

    #[test]
    fn find_relationships_single_verb_no_pipe() {
        let lines = vec!["## Related", "", "References: CHE-0005", "", "## Context"];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].verb, RelVerb::References);
        assert_eq!(rels[0].target.number, 5);
    }

    #[test]
    fn find_relationships_whitespace_around_pipe() {
        let lines = vec![
            "## Related",
            "",
            "Root: CHE-0001  |  References: CHE-0002 ,  CHE-0003",
            "",
            "## Context",
        ];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert_eq!(rels.len(), 3);
        assert_eq!(rels[0].verb, RelVerb::Root);
        assert_eq!(rels[1].target.number, 2);
        assert_eq!(rels[2].target.number, 3);
    }

    #[test]
    fn find_relationships_no_space_pipe_not_parsed() {
        let lines = vec![
            "## Related",
            "",
            "Root: CHE-0001|References: CHE-0002",
            "",
            "## Context",
        ];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert!(
            rels.is_empty(),
            "no-space pipe should not split into segments and strict ID parser \
             rejects trailing text"
        );
    }

    #[test]
    fn find_relationships_multi_line_content() {
        let lines = vec![
            "## Related",
            "",
            "Root: CHE-0001",
            "References: CHE-0002, CHE-0003",
            "",
            "## Context",
        ];
        let (rels, found, _) = find_relationships(&lines);
        assert!(found);
        assert_eq!(rels.len(), 3);
        assert_eq!(rels[0].verb, RelVerb::Root);
        assert_eq!(rels[1].verb, RelVerb::References);
        assert_eq!(rels[1].target.number, 2);
        assert_eq!(rels[2].verb, RelVerb::References);
        assert_eq!(rels[2].target.number, 3);
    }

    #[test]
    fn find_field_stops_at_h2_boundary() {
        let lines = vec![
            "# CHE-0001. Title",
            "",
            "## Context",
            "",
            "Date: 2026-01-01",
        ];
        let (date, _) = find_field(&lines, "Date:");
        assert_eq!(
            date, None,
            "Date: inside a section body should not be found"
        );
    }

    #[test]
    fn find_tier_field_stops_at_h2_boundary() {
        let lines = vec![
            "# CHE-0001. Title",
            "",
            "Tier: B",
            "",
            "## Context",
            "",
            "Tier: A",
        ];
        let (tier, line) = find_tier_field(&lines);
        assert_eq!(
            tier,
            Some(crate::model::Tier::B),
            "should find preamble tier"
        );
        assert_eq!(line, 3);

        let lines_after = vec!["# CHE-0001. Title", "", "## Context", "", "Tier: A"];
        let (tier2, _) = find_tier_field(&lines_after);
        assert_eq!(
            tier2, None,
            "Tier: inside a section body should not be found"
        );
    }
}
