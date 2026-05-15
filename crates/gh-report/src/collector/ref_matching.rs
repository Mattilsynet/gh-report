//! Ref and branch name matching for GitHub rulesets.
//!
//! Implements `~ALL`, `~DEFAULT_BRANCH`, exact matches, full refs,
//! and slash-sensitive wildcard matching. This logic is implemented
//! directly and tested heavily rather than delegated to a generic glob crate.

use crate::config;

/// Normalize a branch name to a full ref (refs/heads/...).
#[must_use]
pub fn normalize_branch_ref(branch: &str) -> String {
    if branch.starts_with("refs/heads/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    }
}

/// Match a GitHub ruleset ref-name pattern against a candidate branch.
///
/// `~DEFAULT_BRANCH` matches only if the candidate matches the repository's
/// default branch. `~ALL` matches everything. Otherwise, exact match or
/// path-pattern matching is used.
#[must_use]
pub fn ref_name_matches(pattern: &str, branch: &str, default_branch: &str) -> bool {
    let branch_ref = normalize_branch_ref(branch);
    let branch_name = branch_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(&branch_ref);
    let default_branch_ref = normalize_branch_ref(default_branch);

    match pattern {
        "~ALL" => true,
        "~DEFAULT_BRANCH" => branch_ref == default_branch_ref,
        _ => {
            // Exact match against full ref or branch name
            if pattern == branch_ref || pattern == branch_name {
                return true;
            }

            // Pattern matching: if pattern starts with "refs/", match against full ref;
            // otherwise match against branch name.
            let candidate = if pattern.starts_with("refs/") {
                &branch_ref
            } else {
                branch_name
            };
            path_pattern_matches(pattern, candidate)
        }
    }
}

/// Match a slash-sensitive wildcard pattern against a candidate path.
///
/// Supports `*` (matches any single path segment component, excluding `/`),
/// `**` (matches zero or more path segments), and `?` (matches a single character).
///
/// Protected against combinatorial explosion with a recursion depth limit.
#[must_use]
pub fn path_pattern_matches(pattern: &str, candidate: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let candidate_parts: Vec<&str> = candidate.split('/').collect();
    match_path_segments(&pattern_parts, &candidate_parts, 0)
}

/// Maximum recursion depth for segment-level matching.
/// Separate from fnmatch char-level depth to bound `**` combinatorics.
const MAX_SEGMENT_RECURSION_DEPTH: usize = 64;

fn match_path_segments(pattern_parts: &[&str], candidate_parts: &[&str], depth: usize) -> bool {
    if depth > MAX_SEGMENT_RECURSION_DEPTH {
        // Safety bail-out: pattern is too complex, treat as non-match
        return false;
    }

    if pattern_parts.is_empty() {
        return candidate_parts.is_empty();
    }

    // SAFETY: `pattern_parts.is_empty()` is checked above, so split_first
    // always returns Some. Using expect() for the lint gate.
    let Some((head, tail)) = pattern_parts.split_first() else {
        return false;
    };

    if *head == "**" {
        if tail.is_empty() {
            return true;
        }
        return (0..=candidate_parts.len())
            .any(|index| match_path_segments(tail, &candidate_parts[index..], depth + 1));
    }

    if candidate_parts.is_empty() {
        return false;
    }

    if !fnmatch_segment(head, candidate_parts[0]) {
        return false;
    }

    match_path_segments(tail, &candidate_parts[1..], depth + 1)
}

/// Simple fnmatch-style matching for a single path segment.
///
/// Supports `*` (match any sequence of non-`/` chars) and `?` (match single char).
/// Consecutive `*` chars are collapsed before matching to prevent exponential
/// backtracking. Protected with a recursion depth limit as a secondary guard.
fn fnmatch_segment(pattern: &str, candidate: &str) -> bool {
    // Collapse consecutive '*' to eliminate exponential branching.
    let pattern_bytes: Vec<u8> = {
        let raw = pattern.as_bytes();
        let mut out = Vec::with_capacity(raw.len());
        for &b in raw {
            if b == b'*' && out.last() == Some(&b'*') {
                continue;
            }
            out.push(b);
        }
        out
    };
    let candidate_bytes = candidate.as_bytes();
    fnmatch_bytes(&pattern_bytes, candidate_bytes, 0)
}

/// Byte-level fnmatch implementation.
///
/// GitHub ref names are ASCII, so byte-level matching is safe and avoids
/// the overhead of `chars().collect::<Vec<char>>()`.
fn fnmatch_bytes(pattern: &[u8], candidate: &[u8], depth: usize) -> bool {
    if depth > config::FNMATCH_MAX_RECURSION_DEPTH {
        // Safety bail-out: pattern is too complex, treat as non-match
        return false;
    }

    if pattern.is_empty() {
        return candidate.is_empty();
    }

    match pattern[0] {
        b'*' => {
            // '*' can match zero or more characters
            (0..=candidate.len()).any(|i| fnmatch_bytes(&pattern[1..], &candidate[i..], depth + 1))
        }
        b'?' => {
            if candidate.is_empty() {
                false
            } else {
                fnmatch_bytes(&pattern[1..], &candidate[1..], depth + 1)
            }
        }
        ch => {
            if candidate.is_empty() || candidate[0] != ch {
                false
            } else {
                fnmatch_bytes(&pattern[1..], &candidate[1..], depth + 1)
            }
        }
    }
}

/// Check if a ruleset applies to a given branch.
#[must_use]
pub fn ruleset_applies_to_branch(
    target: Option<&str>,
    enforcement: Option<&str>,
    include: &[String],
    exclude: &[String],
    branch: &str,
    default_branch: &str,
) -> bool {
    if target != Some("branch") {
        return false;
    }
    if !matches!(enforcement, None | Some("active")) {
        return false;
    }

    let include_patterns: &[String] = if include.is_empty() {
        // Default: include all.
        &[String::from("~ALL")]
    } else {
        include
    };

    let included = include_patterns
        .iter()
        .any(|p| ref_name_matches(p, branch, default_branch));
    let excluded = exclude
        .iter()
        .any(|p| ref_name_matches(p, branch, default_branch));

    included && !excluded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_prefix() {
        assert_eq!(normalize_branch_ref("main"), "refs/heads/main");
    }

    #[test]
    fn normalize_preserves_existing_prefix() {
        assert_eq!(normalize_branch_ref("refs/heads/main"), "refs/heads/main");
    }

    #[test]
    fn tilde_all_matches_everything() {
        assert!(ref_name_matches("~ALL", "main", "main"));
        assert!(ref_name_matches("~ALL", "feature/foo", "main"));
        assert!(ref_name_matches("~ALL", "refs/heads/develop", "main"));
    }

    #[test]
    fn tilde_default_branch_matches_default_only() {
        assert!(ref_name_matches("~DEFAULT_BRANCH", "main", "main"));
        assert!(!ref_name_matches("~DEFAULT_BRANCH", "develop", "main"));
        assert!(ref_name_matches(
            "~DEFAULT_BRANCH",
            "refs/heads/main",
            "main"
        ));
    }

    #[test]
    fn exact_branch_name_match() {
        assert!(ref_name_matches("main", "main", "develop"));
        assert!(!ref_name_matches("main", "develop", "develop"));
    }

    #[test]
    fn exact_full_ref_match() {
        assert!(ref_name_matches("refs/heads/main", "main", "develop"));
        assert!(ref_name_matches(
            "refs/heads/main",
            "refs/heads/main",
            "develop"
        ));
    }

    #[test]
    fn wildcard_pattern_match() {
        assert!(ref_name_matches("feature/*", "feature/foo", "main"));
        assert!(!ref_name_matches("feature/*", "feature/foo/bar", "main"));
    }

    #[test]
    fn double_star_matches_nested() {
        assert!(ref_name_matches("feature/**", "feature/foo/bar", "main"));
        assert!(ref_name_matches("feature/**", "feature/foo", "main"));
        assert!(ref_name_matches("**", "any/path/here", "main"));
    }

    #[test]
    fn question_mark_matches_single_char() {
        assert!(ref_name_matches("mai?", "main", "develop"));
        assert!(!ref_name_matches("mai?", "mai", "develop"));
    }

    #[test]
    fn slash_sensitivity() {
        // Single * should not cross slash boundaries
        assert!(!ref_name_matches("release/*", "release/v1/hotfix", "main"));
        assert!(ref_name_matches("release/**", "release/v1/hotfix", "main"));
    }

    #[test]
    fn ruleset_applies_basic() {
        assert!(ruleset_applies_to_branch(
            Some("branch"),
            Some("active"),
            &[],
            &[],
            "main",
            "main",
        ));
    }

    #[test]
    fn ruleset_skips_non_branch_target() {
        assert!(!ruleset_applies_to_branch(
            Some("tag"),
            Some("active"),
            &[],
            &[],
            "main",
            "main",
        ));
    }

    #[test]
    fn ruleset_skips_disabled_enforcement() {
        assert!(!ruleset_applies_to_branch(
            Some("branch"),
            Some("disabled"),
            &[],
            &[],
            "main",
            "main",
        ));
    }

    #[test]
    fn ruleset_exclude_takes_precedence() {
        assert!(!ruleset_applies_to_branch(
            Some("branch"),
            Some("active"),
            &["~ALL".to_string()],
            &["main".to_string()],
            "main",
            "main",
        ));
    }

    #[test]
    fn deeply_nested_double_star_does_not_hang() {
        // This pattern would cause combinatorial explosion without a recursion limit.
        // 20 ** segments with a 10-segment candidate creates massive branching.
        let pattern = (0..20).map(|_| "**").collect::<Vec<_>>().join("/");
        let candidate = (0..10)
            .map(|i| format!("seg{i}"))
            .collect::<Vec<_>>()
            .join("/");
        // Should complete quickly (either match or bail out), not hang.
        let _result = path_pattern_matches(&pattern, &candidate);
    }

    #[test]
    fn adversarial_fnmatch_pattern_does_not_hang() {
        // Adversarial pattern: many * chars matched against a long candidate
        // that doesn't end with 'b'. This triggers exponential backtracking
        // in naive recursive matchers.
        let pattern = "*a*a*a*a*a*a*a*a*a*a";
        let candidate = "aaaaaaaaaaaaaaaaaaaab";
        // Should return false without hanging
        assert!(!fnmatch_segment(pattern, candidate));
    }
}
