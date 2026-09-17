//! GHAS scanning: secret scanning evaluation and org-level alert aggregation.
//!
//! Evaluates per-repository secret scanning enablement and correlates
//! with org-level open alert observability.

use std::collections::HashMap;
use std::sync::Arc;

use jiff::Timestamp;

use tracing::{debug, instrument, trace, warn};

use crate::config;
use crate::domain::checks::{SecretScanningResult, SecretScanningStatus};
use crate::domain::metrics::OrgAlertSummary;
use crate::domain::repository::Repository;
use crate::domain::status::CollectionStatus;
use crate::domain::time::parse_iso8601;
use crate::github::client::{ApiOutcome, GitHubClient};
use cherry_pit_web::sanitize_path_segment;

/// Derive a scope key from a repository for alert correlation.
///
/// Uses the repository's `id` field, which is always populated during
/// inventory construction (either from the API numeric ID or a legacy
/// synthetic key).
#[must_use]
pub fn scope_key(repo: &Repository) -> String {
    repo.id.clone()
}

/// Create an empty age-bucket map initialised to zero.
#[must_use]
pub fn empty_age_buckets() -> HashMap<String, u64> {
    config::empty_age_buckets()
}

/// Classify an alert's age into a bucket label.
///
/// Returns `None` if `created_at` cannot be parsed; unparseable timestamps
/// are routed into the "unknown" bucket
/// at the call site.
fn age_bucket(created_at: &str, now: Timestamp) -> Option<&'static str> {
    let created = parse_iso8601(created_at)?;
    let age_days = (now.duration_since(created).as_secs() / 86_400).max(0);
    let age_days_u64 = u64::try_from(age_days).ok()?;
    for &(label, min_days, max_days) in config::SECRET_ALERT_AGE_BUCKETS {
        if age_days_u64 < min_days {
            continue;
        }
        if max_days.is_none_or(|max| age_days_u64 <= max) {
            return Some(label);
        }
    }
    None
}

/// Normalise an ISO 8601 timestamp to a canonical UTC form (no fractional seconds).
fn normalize_iso8601(value: &str) -> Option<String> {
    let ts = parse_iso8601(value)?;
    Some(ts.strftime("%Y-%m-%dT%H:%M:%S%:z").to_string())
}

/// Track the oldest/newest timestamps, updating in place.
fn track_timestamp(current: &mut Option<String>, candidate: &str, keep_oldest: bool) {
    match current {
        None => *current = Some(candidate.to_string()),
        Some(existing) => {
            if (keep_oldest && candidate < existing.as_str())
                || (!keep_oldest && candidate > existing.as_str())
            {
                *current = Some(candidate.to_string());
            }
        }
    }
}

/// Build the degraded summary for a failed or truncated org-level alert
/// request.
///
/// Used both for outright failure and for a truncated-but-technically
/// successful paginated fetch (`result.is_ok()` is `true` but
/// `result.is_truncated()` is also `true`) — a partial alert list is not
/// safe to report as a complete [`OrgAlertSummary`] (ghr-3ede27c2, mirrors
/// the H1 fix at [`crate::collector::team_membership::org_members_from_outcome`]).
fn build_failure_summary(result: &ApiOutcome) -> OrgAlertSummary {
    let collection_status = match result.status_code() {
        Some(403) => CollectionStatus::PermissionDenied,
        Some(404) => CollectionStatus::Unavailable,
        _ if result.is_retryable() => CollectionStatus::TransientError,
        _ => CollectionStatus::Unavailable,
    };
    let collection_reason = match collection_status {
        CollectionStatus::PermissionDenied => Some("permission_denied".to_string()),
        CollectionStatus::TransientError => Some("transient_error".to_string()),
        _ => Some("alerts_unavailable".to_string()),
    };
    OrgAlertSummary {
        collection_status,
        collection_reason,
        per_repo: HashMap::new(),
        open_secret_alert_age_buckets: empty_age_buckets(),
        total_open_secret_alerts: 0,
        oldest_open_secret_alert_created_at: None,
        newest_open_secret_alert_created_at: None,
    }
}

enum AlertCorrelation<'a> {
    Matched(&'a Arc<Repository>),
    OutOfScope,
    Conflicting,
    Unidentifiable,
}

fn correlate_alert<'a>(
    alert: &serde_json::Value,
    known_scope: &HashMap<String, &'a Arc<Repository>>,
) -> AlertCorrelation<'a> {
    let Some(repository) = alert
        .get("repository")
        .and_then(serde_json::Value::as_object)
    else {
        return AlertCorrelation::Unidentifiable;
    };

    let num_id = repository
        .get("id")
        .and_then(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(String::from))
        })
        .filter(|id| !id.is_empty());
    let node_id = repository
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .filter(|nid| !nid.is_empty());

    let numeric_match = num_id.as_ref().and_then(|id| known_scope.get(id));
    let node_match = node_id.and_then(|nid| known_scope.get(nid));

    match (numeric_match, node_match) {
        (Some(repo_num), Some(repo_node)) if repo_num.id != repo_node.id => {
            warn!(
                num_id = ?num_id,
                node_id = ?node_id,
                repo_num_id = %repo_num.id,
                repo_node_id = %repo_node.id,
                "conflicting repository correlation between numeric id and node_id; degrading summary to prevent false alert-free classification"
            );
            AlertCorrelation::Conflicting
        }
        (Some(repo), _) | (_, Some(repo)) => AlertCorrelation::Matched(repo),
        (None, None) if num_id.is_none() && node_id.is_none() => AlertCorrelation::Unidentifiable,
        (None, None) => {
            trace!(
                num_id = ?num_id,
                node_id = ?node_id,
                "skipping alert for repository outside inventory scope"
            );
            AlertCorrelation::OutOfScope
        }
    }
}

