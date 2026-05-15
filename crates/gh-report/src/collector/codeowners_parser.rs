//! CODEOWNERS file content parser.
//!
//! Parses the raw text content of a CODEOWNERS file and extracts
//! owner references (`@org/team` and `@user` tokens).
//!
//! The parsed types ([`ParsedCodeowners`] and [`CodeownersEntry`]) are
//! defined in [`crate::domain::codeowners`] and re-exported here as the
//! natural import site for parser consumers.

pub use crate::domain::codeowners::{CodeownersEntry, ParsedCodeowners};
use tracing::{trace, warn};

/// Maximum line length in bytes before a line is skipped.
const MAX_LINE_LENGTH: usize = 10 * 1024;

/// Parse a CODEOWNERS file's text content.
///
/// # Parse rules
///
/// - Blank lines are skipped.
/// - Lines starting with `#` (after trimming) are comments and skipped.
/// - Lines longer than 10 KB are skipped (with a warning).
/// - The first token on each line is the file pattern; remaining `@`-prefixed tokens are owners.
/// - Inline `#` comments are stripped before extracting owners.
/// - Bare `*` wildcard entries are kept because they define the default owners for the repository.
/// - Glob patterns like `*.js` are kept.
/// - Email-format owners (no `@` prefix, contain `@` mid-string) are excluded.
/// - Owners are deduplicated across all entries.
#[must_use]
pub fn parse_codeowners(content: &str) -> ParsedCodeowners {
    let mut entries = Vec::new();
    let mut seen_owners = std::collections::HashSet::new();
    let mut unique_owners = Vec::new();
    let mut skipped_lines: u32 = 0;

    for (line_num, line) in content.lines().enumerate() {
        // Skip lines exceeding max length.
        if line.len() > MAX_LINE_LENGTH {
            warn!(
                line = line_num + 1,
                length = line.len(),
                max_length = MAX_LINE_LENGTH,
                "CODEOWNERS line exceeds max length, skipped"
            );
            skipped_lines = skipped_lines.saturating_add(1);
            continue;
        }

        let trimmed = line.trim();

        // Skip blank lines and comment lines.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Strip inline comments: find first `#` that's preceded by whitespace.
        let effective = strip_inline_comment(trimmed);

        // Split into tokens.
        let mut tokens = effective.split_whitespace();

        // First token is the pattern.
        let Some(pattern) = tokens.next() else {
            continue;
        };

        // Extract owners: tokens starting with `@`.
        let mut owners = Vec::new();
        for token in tokens {
            if token.starts_with('@') {
                owners.push(token.to_string());
            } else if token.contains('@') {
                // Email-format owner (e.g., `user@example.com`) — skip.
                trace!(
                    line = line_num + 1,
                    token = token,
                    "skipping email-format owner"
                );
            }
        }

        // Deduplicate owners within this entry.  `dedup()` only removes
        // consecutive duplicates, so sort first to collapse all duplicates.
        owners.sort_unstable();
        owners.dedup();

        // Track unique owners across all entries.
        for owner in &owners {
            let lower = owner.to_lowercase();
            if seen_owners.insert(lower) {
                unique_owners.push(owner.clone());
            }
        }

        entries.push(CodeownersEntry {
            pattern: pattern.to_string(),
            owners,
        });
    }

    ParsedCodeowners {
        entries,
        unique_owners,
        skipped_lines,
    }
}

