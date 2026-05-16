//! Branch protection evaluation.
//!
//! Evaluates branch protection from both the rulesets API and the
//! legacy branch protection API, then merges the results.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tracing::{debug, instrument, trace};

use crate::config;
use crate::domain::checks::{
    BranchControls, BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus,
    BranchRequirements,
};
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

/// Summarize a single ruleset's branch controls.
fn summarize_ruleset(ruleset: &serde_json::Value) -> BranchControls {
    let mut has_pr = false;
    let mut reviewer_count: u32 = 0;
    let mut has_status_checks = false;

    if let Some(rules) = ruleset.get("rules").and_then(serde_json::Value::as_array) {
        for rule in rules {
            let rule_type = rule
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let params = rule.get("parameters").unwrap_or(&serde_json::Value::Null);

            if rule_type == "pull_request" || rule_type == "required_pull_request_reviews" {
                has_pr = true;
                reviewer_count = reviewer_count.max(reviewer_count_from_value(
                    params.get("required_approving_review_count"),
                ));
            }
            if rule_type == "required_status_checks" {
                let has_checks = params
                    .get("required_checks")
                    .or_else(|| params.get("required_status_checks"))
                    .or_else(|| params.get("contexts"))
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|arr| !arr.is_empty());
                if has_checks {
                    has_status_checks = true;
                }
            }
        }
    }

    let has_broad_bypass = ruleset_has_broad_bypass(ruleset);

    BranchControls::new(
        BranchRequirements::new(has_pr, has_status_checks, !has_broad_bypass),
        reviewer_count,
        has_broad_bypass,
    )
}

/// Parse a required reviewer count and saturate it to `u32::MAX`.
fn reviewer_count_from_value(value: Option<&serde_json::Value>) -> u32 {
    value
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX))
}

/// Check if a ruleset has broad bypass actors.
///
/// Returns `true` if any bypass actor is an `OrganizationAdmin` or `RepositoryRole`.
fn ruleset_has_broad_bypass(ruleset: &serde_json::Value) -> bool {
    ruleset
        .get("bypass_actors")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|actors| {
            actors.iter().any(|actor| {
                matches!(
                    actor.get("actor_type").and_then(serde_json::Value::as_str),
                    Some("OrganizationAdmin" | "RepositoryRole")
                )
            })
        })
}

