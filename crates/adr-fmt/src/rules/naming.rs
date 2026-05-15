//! File naming rules (N001–N004).
//!
//! N001: Filename must match `PREFIX-NNNN-kebab-slug.md`
//! N002: Number in filename must match H1 title ID
//! N003: Slug must be lowercase kebab-case (a-z0-9, hyphens)
//! N004: Prefix must match a configured domain

use std::sync::LazyLock;

use regex::Regex;

use crate::model::{AdrRecord, parse_adr_id_from_filename_stem};
use crate::report::Diagnostic;

/// N001: filename must match `PREFIX-NNNN-kebab-slug.md`.
static N001_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Z]{2,4}-\d{4}-[a-z0-9]+(?:-[a-z0-9]+)*\.md$").expect("valid regex")
});

/// N003: slug must be lowercase kebab-case.
static KEBAB_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9]+(?:-[a-z0-9]+)*$").expect("valid regex"));

pub fn check(record: &AdrRecord, domain_prefixes: &[&str], diags: &mut Vec<Diagnostic>) {
    let Some(file_name) = record.file_path.file_name().and_then(|f| f.to_str()) else {
        return;
    };

    // N001: Overall pattern
    if !N001_PATTERN.is_match(file_name) {
        diags.push(Diagnostic::warning(
            "N001",
            &record.file_path,
            0,
            format!(
                "filename `{file_name}` does not match pattern \
                 `PREFIX-NNNN-kebab-slug.md`"
            ),
        ));
        return; // N002, N003, N004 depend on valid filename structure
    }

    // N002: Number in filename matches H1 ID
    if let Some(file_id) = parse_adr_id_from_filename_stem(&file_name[..file_name.len() - 3]) {
        // We need the first segment PREFIX-NNNN
        if file_id.prefix != record.id.prefix || file_id.number != record.id.number {
            diags.push(Diagnostic::warning(
                "N002",
                &record.file_path,
                record.title_line,
                format!(
                    "filename ID `{file_id}` does not match H1 title ID `{}`",
                    record.id
                ),
            ));
        }
    }

    // N003: Slug is kebab-case
    // Extract slug portion after "PREFIX-NNNN-"
    // "CHE-0001-" → prefix.len() (3) + 1 (-) + 4 (digits) + 1 (-) = len+6
    let prefix_len = record.id.prefix.len() + 6;
    let slug_with_ext = &file_name[prefix_len..];
    let slug = slug_with_ext.strip_suffix(".md").unwrap_or(slug_with_ext);

    if !KEBAB_PATTERN.is_match(slug) {
        diags.push(Diagnostic::warning(
            "N003",
            &record.file_path,
            0,
            format!("slug `{slug}` is not valid kebab-case (a-z0-9, hyphens only)"),
        ));
    }

    // N004: Prefix matches a configured domain
    if !domain_prefixes.contains(&record.id.prefix.as_str()) {
        diags.push(Diagnostic::warning(
            "N004",
            &record.file_path,
            0,
            format!(
                "prefix `{}` does not match any configured domain (known: {})",
                record.id.prefix,
                domain_prefixes.join(", "),
            ),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AdrId, Status, Tier};
    use std::path::PathBuf;

    const TEST_PREFIXES: &[&str] = &["COM", "CHE", "PAR", "GEN"];

    fn make_record(filename: &str, prefix: &str, num: u16) -> AdrRecord {
        AdrRecord {
            id: AdrId {
                prefix: prefix.into(),
                number: num,
            },
            file_path: PathBuf::from(format!("docs/adr/cherry/{filename}")),
            title: Some("Test".into()),
            title_line: 1,
            date: Some("2026-04-25".into()),
            last_reviewed: Some("2026-04-25".into()),
            tier: Some(Tier::B),
            status: Some(Status::Accepted),
            status_line: 8,
            status_raw: Some("Accepted".into()),
            has_related: true,
            has_context: true,
            has_decision: true,
            has_consequences: true,
            ..AdrRecord::default()
        }
    }

    #[test]
    fn valid_filename_no_diagnostics() {
        let record = make_record("CHE-0001-design-priority-ordering.md", "CHE", 1);
        let mut diags = Vec::new();
        check(&record, TEST_PREFIXES, &mut diags);
        assert!(diags.is_empty(), "expected no diags, got: {diags:?}");
    }

    #[test]
    fn uppercase_slug_produces_n001() {
        let record = make_record("CHE-0001-Design-Priority.md", "CHE", 1);
        let mut diags = Vec::new();
        check(&record, TEST_PREFIXES, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "N001"),
            "expected N001, got: {diags:?}"
        );
    }

    #[test]
    fn mismatched_number_produces_n002() {
        let record = make_record("CHE-0099-test.md", "CHE", 1);
        let mut diags = Vec::new();
        check(&record, TEST_PREFIXES, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "N002"),
            "expected N002, got: {diags:?}"
        );
    }

    #[test]
    fn unknown_prefix_produces_n004() {
        let record = make_record("ZZZ-0001-test.md", "ZZZ", 1);
        let mut diags = Vec::new();
        check(&record, TEST_PREFIXES, &mut diags);
        assert!(
            diags.iter().any(|d| d.rule == "N004"),
            "expected N004, got: {diags:?}"
        );
    }

    #[test]
    fn known_prefix_no_n004() {
        let record = make_record("CHE-0001-test.md", "CHE", 1);
        let mut diags = Vec::new();
        check(&record, TEST_PREFIXES, &mut diags);
        assert!(
            !diags.iter().any(|d| d.rule == "N004"),
            "known prefix should not trigger N004"
        );
    }
}
