//! Report view model: shapes data for template rendering.
//!
//! Keeps the template layer thin by pre-computing all display values.
//! The view model is a flat, template-friendly representation of the
//! [`Evidence`] domain object — no nested traversal or formatting
//! logic belongs in the template.

use std::collections::{HashMap, HashSet};

use crate::config;
use crate::config::dashboard::CoverageTiers;
use crate::domain::evidence::Evidence;
use crate::domain::metrics::OwnerType;
use crate::domain::time::is_repo_stale;

/// Coverage tier classification for dashboard display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageTier {
    /// Rate ≥ `pass_threshold`.
    Pass,
    /// Rate ≥ `warn_threshold` and < `pass_threshold`.
    Warn,
    /// Rate < `warn_threshold`.
    Fail,
    /// Rate not available (e.g., denominator is zero).
    Na,
}

impl CoverageTier {
    /// CSS class for styling this tier.
    #[must_use]
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Pass => "tier-pass",
            Self::Warn => "tier-warn",
            Self::Fail => "tier-fail",
            Self::Na => "tier-na",
        }
    }

    /// CSS class for status dot rendering in owner tables.
    ///
    /// Maps tier to the appropriate status-dot class name.
    #[must_use]
    pub fn status_dot_class(&self) -> &'static str {
        match self {
            Self::Pass => "status-pass",
            Self::Warn => "status-warn",
            Self::Fail => "status-fail",
            Self::Na => "status-unknown",
        }
    }

    /// Classify a rate percentage into a tier using the given thresholds.
    ///
    /// When `pass_threshold == warn_threshold`, there is no warn band —
    /// rates are either pass or fail.
    #[must_use]
    pub fn from_rate(rate: Option<f64>, tiers: &CoverageTiers) -> Self {
        match rate {
            None => Self::Na,
            Some(r) if r >= tiers.pass_threshold => Self::Pass,
            Some(r) if r >= tiers.warn_threshold => Self::Warn,
            Some(_) => Self::Fail,
        }
    }
}

/// A row in the owners overview table.
#[derive(Debug, Clone)]
pub struct OwnerOverviewRow {
    /// Display name of the owner (e.g., `@org/team-name`).
    pub owner: String,
    /// Short display name with org prefix stripped (e.g., `team-name`).
    pub owner_short: String,
    /// URL-safe slug for the detail page filename (e.g., `org-team-name`).
    pub slug: String,
    /// Whether this owner is a team or user.
    pub owner_type: OwnerType,
    /// Number of repositories this owner is responsible for.
    pub repo_count: u32,
    /// Per-control coverage cells.
    pub controls: Vec<ControlCell>,
    /// Composite security score (geometric mean of 6 control rates, 0.1% floor):
    /// `security_policy`, `secret_scanning`, `dependabot_security_updates`,
    /// `branch_protection`, `non_stale`, `alert_free`.
    /// `None` when all control rates are N/A.
    pub sec_score: Option<f64>,
    /// Formatted sec score string (e.g., `"72.3%"` or `"N/A"`).
    pub sec_score_formatted: String,
    /// Coverage tier for the sec score.
    pub sec_score_tier: CoverageTier,
    /// CSS width class for the sec score progress bar.
    pub sec_score_width_class: &'static str,
}

/// A per-control coverage cell in the owner overview table.
#[derive(Debug, Clone)]
pub struct ControlCell {
    /// Formatted rate string (e.g., "80.0% (4/5)").
    pub rate_formatted: String,
    /// Coverage tier for styling.
    pub tier: CoverageTier,
    /// CSS width class for progress bar rendering (e.g., `"w-80"`).
    pub width_class: &'static str,
}

/// A row in the per-owner detail table (one repo).
#[derive(Debug, Clone)]
pub struct OwnerRepoRow {
    /// Repository name.
    pub repo_name: String,
    /// URL to the repository on GitHub (e.g., `"https://github.com/org/repo"`).
    pub repo_url: String,
    /// Repository visibility label (`"Public"`, `"Internal"`, or `"Private"`).
    pub visibility: String,
    /// Per-control pass/fail status dots.
    pub controls: Vec<StatusDot>,

    /// Short description of the repository, or `"—"` if none.
    pub description: String,
    /// Primary language, or `"—"` if none.
    pub language: String,
    /// Whether this repository is a fork.
    pub is_fork: bool,
    /// SPDX license identifier, or `"—"` if none.
    pub license: String,
    /// ISO 8601 date of last git push, formatted as `YYYY-MM-DD`, or `"—"`.
    pub pushed_at: String,
    /// ISO 8601 date of repository creation, formatted as `YYYY-MM-DD`, or `"—"`.
    pub created_at: String,
    /// GitHub login of the last committer, or `"—"`.
    pub last_committer_login: String,
    /// URL to the last committer's GitHub profile, or empty string if unknown.
    pub last_committer_url: String,
    /// Date of the last commit, formatted as `YYYY-MM-DD`, or `"—"`.
    pub last_commit_date: String,
    /// Whether this repository is stale (`updated_at` > 2 years before report date).
    pub is_stale: bool,
    /// Per-repo score: `passing_controls` / (passing + failing) * 100, excluding unknowns.
    /// `None` when no controls have a deterministic (pass/fail) result.
    pub repo_score: Option<f64>,
    /// Formatted repo score string (e.g., `"75.0%"` or `"N/A"`).
    pub repo_score_formatted: String,
    /// Coverage tier for the repo score.
    pub repo_score_tier: CoverageTier,
    /// CSS width class for the repo score progress bar.
    pub repo_score_width_class: &'static str,
}

/// A status indicator dot for a single control on a single repo.
///
/// All CSS classes and labels are compile-time constants, so fields use
/// `&'static str` to avoid unnecessary heap allocations.
#[derive(Debug, Clone)]
pub struct StatusDot {
    /// CSS class (e.g., `"status-pass"`, `"status-fail"`, `"status-unknown"`).
    pub css_class: &'static str,
    /// Accessible label/tooltip text (e.g., `"pass"`, `"fail"`).
    pub label: &'static str,
}

/// A summary scorecard card combining a control label with its coverage cell.
///
/// Pre-zipped for template iteration (Askama does not support array indexing).
#[derive(Debug, Clone)]
pub struct SummaryCard {
    /// Human-readable control name (e.g., `"Security Policy"`).
    pub label: String,
    /// Coverage rate and tier for styling.
    pub cell: ControlCell,
}

