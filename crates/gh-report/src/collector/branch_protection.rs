//! Branch protection evaluation.
//!
//! Evaluates branch protection from both the rulesets API and the
//! legacy branch protection API, then merges the results.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tracing::{debug, instrument, trace};

use crate::config;
use crate::domain::checks::{
    BranchControls, BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus,
    BranchRequirements, CollectionFailureReason,
};
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

/// Summarize a single ruleset's branch controls.
fn summarize_ruleset(ruleset: &serde_json::Value) -> BranchControls {
    let mut has_pr = false;
    let mut reviewer_count: u32 = 0;
    let mut has_status_checks = false;
    let mut force_push_blocked = Some(false);
    let mut deletion_blocked = Some(false);

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
            if rule_type == "non_fast_forward" {
                force_push_blocked = Some(true);
            }
            if rule_type == "deletion" {
                deletion_blocked = Some(true);
            }
        }
    }

    let has_broad_bypass = ruleset_has_broad_bypass(ruleset);

    BranchControls::new(
        BranchRequirements::new(has_pr, has_status_checks, !has_broad_bypass)
            .with_integrity_controls(force_push_blocked, deletion_blocked),
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

    let force_push_blocked = protection
        .get("allow_force_pushes")
        .and_then(|value| value.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .map(|allowed| !allowed);

    let deletion_blocked = protection
        .get("allow_deletions")
        .and_then(|value| value.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .map(|allowed| !allowed);

    BranchControls::new(
        BranchRequirements::new(has_pr, has_status_checks, admin_equivalent)
            .with_integrity_controls(force_push_blocked, deletion_blocked),
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
                    reason_kind: Some(CollectionFailureReason::Invalid),
                    http_status: None,
                    force_push_blocked: None,
                    deletion_blocked: None,
                },
                timestamp: run_timestamp.to_string(),
            };
        }
    };

    let default_branch = &repo.default_branch;
    let encoded_branch: String = utf8_percent_encode(default_branch, NON_ALPHANUMERIC).to_string();

    let rulesets_path = format!("/repos/{}/{}/rulesets", client.org_name, safe_name);
    let legacy_path = format!(
        "/repos/{}/{}/branches/{encoded_branch}/protection",
        client.org_name, safe_name
    );
    let (rulesets_result, legacy_result, repo_details_result) = tokio::join!(
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
        client.repo_details(&repo.name),
    );

    let admin = repo_admin_signal(&repo_details_result);

    let result = evaluate_outcomes(
        &rulesets_result,
        &legacy_result,
        default_branch,
        admin,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtectionEndpoint {
    Rulesets,
    Legacy,
}

impl ProtectionEndpoint {
    fn parse_payload(
        self,
        data: Option<&serde_json::Value>,
        default_branch: &str,
    ) -> Option<Vec<BranchControls>> {
        let data = data?;
        match self {
            Self::Rulesets => {
                let rulesets = data.as_array()?;
                let controls: Vec<BranchControls> = rulesets
                    .iter()
                    .filter(|ruleset| ruleset_applies(ruleset, default_branch))
                    .map(summarize_ruleset)
                    .collect();
                trace!(
                    applicable_rulesets = controls.len(),
                    total_rulesets = rulesets.len(),
                    "filtered rulesets for default branch"
                );
                Some(controls)
            }
            Self::Legacy => {
                if !data.is_object() {
                    return None;
                }
                let controls = summarize_legacy_protection(data);
                trace!(
                    has_pr = controls.has_pr(),
                    has_status_checks = controls.has_status_checks(),
                    "legacy branch protection summarized"
                );
                Some(vec![controls])
            }
        }
    }
}

/// Caller's `administration:read` signal for the repository under
/// evaluation, derived from a `repo_details` API outcome.
///
/// `Unknown` is a distinct state from `NotAdmin`: it covers a failed or
/// transient `repo_details` lookup, a missing `permissions` object, or a
/// missing `permissions.admin` field. Collapsing `Unknown` into `NotAdmin`
/// (or into a bare `bool`) would make "we could not determine admin
/// access" indistinguishable from "we determined the caller is not an
/// admin" — an illegal-state-representable defect this enum removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminAccess {
    /// `permissions.admin` was observed `true`.
    Admin,
    /// `permissions.admin` was observed `false`.
    NotAdmin,
    /// The lookup failed, was malformed, or lacked the field.
    Unknown,
}