/// Strip an inline comment from a CODEOWNERS line.
///
/// Looks for ` #` or `\t#` (hash preceded by whitespace) and returns
/// everything before it.
fn strip_inline_comment(line: &str) -> &str {
    // Find the first ` #` or `\t#` that isn't at the start.
    if let Some(pos) = line.find(" #") {
        return line[..pos].trim_end();
    }
    if let Some(pos) = line.find("\t#") {
        return line[..pos].trim_end();
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file() {
        let result = parse_codeowners("");
        assert!(result.entries.is_empty());
        assert!(result.unique_owners.is_empty());
    }

    #[test]
    fn comments_and_blanks_skipped() {
        let content = "# This is a comment\n\n# Another comment\n";
        let result = parse_codeowners(content);
        assert!(result.entries.is_empty());
    }

    #[test]
    fn bare_wildcard_kept() {
        let content = "* @org/team\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].pattern, "*");
        assert_eq!(result.entries[0].owners, vec!["@org/team"]);
        assert_eq!(result.unique_owners, vec!["@org/team"]);
    }

    #[test]
    fn glob_pattern_kept() {
        let content = "*.js @org/frontend-team\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].pattern, "*.js");
        assert_eq!(result.entries[0].owners, vec!["@org/frontend-team"]);
    }

    #[test]
    fn org_team_extraction() {
        let content = "/src/ @org/backend-team @org/infra-team\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(
            result.entries[0].owners,
            vec!["@org/backend-team", "@org/infra-team"]
        );
        assert_eq!(
            result.unique_owners,
            vec!["@org/backend-team", "@org/infra-team"]
        );
    }

    #[test]
    fn user_extraction() {
        let content = "/docs/ @alice @bob\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries[0].owners, vec!["@alice", "@bob"]);
    }

    #[test]
    fn inline_comments_stripped() {
        let content = "/src/ @org/team # This is a comment\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].owners, vec!["@org/team"]);
    }

    #[test]
    fn dedup_across_entries() {
        let content = "/src/ @org/team\n/docs/ @org/team @alice\n";
        let result = parse_codeowners(content);
        assert_eq!(result.unique_owners, vec!["@org/team", "@alice"]);
    }

    #[test]
    fn case_insensitive_dedup() {
        let content = "/src/ @Org/Team\n/docs/ @org/team\n";
        let result = parse_codeowners(content);
        // Only the first-seen casing is kept.
        assert_eq!(result.unique_owners, vec!["@Org/Team"]);
    }

    #[test]
    fn email_owners_excluded() {
        let content = "/src/ @org/team user@example.com\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries[0].owners, vec!["@org/team"]);
    }

    #[test]
    fn long_line_skipped() {
        let long_line = format!("{} @org/team\n", "a".repeat(MAX_LINE_LENGTH + 1));
        let content = format!("/src/ @alice\n{long_line}");
        let result = parse_codeowners(&content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].pattern, "/src/");
        assert_eq!(result.skipped_lines, 1);
    }

    #[test]
    fn skipped_lines_zero_when_all_lines_in_bounds() {
        let result = parse_codeowners("* @org/team\n/src/ @alice\n");
        assert_eq!(result.skipped_lines, 0);
    }

    #[test]
    fn skipped_lines_counts_multiple_overlength_lines() {
        let over = "a".repeat(MAX_LINE_LENGTH + 1);
        let content = format!("{over}\n* @ok\n{over}\n{over}\n");
        let result = parse_codeowners(&content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.skipped_lines, 3);
    }

    #[test]
    fn pattern_without_owners() {
        let content = "/orphan-dir/\n";
        let result = parse_codeowners(content);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].pattern, "/orphan-dir/");
        assert!(result.entries[0].owners.is_empty());
    }

    #[test]
    fn non_consecutive_duplicates_within_entry() {
        let content = "* @org/team @alice @org/team\n";
        let result = parse_codeowners(content);
        // Per-entry owners must be deduplicated even when non-consecutive.
        // After sort+dedup, order is lexicographic.
        assert_eq!(result.entries[0].owners, vec!["@alice", "@org/team"]);
        // unique_owners preserves first-seen order (from sorted entries).
        assert_eq!(result.unique_owners, vec!["@alice", "@org/team"]);
    }

    #[test]
    fn mixed_content() {
        let content = "\
# CODEOWNERS for my-org
* @org/default-team
*.js @org/frontend
/src/backend/ @org/backend @alice
/docs/ @bob # doc owner
";
        let result = parse_codeowners(content);
        assert_eq!(result.entries.len(), 4);
        assert_eq!(result.entries[0].pattern, "*");
        assert_eq!(result.entries[1].pattern, "*.js");
        assert_eq!(result.entries[2].pattern, "/src/backend/");
        assert_eq!(result.entries[3].pattern, "/docs/");
        assert_eq!(
            result.unique_owners,
            vec![
                "@org/default-team",
                "@org/frontend",
                "@alice",
                "@org/backend",
                "@bob"
            ]
        );
    }
}