/// View model for a single owner's detail page.
#[derive(Debug, Clone)]
pub struct OwnerDetailViewModel {
    /// Display name of the owner.
    pub owner: String,
    /// Short display name with org prefix stripped.
    pub owner_short: String,
    /// Owner type as a display string (`"Team"` or `"User"`).
    pub owner_type_label: String,
    /// Breadcrumb label for navigation.
    pub breadcrumb_label: String,
    /// Per-repo rows with status dots.
    pub repo_rows: Vec<OwnerRepoRow>,
    /// Ordered control names for table headers (e.g., `["Security Policy", …]`).
    pub control_names: Vec<String>,
    /// Summary scorecard cards (one per control, same order as `control_names`).
    pub summary_cards: Vec<SummaryCard>,
    /// Whether any repo row is flagged as stale (drives footnote rendering).
    pub has_stale_repos: bool,
    /// Number of stale repos for this owner (`updated_at` > 2 years before report date).
    pub stale_repo_count: u32,
    /// Total number of repos for this owner (for the card denominator).
    pub total_repo_count: u32,
    /// CSS width class for the stale repos progress bar.
    ///
    /// Represents the proportion of stale repos relative to total repos
    /// for this owner — distinct from the org-level stale rate on the
    /// dashboard which measures archival coverage.
    pub stale_width_class: &'static str,
}

/// View model for the owners overview page.
#[derive(Debug, Clone)]
pub struct OwnersViewModel {
    /// One row per owner, sorted.
    pub rows: Vec<OwnerOverviewRow>,
    /// Ordered control names for table headers.
    pub control_names: Vec<String>,
}

/// A top-scoring security team for display in the CODEOWNERS Summary box.
#[derive(Debug, Clone)]
pub struct TopSecurityTeam {
    /// Display name of the owner (e.g., `@org/security-team`).
    pub owner: String,
    /// Short display name with org prefix stripped (e.g., `security-team`).
    pub owner_short: String,
    /// Formatted sec score string (e.g., `"92.3%"`).
    pub sec_score_formatted: String,
    /// URL-safe slug for linking to the owner detail page.
    pub slug: String,
    /// CSS class for podium rank styling (`"rank-gold"`, `"rank-silver"`, `"rank-bronze"`).
    pub rank_class: &'static str,
    /// Trophy emoji for podium position (`"🥇"`, `"🥈"`, `"🥉"`).
    pub rank_emoji: &'static str,
    /// Coverage tier for the sec score (drives progress bar fill colour).
    pub sec_score_tier: CoverageTier,
    /// CSS width class for the sec score progress bar.
    pub sec_score_width_class: &'static str,
}

/// A row in the orphaned repositories table.
///
/// Repos are "orphaned" when they have no identifiable code owners:
/// CODEOWNERS file is absent, or the file contains no `@`-prefixed owners.
#[derive(Debug, Clone)]
pub struct OrphanedRepoRow {
    /// Repository name.
    pub repo_name: String,
    /// URL to the repository on GitHub.
    pub repo_url: String,
    /// Repository visibility label (`"Public"`, `"Internal"`, or `"Private"`).
    pub visibility: String,
    /// Short description, or `"—"` if none.
    pub description: String,
    /// Primary language, or `"—"` if none.
    pub language: String,
    /// GitHub login of the last committer, or `"—"`.
    pub last_committer_login: String,
    /// URL to the last committer's GitHub profile, or empty string.
    pub last_committer_url: String,
    /// Date of the last commit, formatted as `YYYY-MM-DD`, or `"—"`.
    pub last_commit_date: String,
    /// Whether this repository is stale (`updated_at` > 2 years before report date).
    pub is_stale: bool,
}

/// View model for the orphaned repositories page.
#[derive(Debug, Clone)]
pub struct OrphanedViewModel {
    /// One row per orphaned repo, sorted by last committer then name.
    pub rows: Vec<OrphanedRepoRow>,
    /// Organization name (for the page title).
    pub organization: String,
    /// Total count of orphaned repos.
    pub orphaned_count: u32,
    /// Whether any repo row is flagged as stale (drives footnote rendering).
    pub has_stale_repos: bool,
}

/// Generate a URL-safe slug from an owner name.
///
/// Rules:
/// 1. Strip leading `@`.
/// 2. Replace any character NOT in `[a-zA-Z0-9_-]` with `-`.
/// 3. Collapse consecutive dashes.
/// 4. Trim leading/trailing dashes.
///
/// Returns an empty string if nothing remains after sanitization
/// (the caller must handle the fallback).
///
/// # Examples
///
/// ```
/// use gh_report::report::view_model::generate_slug;
///
/// assert_eq!(generate_slug("@org/security-team"), "org-security-team");
/// ```
#[must_use]
pub fn generate_slug(owner: &str) -> String {
    let stripped = owner.strip_prefix('@').unwrap_or(owner);

    let mut slug = String::with_capacity(stripped.len());
    let mut prev_dash = true;

    for c in stripped.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            slug.push(c);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }

    if slug.ends_with('-') {
        slug.pop();
    }

    slug
}

/// Generate unique slugs for a list of owner names.
///
/// Handles collisions by appending a numeric suffix (e.g., `org-team-2`).
/// If a slug is empty after sanitization, generates a fallback `owner-N`.
///
/// Returns a map from canonical owner name → unique slug.
#[must_use]
pub fn generate_unique_slugs(owners: &[String]) -> HashMap<String, String> {
    let mut result: HashMap<String, String> = HashMap::new();
    let mut used_slugs: HashSet<String> = HashSet::new();
    let mut fallback_counter: usize = 0;

    for owner in owners {
        let base = generate_slug(owner);
        let base = if base.is_empty() {
            fallback_counter += 1;
            format!("owner-{fallback_counter}")
        } else {
            base
        };

        let final_slug = if used_slugs.contains(&base) {
            let mut suffix = 2;
            loop {
                let candidate = format!("{base}-{suffix}");
                if !used_slugs.contains(&candidate) {
                    break candidate;
                }
                suffix += 1;
            }
        } else {
            base
        };

        used_slugs.insert(final_slug.clone());
        result.insert(owner.clone(), final_slug);
    }

    result
}

/// Strip the org prefix from an owner display name for terse link text.
///
/// Splits on the first `/` and returns everything after it. When there
/// is no `/` the original string is returned unchanged.
///
/// # Examples
///
/// ```
/// use gh_report::report::view_model::strip_org_prefix;
///
/// assert_eq!(strip_org_prefix("@org/security-team"), "security-team");
/// assert_eq!(strip_org_prefix("@username"), "@username");
/// ```
#[must_use]
pub fn strip_org_prefix(owner: &str) -> String {
    match owner.split_once('/') {
        Some((_, after)) => after.to_string(),
        None => owner.to_string(),
    }
}