/// Derive [`AdminAccess`] from a repo-details API outcome.
fn repo_admin_signal(repo_details_result: &crate::github::client::ApiOutcome) -> AdminAccess {
    if !repo_details_result.is_ok() {
        return AdminAccess::Unknown;
    }
    match repo_details_result
        .data()
        .and_then(|data| data.get("permissions"))
        .and_then(|permissions| permissions.get("admin"))
        .and_then(serde_json::Value::as_bool)
    {
        Some(true) => AdminAccess::Admin,
        Some(false) => AdminAccess::NotAdmin,
        None => AdminAccess::Unknown,
    }
}

enum ProtectionEvidence {
    Complete(BranchControls),
    Incomplete(EndpointFailure),
    AbsentControls {
        absence: ConfirmedAbsence,
        http_status: Option<u16>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmedAbsence {
    NoControls,
    AuthorityConfirmedNotFound,
}

impl ConfirmedAbsence {
    const fn reason_kind(self) -> Option<CollectionFailureReason> {
        match self {
            Self::NoControls => None,
            Self::AuthorityConfirmedNotFound => Some(CollectionFailureReason::NotFoundAbsent),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EndpointFailure {
    reason: IndeterminateReason,
    http_status: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndeterminateReason {
    PermissionDenied,
    RateLimited,
    PermissionSuspected,
    Transient,
    Invalid,
}

impl IndeterminateReason {
    const fn precedence(self) -> u8 {
        match self {
            Self::PermissionDenied => 0,
            Self::RateLimited => 1,
            Self::PermissionSuspected => 2,
            Self::Transient => 3,
            Self::Invalid => 4,
        }
    }

    const fn persisted(self) -> CollectionFailureReason {
        match self {
            Self::PermissionDenied => CollectionFailureReason::PermissionDenied,
            Self::RateLimited => CollectionFailureReason::RateLimited,
            Self::PermissionSuspected => CollectionFailureReason::PermissionSuspected,
            Self::Transient => CollectionFailureReason::Transient,
            Self::Invalid => CollectionFailureReason::Invalid,
        }
    }
}

enum EndpointEvidence {
    Observed(Vec<BranchControls>),
    ConfirmedAbsent { http_status: Option<u16> },
    Indeterminate(EndpointFailure),
}

fn classify_endpoint(
    outcome: &crate::github::client::ApiOutcome,
    endpoint: ProtectionEndpoint,
    default_branch: &str,
    admin: AdminAccess,
) -> EndpointEvidence {
    match outcome {
        crate::github::client::ApiOutcome::Success {
            status_code, data, ..
        } => endpoint
            .parse_payload(data.as_ref(), default_branch)
            .map_or_else(
                || {
                    EndpointEvidence::Indeterminate(EndpointFailure {
                        reason: IndeterminateReason::Invalid,
                        http_status: Some(*status_code),
                    })
                },
                EndpointEvidence::Observed,
            ),
        crate::github::client::ApiOutcome::Failure {
            status_code,
            retryable,
            ..
        } => {
            let reason = match (*status_code, admin) {
                (Some(403), _) => IndeterminateReason::PermissionDenied,
                (Some(429), _) => IndeterminateReason::RateLimited,
                (Some(404), AdminAccess::Admin) => {
                    return EndpointEvidence::ConfirmedAbsent {
                        http_status: *status_code,
                    };
                }
                (Some(404), AdminAccess::NotAdmin | AdminAccess::Unknown) => {
                    IndeterminateReason::PermissionSuspected
                }
                _ if *retryable => IndeterminateReason::Transient,
                _ => IndeterminateReason::Invalid,
            };
            EndpointEvidence::Indeterminate(EndpointFailure {
                reason,
                http_status: *status_code,
            })
        }
    }
}

fn evaluate_outcomes(
    rulesets_result: &crate::github::client::ApiOutcome,
    legacy_result: &crate::github::client::ApiOutcome,
    default_branch: &str,
    admin: AdminAccess,
    run_timestamp: &str,
) -> BranchProtectionResult {
    let evidence = classify_evidence(rulesets_result, legacy_result, default_branch, admin);
    build_protection_result(evidence, default_branch, run_timestamp)
}

fn classify_evidence(
    rulesets_result: &crate::github::client::ApiOutcome,
    legacy_result: &crate::github::client::ApiOutcome,
    default_branch: &str,
    admin: AdminAccess,
) -> ProtectionEvidence {
    let rulesets = classify_endpoint(
        rulesets_result,
        ProtectionEndpoint::Rulesets,
        default_branch,
        admin,
    );
    let legacy = classify_endpoint(
        legacy_result,
        ProtectionEndpoint::Legacy,
        default_branch,
        admin,
    );
    let endpoints = [&rulesets, &legacy];

    let indeterminate = endpoints
        .into_iter()
        .filter_map(|endpoint| match endpoint {
            EndpointEvidence::Indeterminate(failure) => Some(*failure),
            EndpointEvidence::Observed(_) | EndpointEvidence::ConfirmedAbsent { .. } => None,
        })
        .min_by_key(|failure| failure.reason.precedence());

    if let Some(failure) = indeterminate {
        return ProtectionEvidence::Incomplete(failure);
    }

    let observed: Vec<BranchControls> = endpoints
        .into_iter()
        .filter_map(|endpoint| match endpoint {
            EndpointEvidence::Observed(controls) => Some(controls.iter().cloned()),
            EndpointEvidence::ConfirmedAbsent { .. } | EndpointEvidence::Indeterminate(_) => None,
        })
        .flatten()
        .collect();

    let Some(controls) = BranchControls::merge(&observed) else {
        let (absence, http_status) = match (&rulesets, &legacy) {
            (EndpointEvidence::ConfirmedAbsent { http_status }, _)
            | (_, EndpointEvidence::ConfirmedAbsent { http_status }) => {
                (ConfirmedAbsence::AuthorityConfirmedNotFound, *http_status)
            }
            _ => (
                ConfirmedAbsence::NoControls,
                rulesets_result
                    .status_code()
                    .or_else(|| legacy_result.status_code()),
            ),
        };
        return ProtectionEvidence::AbsentControls {
            absence,
            http_status,
        };
    };

    ProtectionEvidence::Complete(controls)
}

fn build_protection_result(
    evidence: ProtectionEvidence,
    default_branch: &str,
    run_timestamp: &str,
) -> BranchProtectionResult {
    match evidence {
        ProtectionEvidence::Incomplete(failure) => BranchProtectionResult {
            status: BranchProtectionStatus::Unknown,
            details: unobserved_details(
                default_branch,
                Some(failure.reason.persisted()),
                failure.http_status,
            ),
            timestamp: run_timestamp.to_string(),
        },
        ProtectionEvidence::AbsentControls {
            absence,
            http_status,
        } => BranchProtectionResult {
            status: BranchProtectionStatus::Fail,
            details: unobserved_details(default_branch, absence.reason_kind(), http_status),
            timestamp: run_timestamp.to_string(),
        },
        ProtectionEvidence::Complete(controls) => BranchProtectionResult {
            status: controls.status(),
            details: BranchProtectionDetails {
                default_branch: default_branch.to_string(),
                has_pr: Some(controls.has_pr()),
                required_reviewers: Some(controls.reviewer_count),
                has_status_checks: Some(controls.has_status_checks()),
                admin_equivalent: Some(controls.admin_equivalent()),
                has_broad_bypass: Some(controls.has_broad_bypass()),
                reason: None,
                reason_kind: None,
                http_status: None,
                force_push_blocked: controls.force_push_blocked(),
                deletion_blocked: controls.deletion_blocked(),
            },
            timestamp: run_timestamp.to_string(),
        },
    }
}

fn unobserved_details(
    default_branch: &str,
    reason_kind: Option<CollectionFailureReason>,
    status_code: Option<u16>,
) -> BranchProtectionDetails {
    BranchProtectionDetails {
        default_branch: default_branch.to_string(),
        has_pr: None,
        required_reviewers: None,
        has_status_checks: None,
        admin_equivalent: None,
        has_broad_bypass: None,
        reason: reason_kind.map(|reason| reason.to_string()),
        reason_kind,
        http_status: status_code,
        force_push_blocked: None,
        deletion_blocked: None,
    }
}

/// Check if a ruleset applies to a given branch.
///
/// Uses the `ref_matching` module for the actual pattern matching, but
/// here we extract the fields from raw JSON since the evaluation works
/// with `serde_json::Value` directly.
fn ruleset_applies(ruleset: &serde_json::Value, default_branch: &str) -> bool {
    let target = ruleset
        .get("target")
        .and_then(serde_json::Value::as_str)
        .and_then(crate::collector::ref_matching::RulesetTarget::parse);

    let enforcement = ruleset
        .get("enforcement")
        .and_then(serde_json::Value::as_str)
        .and_then(crate::collector::ref_matching::RulesetEnforcement::parse);

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
    use crate::domain::checks::BranchProtectionTier;

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
    fn summarize_ruleset_force_push_and_deletion_rules_block_controls() {
        let ruleset = serde_json::json!({
            "rules": [
                {"type": "non_fast_forward"},
                {"type": "deletion"}
            ],
            "bypass_actors": []
        });

        let controls = summarize_ruleset(&ruleset);

        assert_eq!(controls.force_push_blocked(), Some(true));
        assert_eq!(controls.deletion_blocked(), Some(true));
    }

    #[test]
    fn summarize_ruleset_missing_force_push_and_deletion_rules_reports_unblocked() {
        let ruleset = serde_json::json!({
            "rules": [
                {"type": "pull_request", "parameters": {"required_approving_review_count": 1}}
            ],
            "bypass_actors": []
        });

        let controls = summarize_ruleset(&ruleset);

        assert_eq!(controls.force_push_blocked(), Some(false));
        assert_eq!(controls.deletion_blocked(), Some(false));
        assert_eq!(controls.tier(), BranchProtectionTier::BelowBaseline);
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
    fn summarize_legacy_inverts_allow_force_pushes_and_deletions() {
        let protection = serde_json::json!({
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": true}
        });

        let controls = summarize_legacy_protection(&protection);

        assert_eq!(controls.force_push_blocked(), Some(true));
        assert_eq!(controls.deletion_blocked(), Some(false));
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
    fn private_404_with_admin_true_is_genuine_absence() {
        let not_found =
            crate::github::client::ApiOutcome::failure(Some(404), "not found".to_string(), false);

        let result = evaluate_outcomes(
            &not_found,
            &not_found,
            "main",
            AdminAccess::Admin,
            "2026-06-17T11:31:04Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Fail);
        assert_eq!(result.details.reason.as_deref(), Some("not_found_absent"));
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::NotFoundAbsent)
        );
        assert_eq!(result.details.http_status, Some(404));
        assert_eq!(result.details.force_push_blocked, None);
        assert_eq!(result.details.deletion_blocked, None);
    }

    #[test]
    fn private_403_is_still_permission_denied_regardless_of_admin() {
        let denied =
            crate::github::client::ApiOutcome::failure(Some(403), "forbidden".to_string(), false);

        let result = evaluate_outcomes(
            &denied,
            &denied,
            "main",
            AdminAccess::NotAdmin,
            "2026-06-17T11:31:04Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionDenied)
        );
    }

    #[test]
    fn not_admin_404_without_controls_is_permission_suspected() {
        let not_found =
            crate::github::client::ApiOutcome::failure(Some(404), "not found".to_string(), false);

        let result = evaluate_outcomes(
            &not_found,
            &not_found,
            "main",
            AdminAccess::NotAdmin,
            "2026-06-17T11:31:04Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason.as_deref(),
            Some("permission_suspected")
        );
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionSuspected)
        );
        assert_eq!(result.details.http_status, Some(404));
    }

    #[test]
    fn unknown_admin_from_inconclusive_lookup_404_fails_closed_to_permission_suspected() {
        let not_found =
            crate::github::client::ApiOutcome::failure(Some(404), "not found".to_string(), false);

        let result = evaluate_outcomes(
            &not_found,
            &not_found,
            "main",
            AdminAccess::Unknown,
            "2026-06-17T11:31:04Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason.as_deref(),
            Some("permission_suspected")
        );
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionSuspected)
        );
        assert_eq!(result.details.http_status, Some(404));
    }

    #[test]
    fn classify_endpoint_403_unchanged_regardless_of_admin() {
        let denied =
            crate::github::client::ApiOutcome::failure(Some(403), "forbidden".to_string(), false);

        for admin in [
            AdminAccess::NotAdmin,
            AdminAccess::Admin,
            AdminAccess::Unknown,
        ] {
            match classify_endpoint(&denied, ProtectionEndpoint::Rulesets, "main", admin) {
                EndpointEvidence::Indeterminate(failure) => {
                    assert_eq!(failure.reason, IndeterminateReason::PermissionDenied);
                    assert_eq!(failure.http_status, Some(403));
                }
                _ => panic!("403 must classify as an indeterminate denial"),
            }
        }
    }

    #[test]
    fn every_indeterminate_reason_persists_as_an_indeterminate_collection_reason() {
        for reason in [
            IndeterminateReason::PermissionDenied,
            IndeterminateReason::RateLimited,
            IndeterminateReason::PermissionSuspected,
            IndeterminateReason::Transient,
            IndeterminateReason::Invalid,
        ] {
            assert!(reason.persisted().is_indeterminate(), "{reason:?}");
        }
    }

    #[test]
    fn classify_endpoint_maps_every_unmodelled_failure_to_an_indeterminate_reason() {
        let cases = [
            (Some(401), false, IndeterminateReason::Invalid),
            (Some(422), false, IndeterminateReason::Invalid),
            (None, false, IndeterminateReason::Invalid),
            (Some(500), true, IndeterminateReason::Transient),
            (None, true, IndeterminateReason::Transient),
        ];

        for (status, retryable, expected) in cases {
            let outcome =
                crate::github::client::ApiOutcome::failure(status, "failed".to_string(), retryable);
            match classify_endpoint(
                &outcome,
                ProtectionEndpoint::Rulesets,
                "main",
                AdminAccess::Admin,
            ) {
                EndpointEvidence::Indeterminate(failure) => {
                    assert_eq!(failure.reason, expected, "status {status:?}");
                    assert_eq!(failure.http_status, status);
                    assert!(failure.reason.persisted().is_indeterminate());
                }
                _ => panic!("failure {status:?} must never classify as observed or absent"),
            }
        }
    }

    #[test]
    fn unauthenticated_rulesets_with_readable_legacy_protection_is_not_scored() {
        let rulesets_unauthorized = crate::github::client::ApiOutcome::failure(
            Some(401),
            "bad credentials".to_string(),
            false,
        );
        let legacy = crate::github::client::ApiOutcome::success(serde_json::json!({
            "required_pull_request_reviews": { "required_approving_review_count": 2 },
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": false}
        }));

        let result = evaluate_outcomes(
            &rulesets_unauthorized,
            &legacy,
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::Invalid)
        );
        assert_eq!(result.details.http_status, Some(401));
        assert_eq!(result.details.has_pr, None);
    }

    #[test]
    fn asymmetric_failure_persists_the_failing_endpoint_status_not_the_readable_one() {
        let rulesets = applicable_ruleset_outcome();
        let legacy_denied =
            crate::github::client::ApiOutcome::failure(Some(403), "forbidden".to_string(), false);

        let denied_legacy = evaluate_outcomes(
            &rulesets,
            &legacy_denied,
            "main",
            AdminAccess::NotAdmin,
            "2026-08-31T00:00:00Z",
        );
        assert_eq!(denied_legacy.details.http_status, Some(403));

        let legacy = crate::github::client::ApiOutcome::success(serde_json::json!({
            "required_pull_request_reviews": { "required_approving_review_count": 2 }
        }));
        let denied_rulesets =
            crate::github::client::ApiOutcome::failure(Some(403), "forbidden".to_string(), false);

        let denied_ruleset_side = evaluate_outcomes(
            &denied_rulesets,
            &legacy,
            "main",
            AdminAccess::NotAdmin,
            "2026-08-31T00:00:00Z",
        );
        assert_eq!(denied_ruleset_side.details.http_status, Some(403));
    }

    #[test]
    fn confirmed_absence_never_renders_an_indeterminate_reason() {
        assert_eq!(ConfirmedAbsence::NoControls.reason_kind(), None);
        assert_eq!(
            ConfirmedAbsence::AuthorityConfirmedNotFound.reason_kind(),
            Some(CollectionFailureReason::NotFoundAbsent)
        );
        for absence in [
            ConfirmedAbsence::NoControls,
            ConfirmedAbsence::AuthorityConfirmedNotFound,
        ] {
            assert!(
                !absence
                    .reason_kind()
                    .is_some_and(CollectionFailureReason::is_indeterminate)
            );
        }
    }

    #[test]
    fn repo_admin_signal_is_unknown_when_repo_details_lookup_failed() {
        let failed = crate::github::client::ApiOutcome::failure(
            Some(500),
            "internal server error".to_string(),
            true,
        );
        assert_eq!(repo_admin_signal(&failed), AdminAccess::Unknown);
    }

    #[test]
    fn repo_admin_signal_is_unknown_when_permissions_admin_missing() {
        let success = crate::github::client::ApiOutcome::success(serde_json::json!({
            "permissions": {}
        }));
        assert_eq!(repo_admin_signal(&success), AdminAccess::Unknown);
    }

    #[test]
    fn repo_admin_signal_is_unknown_when_permissions_object_absent() {
        let success = crate::github::client::ApiOutcome::success(serde_json::json!({}));
        assert_eq!(repo_admin_signal(&success), AdminAccess::Unknown);
    }

    #[test]
    fn repo_admin_signal_is_admin_when_permissions_admin_true() {
        let success = crate::github::client::ApiOutcome::success(serde_json::json!({
            "permissions": { "admin": true }
        }));
        assert_eq!(repo_admin_signal(&success), AdminAccess::Admin);
    }

    #[test]
    fn repo_admin_signal_is_not_admin_when_permissions_admin_false() {
        let success = crate::github::client::ApiOutcome::success(serde_json::json!({
            "permissions": { "admin": false }
        }));
        assert_eq!(repo_admin_signal(&success), AdminAccess::NotAdmin);
    }

    #[test]
    fn legacy_404_with_failed_repo_details_lookup_is_permission_suspected_not_genuine_absence() {
        let not_found =
            crate::github::client::ApiOutcome::failure(Some(404), "not found".to_string(), false);
        let repo_details_failed = crate::github::client::ApiOutcome::failure(
            Some(500),
            "internal server error".to_string(),
            true,
        );

        let admin = repo_admin_signal(&repo_details_failed);
        let result = evaluate_outcomes(
            &not_found,
            &not_found,
            "main",
            admin,
            "2026-06-17T11:31:04Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionSuspected)
        );
        assert_eq!(result.details.http_status, Some(404));
    }

    fn applicable_ruleset_outcome() -> crate::github::client::ApiOutcome {
        crate::github::client::ApiOutcome::success(serde_json::json!([
            {
                "target": "branch",
                "enforcement": "active",
                "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
                "rules": [
                    {"type": "pull_request", "parameters": {"required_approving_review_count": 1}},
                    {"type": "non_fast_forward"},
                    {"type": "deletion"}
                ],
                "bypass_actors": []
            }
        ]))
    }

    #[test]
    fn readable_rulesets_with_denied_legacy_endpoint_is_not_scored_from_partial_evidence() {
        let rulesets = applicable_ruleset_outcome();
        let legacy_denied =
            crate::github::client::ApiOutcome::failure(Some(403), "forbidden".to_string(), false);

        let result = evaluate_outcomes(
            &rulesets,
            &legacy_denied,
            "main",
            AdminAccess::NotAdmin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionDenied)
        );
    }

    #[test]
    fn denied_rulesets_with_readable_legacy_protection_is_also_indeterminate() {
        let rulesets_denied =
            crate::github::client::ApiOutcome::failure(Some(403), "forbidden".to_string(), false);
        let legacy = crate::github::client::ApiOutcome::success(serde_json::json!({
            "required_pull_request_reviews": { "required_approving_review_count": 2 },
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": false}
        }));

        let result = evaluate_outcomes(
            &rulesets_denied,
            &legacy,
            "main",
            AdminAccess::NotAdmin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionDenied)
        );
        assert_eq!(result.details.has_pr, None);
        assert_eq!(result.details.force_push_blocked, None);
    }

