//! Branch protection evaluation.
//!
//! Evaluates history protection (force-push and branch-deletion blocking on
//! the default branch) from both the rulesets API and the legacy branch
//! protection API, then merges the results into a closed
//! [`BranchProtectionResult`].

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tracing::{debug, instrument, trace};

use crate::config;
use crate::domain::checks::{
    BranchProtectionResult, HistoryProtectionGap, HistorySignals, IndeterminateReason,
};
use crate::domain::repository::Repository;
use crate::github::client::GitHubClient;
use cherry_pit_web::sanitize_path_segment;

/// Summarize a single ruleset's history signals.
///
/// A ruleset that grants force pushes or deletions is treated as not
/// blocking that signal even when other rulesets block it; the merge in
/// [`HistorySignals::merge`] takes any block as definitive.
fn summarize_ruleset(ruleset: &serde_json::Value) -> HistorySignals {
    let mut force_push_blocked = Some(false);
    let mut deletion_blocked = Some(false);

    if let Some(rules) = ruleset.get("rules").and_then(serde_json::Value::as_array) {
        for rule in rules {
            let rule_type = rule
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if rule_type == "non_fast_forward" {
                force_push_blocked = Some(true);
            }
            if rule_type == "deletion" {
                deletion_blocked = Some(true);
            }
        }
    }

    HistorySignals {
        force_push_blocked,
        deletion_blocked,
    }
}

/// Summarize legacy branch protection into history signals.
///
/// The legacy API reports `allow_force_pushes` and `allow_deletions` in
/// "allowed" polarity; the signals invert them into "blocked" polarity.
fn summarize_legacy_protection(protection: &serde_json::Value) -> HistorySignals {
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

    HistorySignals {
        force_push_blocked,
        deletion_blocked,
    }
}

