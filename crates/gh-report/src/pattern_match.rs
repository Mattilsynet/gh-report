//! Generic slash-sensitive glob/wildcard matching over byte strings.
//!
//! Domain-agnostic: operates on `&str`/`&[u8]` only, with no knowledge of
//! GitHub refs, branches, or rulesets. Callers supply their own recursion
//! depth bound for the byte-level engine, keeping this module reusable for
//! any path-shaped matching problem.

/// Match a slash-sensitive wildcard pattern against a candidate path.
///
/// Supports `*` (matches any single path segment component, excluding `/`),
/// `**` (matches zero or more path segments), and `?` (matches a single character).
///
/// `max_fnmatch_depth` bounds the byte-level recursion used within each path
/// segment, guarding against adversarial patterns (e.g. `*a*a*a*a*...`).
/// Segment-level recursion (driven by `**`) is separately bounded by an
/// internal constant to limit combinatorial path-segment expansion.
#[must_use]
pub fn path_pattern_matches(pattern: &str, candidate: &str, max_fnmatch_depth: usize) -> bool {
    let pattern_parts: Vec<&str> = pattern.split('/').collect();
    let candidate_parts: Vec<&str> = candidate.split('/').collect();
    match_path_segments(&pattern_parts, &candidate_parts, 0, max_fnmatch_depth)
}

/// Maximum recursion depth for segment-level matching.
/// Separate from fnmatch char-level depth to bound `**` combinatorics.
const MAX_SEGMENT_RECURSION_DEPTH: usize = 64;

fn match_path_segments(
    pattern_parts: &[&str],
    candidate_parts: &[&str],
    depth: usize,
    max_fnmatch_depth: usize,
) -> bool {
    if depth > MAX_SEGMENT_RECURSION_DEPTH {
        return false;
    }

    if pattern_parts.is_empty() {
        return candidate_parts.is_empty();
    }

    let Some((head, tail)) = pattern_parts.split_first() else {
        return false;
    };

    if *head == "**" {
        if tail.is_empty() {
            return true;
        }
        return (0..=candidate_parts.len()).any(|index| {
            match_path_segments(
                tail,
                &candidate_parts[index..],
                depth + 1,
                max_fnmatch_depth,
            )
        });
    }

    if candidate_parts.is_empty() {
        return false;
    }

    if !fnmatch_segment(head, candidate_parts[0], max_fnmatch_depth) {
        return false;
    }

    match_path_segments(tail, &candidate_parts[1..], depth + 1, max_fnmatch_depth)
}

/// Simple fnmatch-style matching for a single path segment.
///
/// Supports `*` (match any sequence of non-`/` chars) and `?` (match single char).
/// Consecutive `*` chars are collapsed before matching to prevent exponential
/// backtracking. Protected with a recursion depth limit as a secondary guard.
fn fnmatch_segment(pattern: &str, candidate: &str, max_depth: usize) -> bool {
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
    fnmatch_bytes(&pattern_bytes, candidate_bytes, 0, max_depth)
}

/// Byte-level fnmatch implementation.
///
/// Matching is byte-oriented rather than `char`-oriented: correct for ASCII
/// inputs and avoids the overhead of collecting into `Vec<char>>`. Callers
/// with non-ASCII candidates should validate that assumption before use.
#[must_use]
pub fn fnmatch_bytes(pattern: &[u8], candidate: &[u8], depth: usize, max_depth: usize) -> bool {
    if depth > max_depth {
        return false;
    }

    if pattern.is_empty() {
        return candidate.is_empty();
    }

    match pattern[0] {
        b'*' => (0..=candidate.len())
            .any(|i| fnmatch_bytes(&pattern[1..], &candidate[i..], depth + 1, max_depth)),
        b'?' => {
            if candidate.is_empty() {
                false
            } else {
                fnmatch_bytes(&pattern[1..], &candidate[1..], depth + 1, max_depth)
            }
        }
        ch => {
            if candidate.is_empty() || candidate[0] != ch {
                false
            } else {
                fnmatch_bytes(&pattern[1..], &candidate[1..], depth + 1, max_depth)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MAX_DEPTH: usize = 256;

    #[test]
    fn wildcard_pattern_match() {
        assert!(path_pattern_matches(
            "feature/*",
            "feature/foo",
            TEST_MAX_DEPTH
        ));
        assert!(!path_pattern_matches(
            "feature/*",
            "feature/foo/bar",
            TEST_MAX_DEPTH
        ));
    }

    #[test]
    fn double_star_matches_nested() {
        assert!(path_pattern_matches(
            "feature/**",
            "feature/foo/bar",
            TEST_MAX_DEPTH
        ));
        assert!(path_pattern_matches(
            "feature/**",
            "feature/foo",
            TEST_MAX_DEPTH
        ));
        assert!(path_pattern_matches("**", "any/path/here", TEST_MAX_DEPTH));
    }

    #[test]
    fn question_mark_matches_single_char() {
        assert!(path_pattern_matches("mai?", "main", TEST_MAX_DEPTH));
        assert!(!path_pattern_matches("mai?", "mai", TEST_MAX_DEPTH));
    }

    #[test]
    fn slash_sensitivity() {
        assert!(!path_pattern_matches(
            "release/*",
            "release/v1/hotfix",
            TEST_MAX_DEPTH
        ));
        assert!(path_pattern_matches(
            "release/**",
            "release/v1/hotfix",
            TEST_MAX_DEPTH
        ));
    }

    #[test]
    fn deeply_nested_double_star_does_not_hang() {
        let pattern = (0..20).map(|_| "**").collect::<Vec<_>>().join("/");
        let candidate = (0..10)
            .map(|i| format!("seg{i}"))
            .collect::<Vec<_>>()
            .join("/");
        let _result = path_pattern_matches(&pattern, &candidate, TEST_MAX_DEPTH);
    }

    #[test]
    fn adversarial_fnmatch_pattern_does_not_hang() {
        let pattern = "*a*a*a*a*a*a*a*a*a*a";
        let candidate = "aaaaaaaaaaaaaaaaaaaab";
        assert!(!fnmatch_segment(pattern, candidate, TEST_MAX_DEPTH));
    }
}