/// Pre-computed display values for the HTML report template.
///
/// All formatting is done here so the Askama template only interpolates
/// ready-to-display strings and numbers.
#[derive(Debug, Clone)]
pub struct ReportViewModel {
    /// Organization name.
    pub organization: String,
    /// Report date (YYYY-MM-DD).
    pub date: String,
    /// Report date and time (YYYY-MM-DD HH:MM UTC).
    pub date_time: String,
    /// Total non-archived repositories assessed.
    pub total_repos: u32,
    /// Total repositories, including archived repositories.
    pub total_all_repos: u32,

    pub policy_coverage_formatted: String,
    pub dependabot_coverage_formatted: String,
    pub secret_scanning_coverage_formatted: String,
    pub branch_protection_coverage_formatted: String,
    pub codeowners_coverage_formatted: String,

    pub policy_via_setting: u32,
    pub policy_via_file: u32,
    pub policy_missing: u32,
    pub policy_unknown: u32,

    pub dependabot_observable_repos: u32,
    pub dependabot_paused: u32,
    pub dependabot_disabled: u32,
    pub dependabot_unknown: u32,

    pub secret_scanning_observable_repos: u32,
    pub secret_scanning_disabled: u32,
    pub secret_scanning_permission_denied: u32,
    pub secret_scanning_unknown: u32,

    pub branch_protection_partial: u32,
    pub branch_protection_fail: u32,
    pub branch_protection_unknown: u32,

    pub codeowners_conforming: u32,
    pub codeowners_non_conforming: u32,
    pub codeowners_absent: u32,
    pub codeowners_unknown: u32,
    /// Repos where the CODEOWNERS file was found but content parsing was
    /// skipped (encoding mismatch, oversized payload, decode failure, invalid
    /// UTF-8). Orthogonal to status counts; surfaced so operators can detect
    /// silent data loss in the per-repo evidence.
    pub codeowners_truncated: u32,

    pub public_repos: u32,
    pub internal_repos: u32,
    pub private_repos: u32,
    pub rate_limit_warnings: u32,

    pub conforming_codeowners_path: &'static str,
    pub non_conforming_codeowners_path: &'static str,

    pub policy_tier: CoverageTier,
    pub secret_scanning_tier: CoverageTier,
    pub dependabot_tier: CoverageTier,
    pub branch_protection_tier: CoverageTier,
    pub codeowners_tier: CoverageTier,

    pub policy_width_class: &'static str,
    pub secret_scanning_width_class: &'static str,
    pub dependabot_width_class: &'static str,
    pub branch_protection_width_class: &'static str,
    pub codeowners_width_class: &'static str,

    /// Composite health score (geometric mean of available coverage rates).
    /// `None` when all 6 control rates are N/A (`security_policy`,
    /// `secret_scanning`, `dependabot_security_updates`, `branch_protection`,
    /// `codeowners`, `archival_coverage`).
    pub health_score: Option<f64>,
    /// Coverage tier for the health score.
    pub health_tier: CoverageTier,
    /// Formatted health score (e.g., `"72.3%"` or `"N/A"`).
    pub health_score_formatted: String,
    /// CSS width class for the health score progress bar.
    pub health_width_class: &'static str,

    /// Archival coverage rate: proportion of stale-lifecycle repos
    /// (stale active + archived) that have been archived.
    /// Formula: `archived / (archived + stale_active) × 100`.
    /// `None` when `archived_repos == 0` and no active repos are stale.
    pub stale_rate: Option<f64>,
    /// Formatted stale rate string (e.g., `"12.5%"` or `"N/A"`).
    pub stale_rate_formatted: String,
    /// Coverage tier for the stale rate.
    pub stale_tier: CoverageTier,
    /// CSS width class for the stale rate progress bar.
    pub stale_width_class: &'static str,
    /// Number of archived repositories.
    pub archived_repos: u32,
    /// Number of active (non-archived) repos that are stale.
    pub stale_active_repos: u32,

    /// Owners overview data. `None` when no CODEOWNERS parsed data is available.
    pub owners: Option<OwnersViewModel>,

    /// Top 3 security teams by sec score, for the CODEOWNERS Summary box.
    /// Empty when no owner metrics are available.
    pub top_security_teams: Vec<TopSecurityTeam>,

    /// Count of orphaned repositories (drives nav link label).
    pub orphaned_count: u32,

    /// Whether this report was rendered from a cached baseline (warm-start)
    /// rather than a fresh API collection.
    pub warm_start: bool,
}