/// Process a single alert item and update the summary in place.
fn process_alert(
    alert: &serde_json::Value,
    known_scope: &HashMap<String, &Arc<Repository>>,
    summary: &mut OrgAlertSummary,
    now: Timestamp,
) {
    let repo = match correlate_alert(alert, known_scope) {
        AlertCorrelation::Matched(repo) => repo,
        AlertCorrelation::OutOfScope => return,
        AlertCorrelation::Conflicting => {
            summary.collection_status = CollectionStatus::Unavailable;
            summary.collection_reason = Some("alert_coordinate_conflict".to_string());
            return;
        }
        AlertCorrelation::Unidentifiable => {
            warn!(
                "alert item carries no usable repository correlation; degrading summary rather than treating an absent per-repo entry as an authoritative zero"
            );
            summary.collection_status = CollectionStatus::Unavailable;
            summary.collection_reason = Some("alert_correlation_unidentifiable".to_string());
            return;
        }
    };

    let key = scope_key(repo);
    let repo_summary = summary.per_repo.entry(key).or_default();
    repo_summary.open_alert_count += 1;
    summary.total_open_secret_alerts += 1;

    if let Some(created_at_str) = alert.get("created_at").and_then(serde_json::Value::as_str) {
        if let Some(normalised) = normalize_iso8601(created_at_str) {
            track_timestamp(
                &mut repo_summary.oldest_open_alert_created_at,
                &normalised,
                true,
            );
            track_timestamp(
                &mut repo_summary.newest_open_alert_created_at,
                &normalised,
                false,
            );
            track_timestamp(
                &mut summary.oldest_open_secret_alert_created_at,
                &normalised,
                true,
            );
            track_timestamp(
                &mut summary.newest_open_secret_alert_created_at,
                &normalised,
                false,
            );
        }

        let bucket_key = age_bucket(created_at_str, now)
            .unwrap_or(config::SECRET_ALERT_UNKNOWN_AGE_BUCKET)
            .to_string();
        *summary
            .open_secret_alert_age_buckets
            .entry(bucket_key)
            .or_insert(0) += 1;
    } else {
        *summary
            .open_secret_alert_age_buckets
            .entry(config::SECRET_ALERT_UNKNOWN_AGE_BUCKET.to_string())
            .or_insert(0) += 1;
    }
}

/// Collect org-level secret scanning alerts and build a per-repository summary.
///
/// Collects org-level secret scanning alerts.
pub async fn collect_org_alerts(
    client: &GitHubClient,
    repositories: &[Arc<Repository>],
    run_timestamp: &str,
) -> OrgAlertSummary {
    debug!(
        repos = repositories.len(),
        "collecting org-level secret scanning alerts"
    );
    let now = parse_iso8601(run_timestamp).unwrap_or_else(Timestamp::now);

    let mut known_scope: HashMap<String, &Arc<Repository>> = HashMap::new();
    let mut scope_collision = false;
    for repo in repositories {
        let key = scope_key(repo);
        if let Some(existing) = known_scope.get(&key).filter(|e| e.id != repo.id) {
            warn!(
                key = %key,
                repo1 = %existing.name,
                repo2 = %repo.name,
                "duplicate scope key conflict detected across distinct repositories"
            );
            scope_collision = true;
        }
        known_scope.insert(key, repo);
        if let Some(existing) = repo
            .node_id
            .as_ref()
            .and_then(|nid| known_scope.get(nid))
            .filter(|e| e.id != repo.id)
        {
            warn!(
                repo1 = %existing.name,
                repo2 = %repo.name,
                "duplicate node_id alias conflict detected across distinct repositories"
            );
            scope_collision = true;
        }
        if let Some(ref node_id) = repo.node_id {
            known_scope.insert(node_id.clone(), repo);
        }
    }

    let result = client
        .request(
            &format!(
                "/orgs/{}/secret-scanning/alerts?state=open&per_page={}&hide_secret=true",
                client.org_name,
                config::DEFAULT_PAGE_SIZE
            ),
            true,
            1,
            60,
        )
        .await;

    if !result.is_ok() || result.is_truncated() {
        warn!(
            status = ?result.status_code(),
            retryable = result.is_retryable(),
            truncated = result.is_truncated(),
            "org-level secret scanning alert collection failed or truncated — degrading rather than reporting a partial list as complete"
        );
        return build_failure_summary(&result);
    }

    let alert_items = match result {
        ApiOutcome::Success {
            data: Some(serde_json::Value::Array(items)),
            ..
        } => items,
        _ => Vec::new(),
    };

    debug!(
        alert_count = alert_items.len(),
        "org-level secret scanning alerts fetched"
    );

    let (init_status, init_reason) = if scope_collision {
        (
            CollectionStatus::Unavailable,
            Some("repository_alias_collision".to_string()),
        )
    } else {
        (CollectionStatus::Success, None)
    };

    let mut summary = OrgAlertSummary {
        collection_status: init_status,
        collection_reason: init_reason,
        per_repo: HashMap::new(),
        open_secret_alert_age_buckets: empty_age_buckets(),
        total_open_secret_alerts: 0,
        oldest_open_secret_alert_created_at: None,
        newest_open_secret_alert_created_at: None,
    };

    for alert in &alert_items {
        process_alert(alert, &known_scope, &mut summary, now);
    }

    debug!(
        total_open = summary.total_open_secret_alerts,
        repos_with_alerts = summary.per_repo.len(),
        "org alert collection complete"
    );
    summary
}