/// Evaluate branch protection for a repository.
///
/// 1. Fetch rulesets and legacy protection concurrently.
/// 2. Filter rulesets that apply to the default branch.
/// 3. Summarize each applicable ruleset and the legacy protection into
///    history signals.
/// 4. Merge all signals and determine the final outcome.
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
            return BranchProtectionResult::Unobservable {
                default_branch: repo.default_branch.clone(),
                reason: IndeterminateReason::Invalid,
                http_status: None,
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
        default_branch = %result.default_branch(),
        protected = result.is_protected(),
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
    ) -> Option<Vec<HistorySignals>> {
        let data = data?;
        match self {
            Self::Rulesets => {
                let rulesets = data.as_array()?;
                let signals: Vec<HistorySignals> = rulesets
                    .iter()
                    .filter(|ruleset| ruleset_applies(ruleset, default_branch))
                    .map(summarize_ruleset)
                    .collect();
                trace!(
                    applicable_rulesets = signals.len(),
                    total_rulesets = rulesets.len(),
                    "filtered rulesets for default branch"
                );
                Some(signals)
            }
            Self::Legacy => {
                if !data.is_object() {
                    return None;
                }
                let signals = summarize_legacy_protection(data);
                trace!(
                    force_push_blocked = ?signals.force_push_blocked,
                    deletion_blocked = ?signals.deletion_blocked,
                    "legacy branch protection summarized"
                );
                Some(vec![signals])
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
    Complete(HistorySignals),
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
    const fn confirmed_not_found(self) -> bool {
        matches!(self, Self::AuthorityConfirmedNotFound)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EndpointFailure {
    reason: IndeterminateReason,
    http_status: Option<u16>,
}

impl EndpointFailure {
    const fn precedence(self) -> u8 {
        match self.reason {
            IndeterminateReason::PermissionDenied => 0,
            IndeterminateReason::RateLimited => 1,
            IndeterminateReason::PermissionSuspected => 2,
            IndeterminateReason::Transient => 3,
            IndeterminateReason::Invalid => 4,
            IndeterminateReason::Pending => 5,
        }
    }
}

enum EndpointEvidence {
    Observed(Vec<HistorySignals>),
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
    build_protection_result(&evidence, default_branch, run_timestamp)
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
        .min_by_key(|failure| failure.precedence());

    if let Some(failure) = indeterminate {
        return ProtectionEvidence::Incomplete(failure);
    }

    let observed: Vec<HistorySignals> = endpoints
        .into_iter()
        .filter_map(|endpoint| match endpoint {
            EndpointEvidence::Observed(signals) => Some(signals.iter().copied()),
            EndpointEvidence::ConfirmedAbsent { .. } | EndpointEvidence::Indeterminate(_) => None,
        })
        .flatten()
        .collect();

    let Some(signals) = HistorySignals::merge(&observed) else {
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

    ProtectionEvidence::Complete(signals)
}

fn build_protection_result(
    evidence: &ProtectionEvidence,
    default_branch: &str,
    run_timestamp: &str,
) -> BranchProtectionResult {
    match evidence {
        ProtectionEvidence::Incomplete(failure) => BranchProtectionResult::Unobservable {
            default_branch: default_branch.to_string(),
            reason: failure.reason,
            http_status: failure.http_status,
            timestamp: run_timestamp.to_string(),
        },
        ProtectionEvidence::AbsentControls {
            absence,
            http_status,
        } => BranchProtectionResult::Unprotected {
            default_branch: default_branch.to_string(),
            gap: HistoryProtectionGap::Absent {
                confirmed_not_found: absence.confirmed_not_found(),
                http_status: *http_status,
            },
            timestamp: run_timestamp.to_string(),
        },
        ProtectionEvidence::Complete(signals) => BranchProtectionResult::from_history_signals(
            default_branch,
            signals.force_push_blocked,
            signals.deletion_blocked,
            run_timestamp,
        ),
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
    use crate::domain::checks::HistoryProtectionGap;

    #[test]
    fn summarize_ruleset_force_push_and_deletion_rules_block_signal() {
        let ruleset = serde_json::json!({
            "rules": [
                {"type": "non_fast_forward"},
                {"type": "deletion"}
            ]
        });

        let signals = summarize_ruleset(&ruleset);

        assert_eq!(signals.force_push_blocked, Some(true));
        assert_eq!(signals.deletion_blocked, Some(true));
    }

    #[test]
    fn summarize_ruleset_missing_rules_reports_unblocked() {
        let ruleset = serde_json::json!({
            "rules": [
                {"type": "pull_request", "parameters": {"required_approving_review_count": 1}}
            ]
        });

        let signals = summarize_ruleset(&ruleset);

        assert_eq!(signals.force_push_blocked, Some(false));
        assert_eq!(signals.deletion_blocked, Some(false));
    }

    #[test]
    fn summarize_ruleset_no_rules() {
        let ruleset = serde_json::json!({"rules": []});
        let signals = summarize_ruleset(&ruleset);
        assert_eq!(signals.force_push_blocked, Some(false));
        assert_eq!(signals.deletion_blocked, Some(false));
    }

    #[test]
    fn summarize_legacy_inverts_allow_force_pushes_and_deletions() {
        let protection = serde_json::json!({
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": true}
        });

        let signals = summarize_legacy_protection(&protection);

        assert_eq!(signals.force_push_blocked, Some(true));
        assert_eq!(signals.deletion_blocked, Some(false));
    }

    #[test]
    fn summarize_legacy_no_protection() {
        let protection = serde_json::json!({});
        let signals = summarize_legacy_protection(&protection);
        assert_eq!(signals.force_push_blocked, None);
        assert_eq!(signals.deletion_blocked, None);
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

        let gap = result.gap().unwrap();
        assert_eq!(
            gap,
            HistoryProtectionGap::Absent {
                confirmed_not_found: true,
                http_status: Some(404)
            }
        );
        assert!(gap.is_absent());
        assert_eq!(gap.http_status(), Some(404));
        assert!(result.is_observed());
        assert!(!result.is_protected());
        assert_eq!(
            result.score_category(),
            crate::domain::checks::ScoreCategory::Fail
        );
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

        assert!(!result.is_observed());
        assert_eq!(
            result.http_status(),
            Some(403),
            "denied endpoints must surface the 403"
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(404));
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(404));
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
                EndpointEvidence::Observed(_) | EndpointEvidence::ConfirmedAbsent { .. } => {
                    panic!("403 must classify as an indeterminate denial")
                }
            }
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
                }
                EndpointEvidence::Observed(_) | EndpointEvidence::ConfirmedAbsent { .. } => {
                    panic!("failure {status:?} must never classify as observed or absent")
                }
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(401));
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
        assert_eq!(denied_legacy.http_status(), Some(403));

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
        assert_eq!(denied_ruleset_side.http_status(), Some(403));
    }

    #[test]
    fn confirmed_absence_never_renders_an_unobservable_outcome() {
        assert!(!ConfirmedAbsence::NoControls.confirmed_not_found());
        assert!(ConfirmedAbsence::AuthorityConfirmedNotFound.confirmed_not_found());
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(404));
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
                ]
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(403));
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(403));
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(404));
    }

    #[test]
    fn readable_rulesets_with_legacy_404_under_admin_still_scores_as_protected() {
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

        assert!(result.is_protected());
        assert!(result.gap().is_none());
    }

    #[test]
    fn readable_rulesets_blocking_only_force_push_is_unprotected_deletion_allowed() {
        let rulesets = crate::github::client::ApiOutcome::success(serde_json::json!([
            {
                "target": "branch",
                "enforcement": "active",
                "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
                "rules": [
                    {"type": "non_fast_forward"}
                ]
            }
        ]));
        let legacy_fp_blocked = crate::github::client::ApiOutcome::success(serde_json::json!({
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": true}
        }));

        let result = evaluate_outcomes(
            &rulesets,
            &legacy_fp_blocked,
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert!(!result.is_protected());
        assert_eq!(result.gap(), Some(HistoryProtectionGap::DeletionAllowed));
    }

    #[test]
    fn readable_legacy_full_protection() {
        let legacy = crate::github::client::ApiOutcome::success(serde_json::json!({
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": false}
        }));

        let result = evaluate_outcomes(
            &crate::github::client::ApiOutcome::success(serde_json::json!([])),
            &legacy,
            "main",
            AdminAccess::Admin,
            "2026-08-31T00:00:00Z",
        );

        assert!(result.is_protected());
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(200));
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

        assert!(!result.is_observed());
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

        assert!(!result.is_observed());
        assert_eq!(result.http_status(), Some(200));
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

        assert!(!result.is_observed());
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

    fn readable_legacy_outcome() -> crate::github::client::ApiOutcome {
        crate::github::client::ApiOutcome::success(serde_json::json!({
            "required_pull_request_reviews": { "required_approving_review_count": 2 },
            "required_status_checks": { "contexts": ["ci/build"] },
            "enforce_admins": { "enabled": true },
            "allow_force_pushes": {"enabled": false},
            "allow_deletions": {"enabled": false}
        }))
    }
}