impl ReportViewModel {
    /// Build a report view model from collected evidence.
    ///
    /// All formatting is done eagerly so the template layer stays thin.
    /// Coverage tiers are computed from the provided thresholds.
    #[must_use]
    pub fn from_evidence(evidence: &Evidence, tiers: &CoverageTiers) -> Self {
        let metadata = &evidence.assessment_metadata;
        let stats = &evidence.collection_statistics;
        let m = &evidence.metrics;

        let dependabot_observable = extra_u32(
            &m.dependabot_security_updates_coverage.extra,
            "observable_repositories",
        );
        let secret_scanning_observable =
            extra_u32(&m.secret_scanning_coverage.extra, "observable_repositories");

        let (
            archived,
            stale_active_repos,
            stale_rate,
            stale_tier,
            stale_rate_formatted,
            stale_width_class,
        ) = compute_archival_coverage(evidence, tiers);

        let health_score = compute_health_score(&[
            m.security_policy_coverage.rate,
            m.secret_scanning_coverage.rate,
            m.dependabot_security_updates_coverage.rate,
            m.branch_protection_coverage.rate,
            m.codeowners_coverage.rate,
            stale_rate,
        ]);

        Self {
            organization: metadata.organization.clone(),
            date: metadata.date.clone(),
            date_time: format_run_timestamp(&metadata.run_timestamp),
            total_repos: stats.total_repos,
            total_all_repos: stats.total_repos.saturating_add(archived),
            policy_coverage_formatted: m.security_policy_coverage.to_string(),
            dependabot_coverage_formatted: m.dependabot_security_updates_coverage.to_string(),
            secret_scanning_coverage_formatted: m.secret_scanning_coverage.to_string(),
            branch_protection_coverage_formatted: m.branch_protection_coverage.to_string(),
            codeowners_coverage_formatted: m.codeowners_coverage.to_string(),
            policy_via_setting: m.policy_counts.via_setting,
            policy_via_file: m.policy_counts.via_file,
            policy_missing: m.policy_counts.missing,
            policy_unknown: m.policy_counts.unknown,
            dependabot_observable_repos: dependabot_observable,
            dependabot_paused: m.dependabot_security_updates_counts.paused,
            dependabot_disabled: m.dependabot_security_updates_counts.disabled,
            dependabot_unknown: m.dependabot_security_updates_counts.unknown,
            secret_scanning_observable_repos: secret_scanning_observable,
            secret_scanning_disabled: m.secret_scanning_counts.disabled,
            secret_scanning_permission_denied: m.secret_scanning_counts.permission_denied,
            secret_scanning_unknown: m.secret_scanning_counts.unknown,
            branch_protection_partial: m.branch_protection_counts.partial,
            branch_protection_fail: m.branch_protection_counts.fail,
            branch_protection_unknown: m.branch_protection_counts.unknown,
            codeowners_conforming: m.codeowners_counts.conforming,
            codeowners_non_conforming: m.codeowners_counts.non_conforming,
            codeowners_absent: m.codeowners_counts.absent,
            codeowners_unknown: m.codeowners_counts.unknown,
            codeowners_truncated: m.codeowners_counts.truncated,
            public_repos: stats.public_repos,
            internal_repos: stats.internal_repos,
            private_repos: stats.private_repos,
            rate_limit_warnings: metadata.rate_limit_warnings,
            conforming_codeowners_path: config::CONFORMING_CODEOWNERS_PATH,
            non_conforming_codeowners_path: config::NON_CONFORMING_CODEOWNERS_PATH,
            policy_tier: CoverageTier::from_rate(m.security_policy_coverage.rate, tiers),
            secret_scanning_tier: CoverageTier::from_rate(m.secret_scanning_coverage.rate, tiers),
            dependabot_tier: CoverageTier::from_rate(
                m.dependabot_security_updates_coverage.rate,
                tiers,
            ),
            branch_protection_tier: CoverageTier::from_rate(
                m.branch_protection_coverage.rate,
                tiers,
            ),
            codeowners_tier: CoverageTier::from_rate(m.codeowners_coverage.rate, tiers),
            policy_width_class: rate_to_width_class(m.security_policy_coverage.rate),
            secret_scanning_width_class: rate_to_width_class(m.secret_scanning_coverage.rate),
            dependabot_width_class: rate_to_width_class(
                m.dependabot_security_updates_coverage.rate,
            ),
            branch_protection_width_class: rate_to_width_class(m.branch_protection_coverage.rate),
            codeowners_width_class: rate_to_width_class(m.codeowners_coverage.rate),
            health_score,
            health_tier: CoverageTier::from_rate(health_score, tiers),
            health_score_formatted: health_score
                .map_or_else(|| "N/A".to_string(), |s| format!("{s:.1}%")),
            health_width_class: rate_to_width_class(health_score),
            stale_rate,
            stale_rate_formatted,
            stale_tier,
            stale_width_class,
            archived_repos: archived,
            stale_active_repos,
            owners: None,
            top_security_teams: Vec::new(),
            orphaned_count: 0,
            warm_start: metadata.warm_start,
        }
    }
}

/// CSS class names for each 5% increment: `"w-0"` through `"w-100"`.
const WIDTH_CLASSES: [&str; 21] = [
    "w-0", "w-5", "w-10", "w-15", "w-20", "w-25", "w-30", "w-35", "w-40", "w-45", "w-50", "w-55",
    "w-60", "w-65", "w-70", "w-75", "w-80", "w-85", "w-90", "w-95", "w-100",
];

/// Compute the Organisation Governance Score as the geometric mean of available
/// coverage rates.
///
/// Combines security control rates (Security Policy, Secret Scanning,
/// Dependabot, Branch Protection, CODEOWNERS, and optionally Archival Coverage).
/// Controls with `None` rates (N/A) are excluded from the computation —
/// they are not treated as zero.
///
/// Input rates are clamped to `[0.0, 100.0]`: negative values are treated
/// as 0%, and values above 100% (possible when `numerator > denominator`)
/// are capped at 100%.
///
/// A 0.0% rate is floored to 0.1% so that a single zero-rate control does
/// not collapse the entire geometric mean to zero.
///
/// Returns:
/// - `None` if all rates are `None`.
/// - Otherwise, the geometric mean rounded to 1 decimal place.
pub(crate) fn compute_health_score(rates: &[Option<f64>]) -> Option<f64> {
    let available: Vec<f64> = rates
        .iter()
        .filter_map(|r| *r)
        .map(|r| r.clamp(0.0, 100.0))
        .map(|r| if r == 0.0 { 0.1 } else { r })
        .collect();

    if available.is_empty() {
        return None;
    }

    let n = f64::from(u32::try_from(available.len()).unwrap_or(u32::MAX));
    let log_sum: f64 = available.iter().map(|r| r.ln()).sum();
    let geo_mean = (log_sum / n).exp();

    Some((geo_mean * 10.0).round() / 10.0)
}

/// Map an optional rate percentage to a CSS width class.
///
/// Rounds to the nearest 5% increment: `.w-0` through `.w-100`.
/// Returns `"w-0"` if the rate is `None`.
#[must_use]
pub(crate) fn rate_to_width_class(rate: Option<f64>) -> &'static str {
    match rate {
        None => "w-0",
        Some(r) => {
            let clamped = r.clamp(0.0, 100.0);
            let bucket = (clamped / 5.0).round();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "bucket ∈ [0.0, 20.0] post-clamp; no stable safe f64→usize conversion exists"
            )]
            let index = (bucket as usize).min(WIDTH_CLASSES.len() - 1);
            WIDTH_CLASSES[index]
        }
    }
}