/// Extract the `security_and_analysis.secret_scanning.status` field.
fn extract_status(repo_details: &ApiOutcome) -> Option<SecretScanningStatus> {
    let data = repo_details.data()?;
    let status_str = data
        .get("security_and_analysis")
        .and_then(|sa| sa.get("secret_scanning"))
        .and_then(|ss| ss.get("status"))
        .and_then(serde_json::Value::as_str)?;
    match status_str {
        "enabled" => Some(SecretScanningStatus::Enabled),
        "disabled" => Some(SecretScanningStatus::Disabled),
        _ => None,
    }
}

fn build_result(
    status: SecretScanningStatus,
    timestamp: &str,
    has_open_alerts: Option<bool>,
    alerts_observable: bool,
    reason: Option<&str>,
) -> SecretScanningResult {
    debug_assert!(
        !alerts_observable || status == SecretScanningStatus::Enabled,
        "secret scanning invariant violated: alerts_observable=true with status={status:?}"
    );
    SecretScanningResult {
        status,
        has_open_alerts,
        alerts_observable,
        reason: reason.map(String::from),
        timestamp: timestamp.to_string(),
    }
}

/// Evaluate secret scanning for a repository.
///
/// When `org_summary` is `Some`, org-level alerts are the only alert
/// observability source. Repository metadata still determines the
/// enabled/disabled state so coverage denominators remain stable.
///
/// When `org_summary` is `None`, falls back to the per-repo alert endpoint.
///
/// Evaluates secret scanning status for a repository with org alert correlation.
#[instrument(skip_all, fields(repo = %repo.name))]
pub async fn evaluate(
    client: &GitHubClient,
    repo: &Repository,
    run_timestamp: &str,
    org_summary: Option<&OrgAlertSummary>,
) -> SecretScanningResult {
    trace!(repo = %repo.name, has_org_summary = org_summary.is_some(), "evaluating secret scanning");

    let safe_name = match sanitize_path_segment(&repo.name, "repo_name") {
        Ok(n) => n,
        Err(e) => {
            debug!(repo = %repo.name, error = %e, "skipping secret scanning: invalid repo name");
            return build_result(
                SecretScanningStatus::Unknown,
                run_timestamp,
                None,
                false,
                Some("invalid_repo_name"),
            );
        }
    };

    let repo_details = client.repo_details(&safe_name).await;
    let direct_status = if repo_details.is_ok() {
        extract_status(&repo_details)
    } else {
        None
    };

    if let Some(summary) = org_summary {
        let result = evaluate_with_org_summary(repo, run_timestamp, summary, direct_status);
        debug!(repo = %repo.name, status = %result.status, alerts_observable = result.alerts_observable, "secret scanning evaluation complete (org path)");
        return result;
    }

    let result = evaluate_fallback(
        &repo_details,
        client,
        &safe_name,
        run_timestamp,
        direct_status,
    )
    .await;
    debug!(repo = %repo.name, status = %result.status, alerts_observable = result.alerts_observable, "secret scanning evaluation complete (fallback path)");
    result
}

/// Fallback evaluation using the per-repo alert endpoint.
///
/// # Precondition
///
/// `safe_name` must be validated via [`sanitize_path_segment`] by the caller.
async fn evaluate_fallback(
    repo_details: &ApiOutcome,
    client: &GitHubClient,
    safe_name: &str,
    run_timestamp: &str,
    direct_status: Option<SecretScanningStatus>,
) -> SecretScanningResult {
    let alerts_path = format!(
        "/repos/{}/{}/secret-scanning/alerts?state=open&per_page=1",
        client.org_name, safe_name
    );
    let alerts = client
        .request(
            &alerts_path,
            false,
            config::DEFAULT_MAX_RETRIES,
            config::DEFAULT_REQUEST_TIMEOUT_SECS,
        )
        .await;

    if alerts.is_ok() {
        let has_items = alerts
            .data()
            .and_then(serde_json::Value::as_array)
            .is_some_and(|arr| !arr.is_empty());
        return build_result(
            SecretScanningStatus::Enabled,
            run_timestamp,
            Some(has_items),
            true,
            None,
        );
    }

    if alerts.status_code() == Some(404) {
        return match direct_status {
            Some(s) => build_result(s, run_timestamp, None, false, Some("alerts_unavailable")),
            None => build_result(
                SecretScanningStatus::Unknown,
                run_timestamp,
                None,
                false,
                Some("alerts_unavailable"),
            ),
        };
    }

    if alerts.status_code() == Some(403) || repo_details.status_code() == Some(403) {
        return match direct_status {
            Some(s) => build_result(
                s,
                run_timestamp,
                None,
                false,
                Some("alerts_permission_denied"),
            ),
            None => build_result(
                SecretScanningStatus::PermissionDenied,
                run_timestamp,
                None,
                false,
                Some("permission_denied"),
            ),
        };
    }

    if alerts.is_retryable() || repo_details.is_retryable() {
        return match direct_status {
            Some(s) => build_result(
                s,
                run_timestamp,
                None,
                false,
                Some("alerts_transient_error"),
            ),
            None => build_result(
                SecretScanningStatus::Unknown,
                run_timestamp,
                None,
                false,
                Some("transient_error"),
            ),
        };
    }

    match direct_status {
        Some(s) => build_result(s, run_timestamp, None, false, Some("alerts_unavailable")),
        None => build_result(
            SecretScanningStatus::Unknown,
            run_timestamp,
            None,
            false,
            Some("insufficient_evidence"),
        ),
    }
}