    #[test]
    fn readable_rulesets_with_legacy_404_under_non_admin_is_permission_suspected() {
        let rulesets = applicable_ruleset_outcome();
        let legacy_not_found =
            crate::github::client::ApiOutcome::failure(Some(404), "not found".to_string(), false);

        let result = evaluate_outcomes(
            &rulesets,
            &legacy_not_found,
            "main",
            AdminAccess::NotAdmin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::PermissionSuspected)
        );
    }

    #[test]
    fn readable_rulesets_with_legacy_404_under_admin_still_scores_as_genuine_absence() {
        let rulesets = applicable_ruleset_outcome();
        let legacy_not_found =
            crate::github::client::ApiOutcome::failure(Some(404), "not found".to_string(), false);

        let result = evaluate_outcomes(
            &rulesets,
            &legacy_not_found,
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Pass);
        assert_eq!(result.details.reason_kind, None);
        assert_eq!(result.details.has_pr, Some(true));
        assert_eq!(result.details.force_push_blocked, Some(true));
    }

    fn readable_legacy_outcome() -> crate::github::client::ApiOutcome {
        crate::github::client::ApiOutcome::success(serde_json::json!({
            "required_pull_request_reviews": { "required_approving_review_count": 2 },
            "required_status_checks": { "contexts": ["ci/build"] },
            "enforce_admins": { "enabled": true },
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": false}
        }))
    }

    fn success_without_payload() -> crate::github::client::ApiOutcome {
        crate::github::client::ApiOutcome::Success {
            status_code: 200,
            data: None,
            headers: None,
            truncated: false,
        }
    }

    #[test]
    fn wrong_shaped_rulesets_payload_is_indeterminate_despite_readable_legacy_sibling() {
        let rulesets_wrong_shape =
            crate::github::client::ApiOutcome::success(serde_json::json!({"rulesets": "nope"}));

        let result = evaluate_outcomes(
            &rulesets_wrong_shape,
            &readable_legacy_outcome(),
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::Invalid)
        );
        assert_eq!(result.details.http_status, Some(200));
        assert_eq!(result.details.has_pr, None);
    }

    #[test]
    fn absent_rulesets_payload_is_indeterminate_despite_readable_legacy_sibling() {
        let result = evaluate_outcomes(
            &success_without_payload(),
            &readable_legacy_outcome(),
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::Invalid)
        );
        assert_eq!(result.details.has_pr, None);
    }

    #[test]
    fn wrong_shaped_legacy_payload_is_indeterminate_despite_readable_rulesets_sibling() {
        let legacy_wrong_shape =
            crate::github::client::ApiOutcome::success(serde_json::json!(["not", "an", "object"]));

        let result = evaluate_outcomes(
            &applicable_ruleset_outcome(),
            &legacy_wrong_shape,
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::Invalid)
        );
        assert_eq!(result.details.http_status, Some(200));
        assert_eq!(result.details.has_pr, None);
    }

    #[test]
    fn absent_legacy_payload_is_indeterminate_despite_readable_rulesets_sibling() {
        let result = evaluate_outcomes(
            &applicable_ruleset_outcome(),
            &success_without_payload(),
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert_eq!(result.status, BranchProtectionStatus::Unknown);
        assert_eq!(
            result.details.reason_kind,
            Some(CollectionFailureReason::Invalid)
        );
        assert_eq!(result.details.has_pr, None);
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