/// Compute archival coverage metrics from evidence.
///
/// Returns `(archived, stale_active_repos, stale_rate, stale_tier,
/// stale_rate_formatted, stale_width_class)`.
fn compute_archival_coverage(
    evidence: &Evidence,
    tiers: &CoverageTiers,
) -> (u32, u32, Option<f64>, CoverageTier, String, &'static str) {
    let stats = &evidence.collection_statistics;
    let metadata = &evidence.assessment_metadata;

    let archived = stats.archived_repos;
    let stale_active = evidence
        .repositories
        .iter()
        .filter(|r| is_repo_stale(r.repository.updated_at.as_deref(), &metadata.run_timestamp))
        .count();
    let stale_active_repos = u32::try_from(stale_active).unwrap_or(u32::MAX);
    let stale_denominator = archived.saturating_add(stale_active_repos);
    let stale_rate = if stale_denominator > 0 {
        Some((f64::from(archived) / f64::from(stale_denominator)) * 100.0)
    } else {
        None
    };
    let stale_tier = CoverageTier::from_rate(stale_rate, tiers);
    let stale_rate_formatted = stale_rate.map_or_else(|| "N/A".to_string(), |s| format!("{s:.1}%"));
    let stale_width_class = rate_to_width_class(stale_rate);

    (
        archived,
        stale_active_repos,
        stale_rate,
        stale_tier,
        stale_rate_formatted,
        stale_width_class,
    )
}