/// Evaluate when an org-level alert summary is available.
fn evaluate_with_org_summary(
    repo: &Repository,
    run_timestamp: &str,
    summary: &OrgAlertSummary,
    direct_status: Option<SecretScanningStatus>,
) -> SecretScanningResult {
    let repo_summary = summary.per_repo.get(&scope_key(repo));
    let open_alert_count = repo_summary.map_or(0, |s| s.open_alert_count);
    let status_mismatch =
        open_alert_count > 0 && direct_status == Some(SecretScanningStatus::Disabled);

    match summary.collection_status {
        CollectionStatus::Success => evaluate_org_success(
            run_timestamp,
            direct_status,
            open_alert_count,
            status_mismatch,
        ),
        CollectionStatus::PermissionDenied => {
            evaluate_org_permission_denied(run_timestamp, direct_status)
        }
        CollectionStatus::TransientError => {
            evaluate_org_transient_error(run_timestamp, direct_status)
        }
        CollectionStatus::Unavailable | CollectionStatus::NotCollected => {
            let reason = summary
                .collection_reason
                .as_deref()
                .unwrap_or("alerts_unavailable");
            match direct_status {
                Some(s) => build_result(s, run_timestamp, None, false, Some(reason)),
                None => build_result(
                    SecretScanningStatus::Unknown,
                    run_timestamp,
                    None,
                    false,
                    Some(reason),
                ),
            }
        }
    }
}

fn evaluate_org_success(
    run_timestamp: &str,
    direct_status: Option<SecretScanningStatus>,
    open_alert_count: u64,
    status_mismatch: bool,
) -> SecretScanningResult {
    match direct_status {
        Some(SecretScanningStatus::Enabled) => build_result(
            SecretScanningStatus::Enabled,
            run_timestamp,
            Some(open_alert_count > 0),
            true,
            None,
        ),
        Some(SecretScanningStatus::Disabled) => build_result(
            SecretScanningStatus::Disabled,
            run_timestamp,
            None,
            false,
            if status_mismatch {
                Some("status_mismatch")
            } else {
                None
            },
        ),
        Some(other) => build_result(
            other,
            run_timestamp,
            None,
            false,
            Some("alerts_unavailable"),
        ),
        None => build_result(
            SecretScanningStatus::Unknown,
            run_timestamp,
            None,
            false,
            Some("insufficient_evidence"),
        ),
    }
}

fn evaluate_org_permission_denied(
    run_timestamp: &str,
    direct_status: Option<SecretScanningStatus>,
) -> SecretScanningResult {
    match direct_status {
        Some(s) => build_result(
            s,
            run_timestamp,
            None,
            false,
            Some("alerts_permission_denied"),
        ),
        None => build_result(
            SecretScanningStatus::PermissionDenied,
            run_timestamp,
            None,
            false,
            Some("permission_denied"),
        ),
    }
}

