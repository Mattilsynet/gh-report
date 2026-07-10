//! Ref and branch name matching for GitHub rulesets.
//!
//! Implements `~ALL`, `~DEFAULT_BRANCH`, exact matches, and full-ref
//! matching, delegating slash-sensitive wildcard matching to
//! [`crate::pattern_match`].

use std::fmt;

use crate::config;
use crate::pattern_match;

/// Ref type a GitHub ruleset targets.
///
/// Mirrors the closed `target` enum in GitHub's repository-ruleset wire
/// schema (`branch` | `tag` | `push`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesetTarget {
    /// The ruleset targets branch refs.
    Branch,
    /// The ruleset targets tag refs.
    Tag,
    /// The ruleset targets pushes (push rulesets).
    Push,
}

impl RulesetTarget {
    /// Parse a GitHub wire-format target string.
    ///
    /// Returns `None` for any value outside the closed set; callers treat an
    /// unrecognized target as not matching any known ruleset target.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "branch" => Some(Self::Branch),
            "tag" => Some(Self::Tag),
            "push" => Some(Self::Push),
            _ => None,
        }
    }
}

impl fmt::Display for RulesetTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Branch => write!(f, "branch"),
            Self::Tag => write!(f, "tag"),
            Self::Push => write!(f, "push"),
        }
    }
}

/// Enforcement state of a GitHub ruleset.
///
/// Mirrors the closed `enforcement` enum in GitHub's repository-ruleset wire
/// schema (`disabled` | `active` | `evaluate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RulesetEnforcement {
    /// The ruleset is defined but not enforced.
    Disabled,
    /// The ruleset is actively enforced.
    Active,
    /// The ruleset runs in dry-run mode without blocking.
    Evaluate,
}

impl RulesetEnforcement {
    /// Parse a GitHub wire-format enforcement string.
    ///
    /// Returns `None` for any value outside the closed set; callers treat an
    /// unrecognized enforcement as not active.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "disabled" => Some(Self::Disabled),
            "active" => Some(Self::Active),
            "evaluate" => Some(Self::Evaluate),
            _ => None,
        }
    }
}

impl fmt::Display for RulesetEnforcement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "disabled"),
            Self::Active => write!(f, "active"),
            Self::Evaluate => write!(f, "evaluate"),
        }
    }
}

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
            if pattern == branch_ref || pattern == branch_name {
                return true;
            }

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
/// Thin wrapper over [`pattern_match::path_pattern_matches`] supplying this
/// crate's configured recursion bound; see that function for the pattern
/// grammar.
#[must_use]
pub fn path_pattern_matches(pattern: &str, candidate: &str) -> bool {
    pattern_match::path_pattern_matches(pattern, candidate, config::FNMATCH_MAX_RECURSION_DEPTH)
}

/// Check if a ruleset applies to a given branch.
#[must_use]
pub fn ruleset_applies_to_branch(
    target: Option<RulesetTarget>,
    enforcement: Option<RulesetEnforcement>,
    include: &[String],
    exclude: &[String],
    branch: &str,
    default_branch: &str,
) -> bool {
    if target != Some(RulesetTarget::Branch) {
        return false;
    }
    if !matches!(enforcement, None | Some(RulesetEnforcement::Active)) {
        return false;
    }

    let include_patterns: &[String] = if include.is_empty() {
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
        assert!(!ref_name_matches("release/*", "release/v1/hotfix", "main"));
        assert!(ref_name_matches("release/**", "release/v1/hotfix", "main"));
    }

    #[test]
    fn ruleset_applies_basic() {
        assert!(ruleset_applies_to_branch(
            Some(RulesetTarget::Branch),
            Some(RulesetEnforcement::Active),
            &[],
            &[],
            "main",
            "main",
        ));
    }

    #[test]
    fn ruleset_skips_non_branch_target() {
        assert!(!ruleset_applies_to_branch(
            Some(RulesetTarget::Tag),
            Some(RulesetEnforcement::Active),
            &[],
            &[],
            "main",
            "main",
        ));
    }

    #[test]
    fn ruleset_skips_disabled_enforcement() {
        assert!(!ruleset_applies_to_branch(
            Some(RulesetTarget::Branch),
            Some(RulesetEnforcement::Disabled),
            &[],
            &[],
            "main",
            "main",
        ));
    }

    #[test]
    fn ruleset_exclude_takes_precedence() {
        assert!(!ruleset_applies_to_branch(
            Some(RulesetTarget::Branch),
            Some(RulesetEnforcement::Active),
            &["~ALL".to_string()],
            &["main".to_string()],
            "main",
            "main",
        ));
    }

    #[test]
    fn deeply_nested_double_star_does_not_hang() {
        let pattern = (0..20).map(|_| "**").collect::<Vec<_>>().join("/");
        let candidate = (0..10)
            .map(|i| format!("seg{i}"))
            .collect::<Vec<_>>()
            .join("/");
        let _result = path_pattern_matches(&pattern, &candidate);
    }
}