/// Extract a `u32` value from a `RateMetric`'s extra map.
///
/// Returns `0` if the key is absent or cannot be converted.
fn extra_u32(extra: &std::collections::HashMap<String, serde_json::Value>, key: &str) -> u32 {
    extra
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

/// Format an ISO 8601 `run_timestamp` into `YYYY-MM-DD HH:MM UTC`,
/// converting to UTC so the displayed time is always correct.
///
/// Falls back to the raw string if parsing fails.
fn format_run_timestamp(ts: &str) -> String {
    if let Some(dt) = crate::domain::time::parse_iso8601(ts) {
        return dt.strftime("%Y-%m-%d %H:%M UTC").to_string();
    }
    ts.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::metrics::{
        AggregatedMetrics, BranchProtectionCounts, CodeownersCounts, DependabotCounts,
        PolicyCounts, RateMetric, SecretAlertCounts, SecretScanningCounts,
    };
    use crate::test_fixtures;
    use std::collections::HashMap;

    fn sample_metrics() -> AggregatedMetrics {
        AggregatedMetrics {
            security_policy_coverage: RateMetric::new(4, 5)
                .with_extra("observable_repositories", 5)
                .with_extra("unknown", 0),
            policy_counts: PolicyCounts {
                via_setting: 3,
                via_file: 1,
                unknown: 0,
                missing: 1,
            },
            secret_scanning_coverage: RateMetric::new(8, 10)
                .with_extra("disabled", 1)
                .with_extra("permission_denied", 0)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 9),
            secret_scanning_counts: SecretScanningCounts {
                enabled: 8,
                disabled: 1,
                permission_denied: 0,
                unknown: 1,
            },
            dependabot_security_updates_coverage: RateMetric::new(7, 10)
                .with_extra("disabled", 2)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 9),
            dependabot_security_updates_counts: DependabotCounts {
                enabled: 7,
                paused: 0,
                disabled: 2,
                unknown: 1,
            },
            open_secret_alert_prevalence: RateMetric::new(2, 8)
                .with_extra("repos_without_open_alerts", 6)
                .with_extra("unobservable", 2),
            secret_alert_counts: SecretAlertCounts {
                repos_with_open_alerts: 2,
                repos_without_open_alerts: 6,
                unobservable: 2,
            },
            branch_protection_coverage: RateMetric::new(6, 10)
                .with_extra("insufficient", 3)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 9),
            branch_protection_counts: BranchProtectionCounts {
                pass: 6,
                partial: 2,
                fail: 1,
                unknown: 1,
            },
            codeowners_coverage: RateMetric::new(7, 10)
                .with_extra("non_conforming", 2)
                .with_extra("absent", 2)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 9),
            codeowners_counts: CodeownersCounts {
                conforming: 5,
                non_conforming: 2,
                absent: 2,
                unknown: 1,
                truncated: 0,
            },
            owner_metrics: vec![],
        }
    }

    fn sample_evidence() -> Evidence {
        let mut metadata = test_fixtures::make_metadata();
        metadata.rate_limit_warnings = 2;

        let mut observability = test_fixtures::make_observability();
        observability.total_open_secret_alerts = 5;
        observability.observable_enabled_repositories = 8;
        observability.unobservable_repositories = 2;

        test_fixtures::make_full_evidence(
            metadata,
            test_fixtures::make_collection_statistics(10, 5, 3, 2),
            sample_metrics(),
            observability,
            vec![test_fixtures::all_passing_evidence("repo-1")],
        )
    }

    #[test]
    fn view_model_from_evidence_basic_fields() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.organization, "TestOrg");
        assert_eq!(vm.date, "2026-04-09");
        assert_eq!(vm.date_time, "2026-04-09 12:00 UTC");
        assert_eq!(vm.total_repos, 10);
    }

    #[test]
    fn view_model_total_all_repos_includes_archived_repos() {
        let mut evidence = sample_evidence();
        evidence.collection_statistics.archived_repos = 7;
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.total_repos, 10);
        assert_eq!(vm.archived_repos, 7);
        assert_eq!(vm.total_all_repos, vm.total_repos + vm.archived_repos);
    }

    #[test]
    fn format_run_timestamp_happy_path() {
        assert_eq!(
            format_run_timestamp("2026-04-12T14:30:05+00:00"),
            "2026-04-12 14:30 UTC"
        );
    }

    #[test]
    fn format_run_timestamp_z_suffix() {
        assert_eq!(
            format_run_timestamp("2026-04-12T14:30:05Z"),
            "2026-04-12 14:30 UTC"
        );
    }

    #[test]
    fn format_run_timestamp_non_utc_converts() {
        assert_eq!(
            format_run_timestamp("2026-04-12T14:30:05+05:30"),
            "2026-04-12 09:00 UTC"
        );
    }

    #[test]
    fn format_run_timestamp_unparseable_falls_back() {
        assert_eq!(format_run_timestamp("2026-04-12T14:30"), "2026-04-12T14:30");
    }

    #[test]
    fn format_run_timestamp_short_input() {
        assert_eq!(format_run_timestamp("2026-04-12"), "2026-04-12");
    }

    #[test]
    fn format_run_timestamp_empty() {
        assert_eq!(format_run_timestamp(""), "");
    }

    #[test]
    fn format_run_timestamp_no_t_separator() {
        assert_eq!(
            format_run_timestamp("2026-04-12 14:30:05+00:00"),
            "2026-04-12 14:30 UTC"
        );
    }

    #[test]
    fn format_run_timestamp_multibyte_no_panic() {
        assert_eq!(
            format_run_timestamp("日本語タイムスタンプ"),
            "日本語タイムスタンプ"
        );
    }

    #[test]
    fn view_model_formatted_coverage_strings() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.policy_coverage_formatted, "80.0% (4/5)");
        assert_eq!(vm.secret_scanning_coverage_formatted, "80.0% (8/10)");
        assert_eq!(vm.dependabot_coverage_formatted, "70.0% (7/10)");
        assert_eq!(vm.branch_protection_coverage_formatted, "60.0% (6/10)");
        assert_eq!(vm.codeowners_coverage_formatted, "70.0% (7/10)");
    }

    #[test]
    fn view_model_policy_counts() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.policy_via_setting, 3);
        assert_eq!(vm.policy_via_file, 1);
        assert_eq!(vm.policy_missing, 1);
        assert_eq!(vm.policy_unknown, 0);
    }

    #[test]
    fn view_model_observable_repos_from_extra() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.dependabot_observable_repos, 9);
        assert_eq!(vm.secret_scanning_observable_repos, 9);
    }

    #[test]
    fn view_model_assessment_metadata() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.public_repos, 5);
        assert_eq!(vm.internal_repos, 3);
        assert_eq!(vm.private_repos, 2);
        assert_eq!(vm.rate_limit_warnings, 2);
    }

    #[test]
    fn view_model_codeowners_paths() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.conforming_codeowners_path, ".github/CODEOWNERS");
        assert_eq!(vm.non_conforming_codeowners_path, "CODEOWNERS");
    }

    #[test]
    fn view_model_zero_denominator_shows_na() {
        let mut evidence = sample_evidence();
        evidence.metrics.security_policy_coverage = RateMetric::new(0, 0)
            .with_extra("observable_repositories", 0)
            .with_extra("unknown", 0);
        evidence.collection_statistics.total_repos = 0;

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());
        assert_eq!(vm.policy_coverage_formatted, "N/A (0/0)");
    }

    #[test]
    fn extra_u32_missing_key_returns_zero() {
        let extra = HashMap::new();
        assert_eq!(super::extra_u32(&extra, "nonexistent"), 0);
    }

    #[test]
    fn extra_u32_non_numeric_returns_zero() {
        let mut extra = HashMap::new();
        extra.insert(
            "key".to_string(),
            serde_json::Value::String("not a number".to_string()),
        );
        assert_eq!(super::extra_u32(&extra, "key"), 0);
    }

    #[test]
    fn tier_from_rate_pass() {
        let tiers = CoverageTiers::default();
        assert_eq!(
            CoverageTier::from_rate(Some(80.0), &tiers),
            CoverageTier::Pass
        );
        assert_eq!(
            CoverageTier::from_rate(Some(100.0), &tiers),
            CoverageTier::Pass
        );
    }

    #[test]
    fn tier_from_rate_warn() {
        let tiers = CoverageTiers::default();
        assert_eq!(
            CoverageTier::from_rate(Some(50.0), &tiers),
            CoverageTier::Warn
        );
        assert_eq!(
            CoverageTier::from_rate(Some(79.9), &tiers),
            CoverageTier::Warn
        );
    }

    #[test]
    fn tier_from_rate_fail() {
        let tiers = CoverageTiers::default();
        assert_eq!(
            CoverageTier::from_rate(Some(49.9), &tiers),
            CoverageTier::Fail
        );
        assert_eq!(
            CoverageTier::from_rate(Some(0.0), &tiers),
            CoverageTier::Fail
        );
    }

    #[test]
    fn tier_from_rate_na() {
        let tiers = CoverageTiers::default();
        assert_eq!(CoverageTier::from_rate(None, &tiers), CoverageTier::Na);
    }

    #[test]
    fn tier_equal_thresholds_no_warn_band() {
        let tiers = CoverageTiers {
            pass_threshold: 80.0,
            warn_threshold: 80.0,
        };
        assert_eq!(
            CoverageTier::from_rate(Some(80.0), &tiers),
            CoverageTier::Pass
        );
        assert_eq!(
            CoverageTier::from_rate(Some(79.9), &tiers),
            CoverageTier::Fail
        );
    }

    #[test]
    fn tier_css_classes() {
        assert_eq!(CoverageTier::Pass.css_class(), "tier-pass");
        assert_eq!(CoverageTier::Warn.css_class(), "tier-warn");
        assert_eq!(CoverageTier::Fail.css_class(), "tier-fail");
        assert_eq!(CoverageTier::Na.css_class(), "tier-na");
    }

    #[test]
    fn tier_custom_thresholds() {
        let tiers = CoverageTiers {
            pass_threshold: 90.0,
            warn_threshold: 60.0,
        };
        assert_eq!(
            CoverageTier::from_rate(Some(90.0), &tiers),
            CoverageTier::Pass
        );
        assert_eq!(
            CoverageTier::from_rate(Some(89.9), &tiers),
            CoverageTier::Warn
        );
        assert_eq!(
            CoverageTier::from_rate(Some(60.0), &tiers),
            CoverageTier::Warn
        );
        assert_eq!(
            CoverageTier::from_rate(Some(59.9), &tiers),
            CoverageTier::Fail
        );
    }

    #[test]
    fn view_model_tiers_computed_from_evidence() {
        let evidence = sample_evidence();
        let tiers = CoverageTiers::default();
        let vm = ReportViewModel::from_evidence(&evidence, &tiers);

        assert_eq!(vm.policy_tier, CoverageTier::Pass);
        assert_eq!(vm.secret_scanning_tier, CoverageTier::Pass);
        assert_eq!(vm.dependabot_tier, CoverageTier::Warn);
        assert_eq!(vm.branch_protection_tier, CoverageTier::Warn);
        assert_eq!(vm.codeowners_tier, CoverageTier::Warn);
    }

    #[test]
    fn view_model_tier_na_when_no_repos() {
        let mut evidence = sample_evidence();
        evidence.metrics.security_policy_coverage = RateMetric::new(0, 0);
        let tiers = CoverageTiers::default();
        let vm = ReportViewModel::from_evidence(&evidence, &tiers);

        assert_eq!(vm.policy_tier, CoverageTier::Na);
    }

    #[test]
    fn slug_basic_team() {
        assert_eq!(generate_slug("@org/team-name"), "org-team-name");
    }

    #[test]
    fn slug_basic_user() {
        assert_eq!(generate_slug("@username"), "username");
    }

    #[test]
    fn slug_preserves_underscores() {
        assert_eq!(generate_slug("@org/my_team"), "org-my_team");
    }

    #[test]
    fn slug_collapses_consecutive_dashes() {
        assert_eq!(generate_slug("@org//team"), "org-team");
    }

    #[test]
    fn slug_trims_leading_trailing_dashes() {
        assert_eq!(generate_slug("@/team/"), "team");
    }

    #[test]
    fn slug_special_characters_replaced() {
        assert_eq!(generate_slug("@org/team.name!"), "org-team-name");
        assert_eq!(generate_slug("@org/team.name"), "org-team-name");
    }

    #[test]
    fn slug_unicode_owner_names() {
        let slug = generate_slug("@组织/团队");
        assert!(slug.is_empty());
    }

    #[test]
    fn slug_empty_after_sanitization_fallback() {
        assert!(generate_slug("@///").is_empty());
    }

    #[test]
    fn unique_slugs_no_collision() {
        let owners = vec!["@org/team-a".to_string(), "@org/team-b".to_string()];
        let slugs = generate_unique_slugs(&owners);
        assert_eq!(slugs["@org/team-a"], "org-team-a");
        assert_eq!(slugs["@org/team-b"], "org-team-b");
    }

    #[test]
    fn unique_slugs_collision_disambiguation() {
        let owners = vec!["@org/team.name".to_string(), "@org/team_name".to_string()];
        let slugs = generate_unique_slugs(&owners);
        assert_eq!(slugs["@org/team.name"], "org-team-name");
        assert_eq!(slugs["@org/team_name"], "org-team_name");
    }

    #[test]
    fn unique_slugs_identical_owners() {
        let owners = vec!["@org/team".to_string(), "@org/team".to_string()];
        let slugs = generate_unique_slugs(&owners);
        assert_eq!(slugs.len(), 1);
    }

    #[test]
    fn unique_slugs_empty_fallback() {
        let owners = vec!["@///".to_string(), "@###".to_string()];
        let slugs = generate_unique_slugs(&owners);
        assert_eq!(slugs["@///"], "owner-1");
        assert_eq!(slugs["@###"], "owner-2");
    }

    #[test]
    fn unique_slugs_mixed_empty_and_normal() {
        let owners = vec!["@org/team".to_string(), "@///".to_string()];
        let slugs = generate_unique_slugs(&owners);
        assert_eq!(slugs["@org/team"], "org-team");
        assert_eq!(slugs["@///"], "owner-1");
    }

    #[test]
    fn width_class_none_returns_w0() {
        assert_eq!(rate_to_width_class(None), "w-0");
    }

    #[test]
    fn width_class_exact_boundaries() {
        assert_eq!(rate_to_width_class(Some(0.0)), "w-0");
        assert_eq!(rate_to_width_class(Some(5.0)), "w-5");
        assert_eq!(rate_to_width_class(Some(10.0)), "w-10");
        assert_eq!(rate_to_width_class(Some(15.0)), "w-15");
        assert_eq!(rate_to_width_class(Some(20.0)), "w-20");
        assert_eq!(rate_to_width_class(Some(25.0)), "w-25");
        assert_eq!(rate_to_width_class(Some(30.0)), "w-30");
        assert_eq!(rate_to_width_class(Some(35.0)), "w-35");
        assert_eq!(rate_to_width_class(Some(40.0)), "w-40");
        assert_eq!(rate_to_width_class(Some(45.0)), "w-45");
        assert_eq!(rate_to_width_class(Some(50.0)), "w-50");
        assert_eq!(rate_to_width_class(Some(55.0)), "w-55");
        assert_eq!(rate_to_width_class(Some(60.0)), "w-60");
        assert_eq!(rate_to_width_class(Some(65.0)), "w-65");
        assert_eq!(rate_to_width_class(Some(70.0)), "w-70");
        assert_eq!(rate_to_width_class(Some(75.0)), "w-75");
        assert_eq!(rate_to_width_class(Some(80.0)), "w-80");
        assert_eq!(rate_to_width_class(Some(85.0)), "w-85");
        assert_eq!(rate_to_width_class(Some(90.0)), "w-90");
        assert_eq!(rate_to_width_class(Some(95.0)), "w-95");
        assert_eq!(rate_to_width_class(Some(100.0)), "w-100");
    }

    #[test]
    fn width_class_rounds_to_nearest_5() {
        assert_eq!(rate_to_width_class(Some(2.4)), "w-0");
        assert_eq!(rate_to_width_class(Some(2.5)), "w-5");
        assert_eq!(rate_to_width_class(Some(7.4)), "w-5");
        assert_eq!(rate_to_width_class(Some(7.5)), "w-10");
        assert_eq!(rate_to_width_class(Some(33.3)), "w-35");
        assert_eq!(rate_to_width_class(Some(97.6)), "w-100");
    }

    #[test]
    fn width_class_clamps_out_of_range() {
        assert_eq!(rate_to_width_class(Some(-10.0)), "w-0");
        assert_eq!(rate_to_width_class(Some(150.0)), "w-100");
    }

    #[test]
    fn width_class_non_finite_returns_w0() {
        assert_eq!(rate_to_width_class(Some(f64::NAN)), "w-0");
        assert_eq!(rate_to_width_class(Some(f64::INFINITY)), "w-100");
        assert_eq!(rate_to_width_class(Some(f64::NEG_INFINITY)), "w-0");
    }

    #[test]
    fn slug_mixed_ascii_and_unicode() {
        assert_eq!(generate_slug("@org/team-名前"), "org-team");
    }

    #[test]
    fn health_score_all_none_returns_none() {
        assert_eq!(compute_health_score(&[None, None, None, None, None]), None);
    }

    #[test]
    fn health_score_all_100_returns_100() {
        let score = compute_health_score(&[
            Some(100.0),
            Some(100.0),
            Some(100.0),
            Some(100.0),
            Some(100.0),
        ]);
        assert_eq!(score, Some(100.0));
    }

    #[test]
    fn health_score_any_zero_floors_to_0_1() {
        let score =
            compute_health_score(&[Some(80.0), Some(0.0), Some(90.0), Some(70.0), Some(60.0)]);
        let s = score.unwrap();
        assert!(s > 0.0, "score should not collapse to zero; got {s}");
        assert!(
            s < 30.0,
            "score with a near-zero input should be low; got {s}"
        );
    }

    #[test]
    fn health_score_excludes_none_rates() {
        let score = compute_health_score(&[Some(80.0), None, None, None, Some(80.0)]);
        assert_eq!(score, Some(80.0));
    }

    #[test]
    fn health_score_single_rate() {
        let score = compute_health_score(&[None, None, Some(75.0), None, None]);
        assert_eq!(score, Some(75.0));
    }

    #[test]
    fn health_score_geometric_mean_mixed_rates() {
        let score =
            compute_health_score(&[Some(80.0), Some(70.0), Some(60.0), Some(80.0), Some(70.0)]);
        let s = score.unwrap();
        assert!((s - 71.5).abs() < 0.1, "expected ~71.5, got {s}");
    }

    #[test]
    fn health_score_geometric_mean_favors_balance() {
        let score = compute_health_score(&[Some(50.0), Some(100.0), None, None, None]);
        let s = score.unwrap();
        assert!((s - 70.7).abs() < 0.1, "expected ~70.7, got {s}");
        assert!(
            s < 75.0,
            "geometric mean should be less than arithmetic mean"
        );
    }

    #[test]
    fn health_score_rounds_to_one_decimal() {
        let score = compute_health_score(&[Some(33.3), Some(66.7), None, None, None]);
        let s = score.unwrap();
        assert!(
            (s * 10.0).fract().abs() < f64::EPSILON,
            "score {s} should be rounded to 1 decimal place"
        );
    }

    #[test]
    fn health_score_empty_slice_returns_none() {
        assert_eq!(compute_health_score(&[]), None);
    }

    #[test]
    fn health_score_negative_rate_floored_via_clamp() {
        let score = compute_health_score(&[Some(-10.0), Some(80.0), None, None, None]);
        let s = score.unwrap();
        assert!(s > 0.0, "score should not be zero; got {s}");
        assert!(
            s < 5.0,
            "score with a near-zero input should be very low; got {s}"
        );
    }

    #[test]
    fn health_score_rate_above_100_clamped() {
        let score = compute_health_score(&[Some(120.0), Some(80.0), None, None, None]);
        let s = score.unwrap();
        let expected = (100.0_f64 * 80.0).sqrt();
        assert!(
            (s - (expected * 10.0).round() / 10.0).abs() < 0.1,
            "expected ~{expected:.1}, got {s}",
        );
    }

    #[test]
    fn health_score_all_same_rate() {
        let score = compute_health_score(&[Some(50.0), Some(50.0), Some(50.0), None, None]);
        assert_eq!(score, Some(50.0));
    }

    #[test]
    fn view_model_health_score_computed() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert!(vm.health_score.is_some());
        let s = vm.health_score.unwrap();
        assert!((s - 71.5).abs() < 0.5, "expected ~71.5, got {s}");
        assert_eq!(vm.health_tier, CoverageTier::Warn);
        assert!(vm.health_score_formatted.contains('%'));
        assert_ne!(vm.health_width_class, "w-0");
    }

    #[test]
    fn view_model_health_score_na_when_all_zero_denominator() {
        let mut evidence = sample_evidence();
        evidence.metrics.security_policy_coverage = RateMetric::new(0, 0);
        evidence.metrics.secret_scanning_coverage = RateMetric::new(0, 0);
        evidence.metrics.dependabot_security_updates_coverage = RateMetric::new(0, 0);
        evidence.metrics.branch_protection_coverage = RateMetric::new(0, 0);
        evidence.metrics.codeowners_coverage = RateMetric::new(0, 0);

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());
        assert_eq!(vm.health_score, None);
        assert_eq!(vm.health_tier, CoverageTier::Na);
        assert_eq!(vm.health_score_formatted, "N/A");
        assert_eq!(vm.health_width_class, "w-0");
    }

    #[test]
    fn view_model_stale_rate_none_when_no_archived_no_stale() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.stale_rate, None);
        assert_eq!(vm.stale_rate_formatted, "N/A");
        assert_eq!(vm.stale_tier, CoverageTier::Na);
        assert_eq!(vm.stale_width_class, "w-0");
        assert_eq!(vm.archived_repos, 0);
        assert_eq!(vm.stale_active_repos, 0);
    }

    #[test]
    fn view_model_stale_rate_with_archived_repos() {
        let mut evidence = sample_evidence();
        evidence.collection_statistics.archived_repos = 3;

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.stale_rate, Some(100.0));
        assert_eq!(vm.stale_rate_formatted, "100.0%");
        assert_eq!(vm.archived_repos, 3);
        assert_eq!(vm.stale_active_repos, 0);
    }

    #[test]
    fn view_model_stale_rate_with_stale_active_repos() {
        let mut evidence = sample_evidence();
        evidence.repositories[0].repository.updated_at = Some("2023-01-01T00:00:00Z".to_string());
        evidence.collection_statistics.archived_repos = 2;

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.stale_active_repos, 1);
        assert_eq!(vm.archived_repos, 2);
        let s = vm.stale_rate.unwrap();
        assert!((s - 66.7).abs() < 0.1, "expected ~66.7, got {s}");
    }

    #[test]
    fn view_model_stale_rate_included_in_health_score() {
        let mut evidence = sample_evidence();
        evidence.collection_statistics.archived_repos = 5;

        let vm_with_stale = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        let evidence_no_stale = sample_evidence();
        let vm_without =
            ReportViewModel::from_evidence(&evidence_no_stale, &CoverageTiers::default());

        assert_ne!(
            vm_with_stale.health_score, vm_without.health_score,
            "stale rate should affect health score"
        );
    }

    #[test]
    fn strip_org_prefix_team() {
        assert_eq!(strip_org_prefix("@org/team-name"), "team-name");
    }

    #[test]
    fn strip_org_prefix_user_no_slash() {
        assert_eq!(strip_org_prefix("@username"), "@username");
    }

    #[test]
    fn strip_org_prefix_nested_slash() {
        assert_eq!(strip_org_prefix("@org/sub/team"), "sub/team");
    }

    #[test]
    fn strip_org_prefix_empty_string() {
        assert_eq!(strip_org_prefix(""), "");
    }

    #[test]
    fn strip_org_prefix_at_only() {
        assert_eq!(strip_org_prefix("@"), "@");
    }

    #[test]
    fn strip_org_prefix_trailing_slash() {
        assert_eq!(strip_org_prefix("@org/"), "");
    }

    #[test]
    fn strip_org_prefix_leading_slash_after_at() {
        assert_eq!(strip_org_prefix("@/team"), "team");
    }

    #[test]
    fn strip_org_prefix_multibyte_no_panic() {
        assert_eq!(strip_org_prefix("@組織/チーム"), "チーム");
    }
}