fn evaluate_org_transient_error(
    run_timestamp: &str,
    direct_status: Option<SecretScanningStatus>,
) -> SecretScanningResult {
    match direct_status {
        Some(s) => build_result(
            s,
            run_timestamp,
            None,
            false,
            Some("alerts_transient_error"),
        ),
        None => build_result(
            SecretScanningStatus::Unknown,
            run_timestamp,
            None,
            false,
            Some("transient_error"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_age_buckets_has_all_labels() {
        let buckets = empty_age_buckets();
        for &(label, _, _) in config::SECRET_ALERT_AGE_BUCKETS {
            assert!(buckets.contains_key(label), "missing bucket: {label}");
        }
        assert!(buckets.contains_key(config::SECRET_ALERT_UNKNOWN_AGE_BUCKET));
        for v in buckets.values() {
            assert_eq!(*v, 0);
        }
    }

    #[test]
    fn age_bucket_recent_alert() {
        let now = parse_iso8601("2026-01-10T00:00:00Z").unwrap();
        assert_eq!(age_bucket("2026-01-05T00:00:00Z", now), Some("0_7_days"));
    }

    #[test]
    fn age_bucket_8_30_days() {
        let now = parse_iso8601("2026-01-30T00:00:00Z").unwrap();
        assert_eq!(age_bucket("2026-01-15T00:00:00Z", now), Some("8_30_days"));
    }

    #[test]
    fn age_bucket_31_90_days() {
        let now = parse_iso8601("2026-04-01T00:00:00Z").unwrap();
        assert_eq!(age_bucket("2026-02-15T00:00:00Z", now), Some("31_90_days"));
    }

    #[test]
    fn age_bucket_91_plus() {
        let now = parse_iso8601("2026-07-01T00:00:00Z").unwrap();
        assert_eq!(
            age_bucket("2026-01-01T00:00:00Z", now),
            Some("91_plus_days")
        );
    }

    #[test]
    fn age_bucket_unparseable_returns_none() {
        let now = Timestamp::now();
        assert_eq!(age_bucket("not-a-date", now), None);
    }

    #[test]
    fn age_bucket_future_dated_alert() {
        let now = "2026-04-01T00:00:00Z".parse::<Timestamp>().unwrap();
        assert_eq!(age_bucket("2026-04-10T00:00:00Z", now), Some("0_7_days"));
    }

    #[test]
    fn normalize_iso8601_canonical() {
        let result = normalize_iso8601("2026-06-15T14:30:45Z").unwrap();
        assert_eq!(result, "2026-06-15T14:30:45+00:00");
    }

    #[test]
    fn normalize_iso8601_with_offset() {
        let result = normalize_iso8601("2026-06-15T16:30:45+02:00").unwrap();
        assert_eq!(result, "2026-06-15T14:30:45+00:00");
    }

    #[test]
    fn normalize_iso8601_invalid() {
        assert!(normalize_iso8601("not-a-date").is_none());
    }

    #[test]
    fn scope_key_uses_id() {
        let repo = Repository {
            id: "12345".to_string(),
            node_id: Some("MDEwOlJlcG9zaXRvcnkxMjM0NQ==".to_string()),
            name: "test-repo".to_string(),
            visibility: crate::domain::repository::Visibility::Public,
            language: None,
            default_branch: "main".to_string(),
            archived: false,
            has_issues: true,
            inventory_key: "12345".to_string(),
            updated_at: None,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            is_empty: false,
            html_url: None,
            topics: vec![],
            license_spdx: None,
        };
        assert_eq!(scope_key(&repo), "12345");
    }

    #[test]
    fn track_timestamp_oldest() {
        let mut ts: Option<String> = None;
        track_timestamp(&mut ts, "2026-01-10T00:00:00+00:00", true);
        assert_eq!(ts.as_deref(), Some("2026-01-10T00:00:00+00:00"));
        track_timestamp(&mut ts, "2026-01-05T00:00:00+00:00", true);
        assert_eq!(ts.as_deref(), Some("2026-01-05T00:00:00+00:00"));
        track_timestamp(&mut ts, "2026-01-15T00:00:00+00:00", true);
        assert_eq!(ts.as_deref(), Some("2026-01-05T00:00:00+00:00"));
    }

    #[test]
    fn track_timestamp_newest() {
        let mut ts: Option<String> = None;
        track_timestamp(&mut ts, "2026-01-05T00:00:00+00:00", false);
        assert_eq!(ts.as_deref(), Some("2026-01-05T00:00:00+00:00"));
        track_timestamp(&mut ts, "2026-01-10T00:00:00+00:00", false);
        assert_eq!(ts.as_deref(), Some("2026-01-10T00:00:00+00:00"));
        track_timestamp(&mut ts, "2026-01-01T00:00:00+00:00", false);
        assert_eq!(ts.as_deref(), Some("2026-01-10T00:00:00+00:00"));
    }

    #[test]
    fn build_result_fields() {
        let result = build_result(
            SecretScanningStatus::Enabled,
            "2026-01-01T00:00:00+00:00",
            Some(true),
            true,
            None,
        );
        assert_eq!(result.status, SecretScanningStatus::Enabled);
        assert_eq!(result.has_open_alerts, Some(true));
        assert!(result.alerts_observable);
        assert!(result.reason.is_none());
    }

    #[test]
    fn build_result_with_reason() {
        let result = build_result(
            SecretScanningStatus::Unknown,
            "2026-01-01T00:00:00+00:00",
            None,
            false,
            Some("alerts_unavailable"),
        );
        assert_eq!(result.status, SecretScanningStatus::Unknown);
        assert_eq!(result.reason.as_deref(), Some("alerts_unavailable"));
    }

    #[test]
    fn evaluate_org_permission_denied_with_direct_status() {
        let result = evaluate_org_permission_denied(
            "2026-04-09T12:00:00+00:00",
            Some(SecretScanningStatus::Enabled),
        );
        assert_eq!(result.status, SecretScanningStatus::Enabled);
        assert_eq!(
            result.reason.as_deref(),
            Some("alerts_permission_denied"),
            "Some(s) arm should use 'alerts_permission_denied' reason"
        );
        assert!(!result.alerts_observable);
    }

    #[test]
    fn evaluate_org_permission_denied_without_direct_status() {
        let result = evaluate_org_permission_denied("2026-04-09T12:00:00+00:00", None);
        assert_eq!(result.status, SecretScanningStatus::PermissionDenied);
        assert_eq!(
            result.reason.as_deref(),
            Some("permission_denied"),
            "None arm should use 'permission_denied' reason"
        );
    }

    #[test]
    fn evaluate_org_transient_error_with_direct_status() {
        let result = evaluate_org_transient_error(
            "2026-04-09T12:00:00+00:00",
            Some(SecretScanningStatus::Disabled),
        );
        assert_eq!(result.status, SecretScanningStatus::Disabled);
        assert_eq!(
            result.reason.as_deref(),
            Some("alerts_transient_error"),
            "Some(s) arm should use 'alerts_transient_error' reason"
        );
        assert!(!result.alerts_observable);
    }

    #[test]
    fn evaluate_org_transient_error_without_direct_status() {
        let result = evaluate_org_transient_error("2026-04-09T12:00:00+00:00", None);
        assert_eq!(result.status, SecretScanningStatus::Unknown);
        assert_eq!(
            result.reason.as_deref(),
            Some("transient_error"),
            "None arm should use 'transient_error' reason"
        );
    }

    /// ghr-3ede27c2 H1-mirror: a truncated-but-technically-successful
    /// paginated fetch (`ApiOutcome::Success { truncated: true, .. }` —
    /// `is_ok()` is `true`, so the pre-fix `collect_org_alerts` guard
    /// `!result.is_ok()` alone would NOT degrade this) must build a
    /// degraded [`OrgAlertSummary`] via [`build_failure_summary`], never
    /// report [`CollectionStatus::Success`] over a partial alert list. The
    /// outcome carries a real alert item to prove this isn't merely an
    /// empty-response case caught by some other path.
    #[test]
    fn truncated_success_degrades_instead_of_reporting_complete() {
        let truncated = ApiOutcome::Success {
            status_code: 200,
            data: Some(serde_json::json!([{"repository": {"id": 1}}])),
            headers: None,
            truncated: true,
        };

        assert!(
            truncated.is_ok(),
            "precondition: a truncated success must still report is_ok() == true"
        );
        assert!(truncated.is_truncated());

        let summary = build_failure_summary(&truncated);

        assert_ne!(
            summary.collection_status,
            CollectionStatus::Success,
            "a truncated alert list must never surface as CollectionStatus::Success"
        );
        assert!(
            summary.collection_reason.is_some(),
            "a degraded summary must carry a collection_reason"
        );
        assert_eq!(
            summary.total_open_secret_alerts, 0,
            "a degraded summary must not report partial alert counts"
        );
    }

    fn sample_repo(id: &str, node_id: Option<&str>, name: &str) -> Arc<Repository> {
        Arc::new(Repository {
            id: id.to_string(),
            node_id: node_id.map(String::from),
            name: name.to_string(),
            visibility: crate::domain::repository::Visibility::Public,
            language: None,
            default_branch: "main".to_string(),
            archived: false,
            has_issues: true,
            inventory_key: id.to_string(),
            updated_at: None,
            pushed_at: None,
            created_at: None,
            description: None,
            fork: false,
            is_empty: false,
            html_url: None,
            topics: vec![],
            license_spdx: None,
        })
    }

    #[test]
    fn process_alert_matches_node_id_when_numeric_id_absent_in_inventory() {
        let repo = sample_repo("legacy-key", Some("R_kgDO_node_123"), "test-repo");

        let mut known_scope = HashMap::new();
        known_scope.insert(scope_key(&repo), &repo);
        if let Some(ref node_id) = repo.node_id {
            known_scope.insert(node_id.clone(), &repo);
        }

        let alert = serde_json::json!({
            "repository": {
                "id": 99999,
                "node_id": "R_kgDO_node_123"
            },
            "created_at": "2026-06-15T14:30:45Z"
        });

        let mut summary = OrgAlertSummary {
            collection_status: CollectionStatus::Success,
            collection_reason: None,
            per_repo: HashMap::new(),
            open_secret_alert_age_buckets: empty_age_buckets(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        };

        let now = Timestamp::now();
        process_alert(&alert, &known_scope, &mut summary, now);

        assert_eq!(summary.total_open_secret_alerts, 1);
        let repo_summary = summary.per_repo.get(&scope_key(&repo));
        assert!(repo_summary.is_some());
        assert_eq!(repo_summary.unwrap().open_alert_count, 1);
    }

    fn success_summary() -> OrgAlertSummary {
        OrgAlertSummary {
            collection_status: CollectionStatus::Success,
            collection_reason: None,
            per_repo: HashMap::new(),
            open_secret_alert_age_buckets: empty_age_buckets(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        }
    }

    #[test]
    fn process_alert_degrades_on_unidentifiable_repository_correlation() {
        let repo = sample_repo("11111", Some("R_node_AAA"), "repo-a");
        let mut known_scope = HashMap::new();
        known_scope.insert(scope_key(&repo), &repo);
        if let Some(ref node_id) = repo.node_id {
            known_scope.insert(node_id.clone(), &repo);
        }
        let now = Timestamp::now();

        for (label, alert) in [
            ("empty item", serde_json::json!({})),
            (
                "repository not an object",
                serde_json::json!({"repository": "octo/repo"}),
            ),
            (
                "repository without usable coordinates",
                serde_json::json!({"repository": {"full_name": "octo/repo"}}),
            ),
            (
                "repository with unusable coordinate types",
                serde_json::json!({"repository": {"id": null, "node_id": 7}}),
            ),
            (
                "empty numeric id",
                serde_json::json!({"repository": {"id": ""}}),
            ),
            (
                "empty node id",
                serde_json::json!({"repository": {"node_id": ""}}),
            ),
            (
                "both coordinates empty",
                serde_json::json!({"repository": {"id": "", "node_id": ""}}),
            ),
        ] {
            let mut summary = success_summary();
            process_alert(&alert, &known_scope, &mut summary, now);

            assert_eq!(
                summary.collection_status,
                CollectionStatus::Unavailable,
                "{label}: an uncorrelatable alert must degrade the summary"
            );
            assert_eq!(
                summary.collection_reason.as_deref(),
                Some("alert_correlation_unidentifiable"),
                "{label}: degraded reason must name the correlation gap"
            );
            assert_eq!(summary.total_open_secret_alerts, 0, "{label}");
            assert!(summary.per_repo.is_empty(), "{label}");

            let eval = evaluate_with_org_summary(
                &repo,
                "2026-06-15T15:00:00Z",
                &summary,
                Some(SecretScanningStatus::Enabled),
            );
            assert_eq!(
                eval.has_open_alerts, None,
                "{label}: degraded summary must not claim alert-free"
            );
            assert!(!eval.alerts_observable, "{label}");
        }
    }

    #[test]
    fn process_alert_correlates_when_one_coordinate_is_empty_but_the_other_is_usable() {
        let repo = sample_repo("11111", Some("R_node_AAA"), "repo-a");
        let mut known_scope = HashMap::new();
        known_scope.insert(scope_key(&repo), &repo);
        if let Some(ref node_id) = repo.node_id {
            known_scope.insert(node_id.clone(), &repo);
        }
        let now = Timestamp::now();

        for (label, alert) in [
            (
                "empty numeric id, usable node id",
                serde_json::json!({"repository": {"id": "", "node_id": "R_node_AAA"}}),
            ),
            (
                "usable numeric id, empty node id",
                serde_json::json!({"repository": {"id": 11111, "node_id": ""}}),
            ),
            (
                "usable string numeric id, empty node id",
                serde_json::json!({"repository": {"id": "11111", "node_id": ""}}),
            ),
        ] {
            let mut summary = success_summary();
            process_alert(&alert, &known_scope, &mut summary, now);

            assert_eq!(
                summary.collection_status,
                CollectionStatus::Success,
                "{label}: a usable alternate coordinate must still correlate"
            );
            assert_eq!(summary.collection_reason, None, "{label}");
            assert_eq!(summary.total_open_secret_alerts, 1, "{label}");
            assert_eq!(
                summary
                    .per_repo
                    .get(&scope_key(&repo))
                    .map(|r| r.open_alert_count),
                Some(1),
                "{label}"
            );
        }
    }

    #[test]
    fn process_alert_skips_valid_out_of_scope_repository_without_degrading() {
        let repo = sample_repo("11111", Some("R_node_AAA"), "repo-a");
        let mut known_scope = HashMap::new();
        known_scope.insert(scope_key(&repo), &repo);
        if let Some(ref node_id) = repo.node_id {
            known_scope.insert(node_id.clone(), &repo);
        }
        let now = Timestamp::now();

        for (label, alert) in [
            (
                "numeric identity out of scope",
                serde_json::json!({"repository": {"id": 99999}}),
            ),
            (
                "node identity out of scope",
                serde_json::json!({"repository": {"node_id": "R_node_ZZZ"}}),
            ),
            (
                "both identities out of scope",
                serde_json::json!({"repository": {"id": 99999, "node_id": "R_node_ZZZ"}}),
            ),
            (
                "empty numeric id with usable out-of-scope node id",
                serde_json::json!({"repository": {"id": "", "node_id": "R_node_ZZZ"}}),
            ),
            (
                "usable out-of-scope numeric id with empty node id",
                serde_json::json!({"repository": {"id": 99999, "node_id": ""}}),
            ),
        ] {
            let mut summary = success_summary();
            process_alert(&alert, &known_scope, &mut summary, now);

            assert_eq!(
                summary.collection_status,
                CollectionStatus::Success,
                "{label}: a valid out-of-scope identity must not degrade"
            );
            assert_eq!(summary.collection_reason, None, "{label}");
            assert_eq!(summary.total_open_secret_alerts, 0, "{label}");
            assert!(summary.per_repo.is_empty(), "{label}");
        }
    }

    #[test]
    fn process_alert_rejects_conflicting_numeric_and_node_id_correlation() {
        let repo_a = sample_repo("11111", Some("R_node_AAA"), "repo-a");
        let repo_b = sample_repo("22222", Some("R_node_BBB"), "repo-b");

        let mut known_scope = HashMap::new();
        known_scope.insert(scope_key(&repo_a), &repo_a);
        if let Some(ref node_id) = repo_a.node_id {
            known_scope.insert(node_id.clone(), &repo_a);
        }
        known_scope.insert(scope_key(&repo_b), &repo_b);
        if let Some(ref node_id) = repo_b.node_id {
            known_scope.insert(node_id.clone(), &repo_b);
        }

        let alert = serde_json::json!({
            "repository": {
                "id": 11111,
                "node_id": "R_node_BBB"
            },
            "created_at": "2026-06-15T14:30:45Z"
        });

        let mut summary = OrgAlertSummary {
            collection_status: CollectionStatus::Success,
            collection_reason: None,
            per_repo: HashMap::new(),
            open_secret_alert_age_buckets: empty_age_buckets(),
            total_open_secret_alerts: 0,
            oldest_open_secret_alert_created_at: None,
            newest_open_secret_alert_created_at: None,
        };

        let now = Timestamp::now();
        process_alert(&alert, &known_scope, &mut summary, now);

        assert_eq!(summary.total_open_secret_alerts, 0);
        assert!(summary.per_repo.is_empty());
        assert_eq!(summary.collection_status, CollectionStatus::Unavailable);
        assert_eq!(
            summary.collection_reason.as_deref(),
            Some("alert_coordinate_conflict")
        );

        let eval_a = evaluate_with_org_summary(
            &repo_a,
            "2026-06-15T15:00:00Z",
            &summary,
            Some(SecretScanningStatus::Enabled),
        );
        assert_eq!(eval_a.has_open_alerts, None);
        assert!(!eval_a.alerts_observable);
        assert_eq!(eval_a.reason.as_deref(), Some("alert_coordinate_conflict"));

        let eval_b = evaluate_with_org_summary(
            &repo_b,
            "2026-06-15T15:00:00Z",
            &summary,
            Some(SecretScanningStatus::Enabled),
        );
        assert_eq!(eval_b.has_open_alerts, None);
        assert!(!eval_b.alerts_observable);
        assert_eq!(eval_b.reason.as_deref(), Some("alert_coordinate_conflict"));
    }

    fn test_client(base_url: &str) -> GitHubClient {
        let credential = crate::github::auth::GitHubCredential {
            mode: crate::domain::auth::AuthMode::Pat,
            token: secrecy::SecretString::from("test-token"),
            expires_at: None,
        };
        let budget = Arc::new(crate::github::budget::BudgetGate::new(
            config::API_BUDGET_LIMIT,
            std::time::Duration::from_secs(config::API_BUDGET_WAIT_SECS),
        ));
        let rate_limit = Arc::new(crate::github::rate_limit::new_default());
        GitHubClient::new(credential, base_url, "test-org", None, budget, rate_limit)
            .expect("test client construction should succeed")
    }

    #[tokio::test]
    async fn collect_org_alerts_correlates_by_node_id_with_legacy_inventory_id() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = test_client(&server.uri());

        let alerts_payload = serde_json::json!([
            {
                "repository": {
                    "id": 99999,
                    "node_id": "R_kgDO_node_123"
                },
                "created_at": "2026-06-15T14:30:45Z"
            },
            {
                "repository": {
                    "id": 54321,
                    "node_id": "R_kgDO_other_node"
                },
                "created_at": "2026-06-15T14:30:45Z"
            },
            {
                "repository": {
                    "id": "77777",
                    "node_id": "R_kgDO_string_node"
                },
                "created_at": "2026-06-15T14:30:45Z"
            },
            {
                "repository": {
                    "id": 33333,
                    "node_id": "R_kgDO_dual_match"
                },
                "created_at": "2026-06-15T14:30:45Z"
            },
            {
                "repository": {
                    "id": 88888,
                    "node_id": "R_kgDO_unmatched"
                },
                "created_at": "2026-06-15T14:30:45Z"
            }
        ]);

        Mock::given(path("/orgs/test-org/secret-scanning/alerts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(alerts_payload))
            .mount(&server)
            .await;

        let repo_legacy = sample_repo("legacy-key", Some("R_kgDO_node_123"), "legacy-repo");
        let repo_numeric = sample_repo("54321", Some("R_kgDO_numeric_node"), "numeric-repo");
        let repo_string = sample_repo("77777", Some("R_kgDO_some_node"), "string-repo");
        let repo_dual = sample_repo("33333", Some("R_kgDO_dual_match"), "dual-repo");

        let repos = [repo_legacy, repo_numeric, repo_string, repo_dual];
        let summary = collect_org_alerts(&client, &repos, "2026-06-15T15:00:00Z").await;

        assert_eq!(summary.collection_status, CollectionStatus::Success);
        assert_eq!(summary.total_open_secret_alerts, 4);
        assert_eq!(summary.per_repo.len(), 4);
        assert_eq!(
            summary.per_repo.get("33333").map(|s| s.open_alert_count),
            Some(1)
        );
        assert_eq!(
            summary
                .per_repo
                .get("legacy-key")
                .map(|s| s.open_alert_count),
            Some(1)
        );
        assert_eq!(
            summary.per_repo.get("54321").map(|s| s.open_alert_count),
            Some(1)
        );
        assert_eq!(
            summary.per_repo.get("77777").map(|s| s.open_alert_count),
            Some(1)
        );
        assert!(!summary.per_repo.contains_key("R_kgDO_node_123"));
        assert!(!summary.per_repo.contains_key("99999"));
        assert!(!summary.per_repo.contains_key("88888"));
    }

    #[tokio::test]
    async fn collect_org_alerts_degrades_on_duplicate_node_id_alias_collision() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let client = test_client(&server.uri());

        Mock::given(path("/orgs/test-org/secret-scanning/alerts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let repo_1 = sample_repo("111", Some("R_shared_node"), "repo-1");
        let repo_2 = sample_repo("222", Some("R_shared_node"), "repo-2");

        let repos = [repo_1, repo_2];
        let summary = collect_org_alerts(&client, &repos, "2026-06-15T15:00:00Z").await;

        assert_eq!(summary.collection_status, CollectionStatus::Unavailable);
        assert_eq!(
            summary.collection_reason.as_deref(),
            Some("repository_alias_collision")
        );
    }
}