/// Summarize legacy branch protection into `BranchControls`.
///
/// Extracts controls from GitHub's legacy branch protection API response.
fn summarize_legacy_protection(protection: &serde_json::Value) -> BranchControls {
    let pr_reviews = protection.get("required_pull_request_reviews");
    let has_pr = pr_reviews.is_some_and(|v| !v.is_null());

    let reviewer_count = pr_reviews
        .and_then(serde_json::Value::as_object)
        .map_or(0, |pr| {
            reviewer_count_from_value(pr.get("required_approving_review_count"))
        });

    let status_checks = protection.get("required_status_checks");
    let has_status_checks = status_checks
        .and_then(serde_json::Value::as_object)
        .is_some_and(|sc| {
            let checks = sc
                .get("checks")
                .or_else(|| sc.get("contexts"))
                .and_then(serde_json::Value::as_array);
            checks.is_some_and(|arr| !arr.is_empty())
        });

    let admin_equivalent = protection
        .get("enforce_admins")
        .and_then(|ea| ea.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    BranchControls::new(
        BranchRequirements::new(has_pr, has_status_checks, admin_equivalent),
        reviewer_count,
        false,
    )
}

/// Evaluate branch protection for a repository.
///
/// Evaluates branch protection for a repository:
/// 1. Fetch rulesets and legacy branch protection concurrently.
/// 2. Filter rulesets that apply to the default branch.
/// 3. Summarize each applicable ruleset and the legacy protection.
/// 4. Merge all controls and determine the final status.
#[instrument(skip_all, fields(repo = %repo.name))]
pub async fn evaluate(
    client: &GitHubClient,
    repo: &Repository,
    run_timestamp: &str,
) -> BranchProtectionResult {
    trace!(repo = %repo.name, default_branch = %repo.default_branch, "evaluating branch protection");

    // Validate repo name before URL interpolation — defense-in-depth against
    // path injection from API-derived data.
    let safe_name = match sanitize_path_segment(&repo.name, "repo_name") {
        Ok(n) => n,
        Err(e) => {
            debug!(repo = %repo.name, error = %e, "skipping branch protection: invalid repo name");
            return BranchProtectionResult {
                status: BranchProtectionStatus::Unknown,
                details: BranchProtectionDetails {
                    default_branch: repo.default_branch.clone(),
                    has_pr: None,
                    required_reviewers: None,
                    has_status_checks: None,
                    admin_equivalent: None,
                    has_broad_bypass: None,
                    reason: Some("invalid_repo_name".to_string()),
                },
                timestamp: run_timestamp.to_string(),
            };
        }
    };

    let default_branch = &repo.default_branch;
    // default_branch is percent-encoded with NON_ALPHANUMERIC — `/` becomes `%2F`,
    // preventing path traversal even though it appears in a path segment position.
    let encoded_branch: String = utf8_percent_encode(default_branch, NON_ALPHANUMERIC).to_string();

    // Run rulesets and legacy branch protection API calls concurrently.
    // These are completely independent calls — parallelizing reduces
    // BP check wall time by ~2x.
    let rulesets_path = format!("/repos/{}/{}/rulesets", client.org_name, safe_name);
    let legacy_path = format!(
        "/repos/{}/{}/branches/{encoded_branch}/protection",
        client.org_name, safe_name
    );
    let (rulesets_result, legacy_result) = tokio::join!(
        client.request(
            &rulesets_path,
            false,
            config::DEFAULT_MAX_RETRIES,
            config::DEFAULT_REQUEST_TIMEOUT_SECS,
        ),
        client.request(
            &legacy_path,
            false,
            config::DEFAULT_MAX_RETRIES,
            config::DEFAULT_REQUEST_TIMEOUT_SECS,
        ),
    );

    let combined = collect_and_merge_controls(&rulesets_result, &legacy_result, default_branch);

    let result = build_protection_result(
        combined,
        &rulesets_result,
        &legacy_result,
        default_branch,
        run_timestamp,
    );

    debug!(
        repo = %repo.name,
        status = %result.status,
        has_pr = ?result.details.has_pr,
        required_reviewers = ?result.details.required_reviewers,
        "branch protection evaluation complete"
    );

    result
}

/// Collect applicable ruleset + legacy controls and merge them.
fn collect_and_merge_controls(
    rulesets_result: &crate::github::client::ApiOutcome,
    legacy_result: &crate::github::client::ApiOutcome,
    default_branch: &str,
) -> Option<BranchControls> {
    let mut ruleset_controls: Vec<BranchControls> = Vec::new();
    if rulesets_result.is_ok()
        && let Some(rulesets) = rulesets_result.data().and_then(serde_json::Value::as_array)
    {
        for ruleset in rulesets {
            if ruleset_applies(ruleset, default_branch) {
                ruleset_controls.push(summarize_ruleset(ruleset));
            }
        }
        trace!(
            applicable_rulesets = ruleset_controls.len(),
            total_rulesets = rulesets.len(),
            "filtered rulesets for default branch"
        );
    }

    let merged_ruleset = BranchControls::merge(&ruleset_controls);

    let legacy_controls = if legacy_result.is_ok() {
        legacy_result.data().map(|data| {
            let controls = summarize_legacy_protection(data);
            trace!(
                has_pr = controls.has_pr(),
                has_status_checks = controls.has_status_checks(),
                "legacy branch protection summarized"
            );
            controls
        })
    } else {
        None
    };

    let all_controls: Vec<BranchControls> =
        merged_ruleset.into_iter().chain(legacy_controls).collect();
    BranchControls::merge(&all_controls)
}

/// Build the final `BranchProtectionResult` from merged controls.
fn build_protection_result(
    combined: Option<BranchControls>,
    rulesets_result: &crate::github::client::ApiOutcome,
    legacy_result: &crate::github::client::ApiOutcome,
    default_branch: &str,
    run_timestamp: &str,
) -> BranchProtectionResult {
    match combined {
        None => {
            let reason = if rulesets_result.status_code() == Some(403)
                || legacy_result.status_code() == Some(403)
            {
                Some("permission_denied".to_string())
            } else if rulesets_result.is_retryable() || legacy_result.is_retryable() {
                Some("transient_error".to_string())
            } else {
                None
            };

            let status = if reason.is_some() {
                BranchProtectionStatus::Unknown
            } else {
                BranchProtectionStatus::Fail
            };

            BranchProtectionResult {
                status,
                details: BranchProtectionDetails {
                    default_branch: default_branch.to_string(),
                    has_pr: None,
                    required_reviewers: None,
                    has_status_checks: None,
                    admin_equivalent: None,
                    has_broad_bypass: None,
                    reason,
                },
                timestamp: run_timestamp.to_string(),
            }
        }
        Some(controls) => {
            let status = controls.status();
            BranchProtectionResult {
                status,
                details: BranchProtectionDetails {
                    default_branch: default_branch.to_string(),
                    has_pr: Some(controls.has_pr()),
                    required_reviewers: Some(controls.reviewer_count),
                    has_status_checks: Some(controls.has_status_checks()),
                    admin_equivalent: Some(controls.admin_equivalent()),
                    has_broad_bypass: Some(controls.has_broad_bypass()),
                    reason: None,
                },
                timestamp: run_timestamp.to_string(),
            }
        }
    }
}

/// Check if a ruleset applies to a given branch.
///
/// Uses the `ref_matching` module for the actual pattern matching, but
/// here we extract the fields from raw JSON since the evaluation works
/// with `serde_json::Value` directly.
fn ruleset_applies(ruleset: &serde_json::Value, default_branch: &str) -> bool {
    let target = ruleset.get("target").and_then(serde_json::Value::as_str);

    let enforcement = ruleset
        .get("enforcement")
        .and_then(serde_json::Value::as_str);

    let ref_name = ruleset.get("conditions").and_then(|c| c.get("ref_name"));

    let extract_patterns = |key: &str| -> Vec<String> {
        ref_name
            .and_then(|r| r.get(key))
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };

    let include = extract_patterns("include");
    let exclude = extract_patterns("exclude");

    crate::collector::ref_matching::ruleset_applies_to_branch(
        target,
        enforcement,
        &include,
        &exclude,
        default_branch,
        default_branch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_ruleset_pr_and_status_checks() {
        let ruleset = serde_json::json!({
            "rules": [
                {
                    "type": "pull_request",
                    "parameters": {
                        "required_approving_review_count": 2
                    }
                },
                {
                    "type": "required_status_checks",
                    "parameters": {
                        "required_checks": [{"context": "ci"}]
                    }
                }
            ],
            "bypass_actors": []
        });
        let controls = summarize_ruleset(&ruleset);
        assert!(controls.has_pr());
        assert_eq!(controls.reviewer_count, 2);
        assert!(controls.has_status_checks());
        assert!(controls.admin_equivalent());
        assert!(!controls.has_broad_bypass());
    }

    #[test]
    fn summarize_ruleset_with_broad_bypass() {
        let ruleset = serde_json::json!({
            "rules": [
                {"type": "pull_request", "parameters": {"required_approving_review_count": 1}}
            ],
            "bypass_actors": [
                {"actor_type": "OrganizationAdmin", "actor_id": 1}
            ]
        });
        let controls = summarize_ruleset(&ruleset);
        assert!(controls.has_pr());
        assert!(!controls.admin_equivalent());
        assert!(controls.has_broad_bypass());
    }

    #[test]
    fn summarize_ruleset_no_rules() {
        let ruleset = serde_json::json!({"rules": [], "bypass_actors": []});
        let controls = summarize_ruleset(&ruleset);
        assert!(!controls.has_pr());
        assert_eq!(controls.reviewer_count, 0);
        assert!(!controls.has_status_checks());
    }

    #[test]
    fn summarize_legacy_full_protection() {
        let protection = serde_json::json!({
            "required_pull_request_reviews": {
                "required_approving_review_count": 1
            },
            "required_status_checks": {
                "checks": [{"context": "ci"}]
            },
            "enforce_admins": {
                "enabled": true
            }
        });
        let controls = summarize_legacy_protection(&protection);
        assert!(controls.has_pr());
        assert_eq!(controls.reviewer_count, 1);
        assert!(controls.has_status_checks());
        assert!(controls.admin_equivalent());
        assert!(!controls.has_broad_bypass());
    }

    #[test]
    fn summarize_legacy_no_protection() {
        let protection = serde_json::json!({});
        let controls = summarize_legacy_protection(&protection);
        assert!(!controls.has_pr());
        assert_eq!(controls.reviewer_count, 0);
        assert!(!controls.has_status_checks());
        assert!(!controls.admin_equivalent());
    }

    #[test]
    fn summarize_legacy_contexts_fallback() {
        let protection = serde_json::json!({
            "required_status_checks": {
                "contexts": ["ci/build"]
            }
        });
        let controls = summarize_legacy_protection(&protection);
        assert!(controls.has_status_checks());
    }

    #[test]
    fn ruleset_has_broad_bypass_org_admin() {
        let ruleset = serde_json::json!({
            "bypass_actors": [{"actor_type": "OrganizationAdmin"}]
        });
        assert!(ruleset_has_broad_bypass(&ruleset));
    }

    #[test]
    fn ruleset_has_broad_bypass_repo_role() {
        let ruleset = serde_json::json!({
            "bypass_actors": [{"actor_type": "RepositoryRole"}]
        });
        assert!(ruleset_has_broad_bypass(&ruleset));
    }

    #[test]
    fn ruleset_has_no_broad_bypass() {
        let ruleset = serde_json::json!({
            "bypass_actors": [{"actor_type": "Team"}]
        });
        assert!(!ruleset_has_broad_bypass(&ruleset));
    }

    #[test]
    fn ruleset_has_no_bypass_actors() {
        let ruleset = serde_json::json!({});
        assert!(!ruleset_has_broad_bypass(&ruleset));
    }

    #[test]
    fn ruleset_applies_active_branch_target() {
        let ruleset = serde_json::json!({
            "target": "branch",
            "enforcement": "active",
            "conditions": {
                "ref_name": {
                    "include": ["~DEFAULT_BRANCH"],
                    "exclude": []
                }
            }
        });
        assert!(ruleset_applies(&ruleset, "main"));
    }

    #[test]
    fn ruleset_does_not_apply_disabled() {
        let ruleset = serde_json::json!({
            "target": "branch",
            "enforcement": "disabled",
            "conditions": {
                "ref_name": {
                    "include": ["~ALL"],
                    "exclude": []
                }
            }
        });
        assert!(!ruleset_applies(&ruleset, "main"));
    }

    #[test]
    fn ruleset_does_not_apply_tag_target() {
        let ruleset = serde_json::json!({
            "target": "tag",
            "enforcement": "active",
            "conditions": {
                "ref_name": {
                    "include": ["~ALL"],
                    "exclude": []
                }
            }
        });
        assert!(!ruleset_applies(&ruleset, "main"));
    }

    #[test]
    fn summarize_ruleset_required_pull_request_reviews_type() {
        // Both "pull_request" and "required_pull_request_reviews" are valid PR rule types
        let ruleset = serde_json::json!({
            "rules": [
                {
                    "type": "required_pull_request_reviews",
                    "parameters": {"required_approving_review_count": 3}
                }
            ],
            "bypass_actors": []
        });
        let controls = summarize_ruleset(&ruleset);
        assert!(controls.has_pr());
        assert_eq!(controls.reviewer_count, 3);
    }
}
