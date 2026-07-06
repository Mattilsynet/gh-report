//! Report view model: shapes data for template rendering.
//!
//! Keeps the template layer thin by pre-computing all display values.
//! The view model is a flat, template-friendly representation of the
//! [`Evidence`] domain object — no nested traversal or formatting
//! logic belongs in the template.

use std::collections::{BTreeMap, HashMap, HashSet};

use jiff::{SignedDuration, Timestamp};

use crate::config;
use crate::config::dashboard::CoverageTiers;
use crate::domain::auth::{AuthMode, Capability, TokenTier};
use crate::domain::checks::{CollectionFailureReason, SecretScanningStatus};
use crate::domain::evidence::{AssessmentMetadata, Evidence, RepositoryEvidence};
use crate::domain::metrics::{
    AggregatedMetrics, CollectionHealthCheckKind, CollectionHealthCount, OwnerType,
};
use crate::domain::time::{is_repo_stale, parse_iso8601};

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

/// A single member row in a team roster (B1).
#[derive(Debug, Clone)]
pub struct TeamMemberRow {
    /// GitHub login.
    pub login: String,
    /// `"Maintainer"` or `"Member"`.
    pub role_label: &'static str,
    /// URL to the member's GitHub profile.
    pub profile_url: String,
}

/// View model for a team's member roster (B1), embedded in
/// [`OwnerDetailViewModel`] for team-type owners.
#[derive(Debug, Clone)]
pub struct TeamRosterViewModel {
    /// Whether the roster fetch completed; drives a degraded-state notice.
    pub is_complete: bool,
    /// Human-readable fetch status (e.g., `"Complete"`, `"Permission denied"`).
    pub status_label: &'static str,
    /// Roster rows, sorted by login.
    pub members: Vec<TeamMemberRow>,
    /// `members.len()`, for the section heading.
    pub member_count: u32,
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
    /// Team member roster (B1). `Some` only for team-type owners with a
    /// fetched roster; `None` for user-type owners or when B1 has not
    /// (yet) collected this team.
    pub roster: Option<TeamRosterViewModel>,
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
    /// Display name of the team the last committer belongs to (B2), if any
    /// fetched roster lists `last_committer_login` as a member. `None` when
    /// no roster match is found (committer unknown, not on any team, or no
    /// roster fetched).
    pub attributed_team: Option<String>,
    /// URL-safe slug of `attributed_team`, for linking to its detail page.
    pub attributed_team_slug: Option<String>,
}

/// One team's group of orphan repos attributed via last-committer
/// membership (B2), for the "Orphans by Team" section.
#[derive(Debug, Clone)]
pub struct OrphanedTeamGroup {
    /// Display name of the team.
    pub team: String,
    /// Short display name with org prefix stripped.
    pub team_short: String,
    /// URL-safe slug, for linking to the team's detail page.
    pub slug: String,
    /// Orphan repos attributed to this team, sorted by repo name.
    pub rows: Vec<OrphanedRepoRow>,
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
    /// Orphan repos grouped by attributed team (B2), sorted by team name.
    /// Only teams with at least one attributed orphan appear.
    pub by_team: Vec<OrphanedTeamGroup>,
}

/// A row in the deleted repositories table.
#[derive(Debug, Clone)]
pub struct DeletedRepoRow {
    /// Repository name.
    pub repo_name: String,
    /// ISO 8601 timestamp when deletion was detected.
    pub detected_at: String,
}

/// View model for the deleted repositories page.
#[derive(Debug, Clone)]
pub struct DeletedViewModel {
    /// One row per deleted repo, sorted by name.
    pub rows: Vec<DeletedRepoRow>,
    /// Organization name for the page title.
    pub organization: String,
    /// Total count of deleted repos.
    pub deleted_count: u32,
}

/// Operator-facing diagnostics for collection health and credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminDiagnosticsViewModel {
    /// Deterministically ordered technical issue sections.
    pub collection_health_sections: Vec<CollectionHealthSection>,
    /// Total technical issues across every collection-health section.
    pub technical_issues_total: u32,
    /// Active credential mode, tier, and degraded capabilities.
    pub credentials: CredentialLimitationsViewModel,
    /// Derived red flags, sorted by severity then category then id.
    pub red_flags: Vec<RedFlag>,
}

impl AdminDiagnosticsViewModel {
    /// Whether any technical issue is present.
    #[must_use]
    pub fn has_technical_issues(&self) -> bool {
        self.technical_issues_total > 0
    }
}

/// One collection-health check-kind section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionHealthSection {
    /// Collection-health check kind.
    pub check_kind: CollectionHealthCheckKind,
    /// Human-readable check-kind label.
    pub label: &'static str,
    /// Ordered reason rows within the section.
    pub rows: Vec<CollectionHealthReasonRow>,
    /// Subtotal for every reason row in the section.
    pub subtotal: u32,
}

/// One collection-health reason row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionHealthReasonRow {
    /// Failure taxonomy reason.
    pub reason: CollectionFailureReason,
    /// Human-readable reason label.
    pub label: String,
    /// Number of repositories or checks counted for this reason.
    pub count: u32,
}

/// Credential limitations shown on the admin diagnostics page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialLimitationsViewModel {
    /// Authentication mode used by the active collection run.
    pub auth_mode: AuthMode,
    /// Human-readable authentication mode.
    pub auth_mode_label: String,
    /// Token capability tier used by the active collection run.
    pub token_tier: TokenTier,
    /// Human-readable token tier.
    pub token_tier_label: String,
    /// Ordered unavailable capabilities reported by assessment metadata.
    pub unavailable_capabilities: Vec<CredentialCapabilityLimitation>,
}

impl CredentialLimitationsViewModel {
    /// Whether the active credential set has degraded capabilities.
    #[must_use]
    pub fn has_degraded_capabilities(&self) -> bool {
        !self.unavailable_capabilities.is_empty()
    }
}

/// One unavailable credential capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialCapabilityLimitation {
    /// Capability reported unavailable by the collector.
    pub capability: Capability,
    /// Human-readable capability label.
    pub label: String,
}

/// Severity ranking for a derived red flag.
///
/// Declaration order is the primary sort order used by [`build_red_flags`]
/// (most severe first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Collection is materially incomplete or actively misleading.
    Critical,
    /// Actionable now; meaningfully degrades coverage or posture.
    High,
    /// Actionable; narrower or lower-confidence impact than `High`.
    Medium,
}

impl Severity {
    /// CSS class for the red-flag card's left-border accent.
    #[must_use]
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Critical => "severity-critical",
            Self::High => "severity-high",
            Self::Medium => "severity-medium",
        }
    }

    /// CSS class for status-dot rendering; reuses the existing status palette.
    #[must_use]
    pub fn status_dot_class(&self) -> &'static str {
        match self {
            Self::Critical => "status-fail",
            Self::High => "status-warn",
            Self::Medium => "status-unknown",
        }
    }
}

/// Routing axis for a derived red flag: who is best placed to act on it.
///
/// Declaration order is the secondary sort order used by [`build_red_flags`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RedFlagCategory {
    /// Fix is a token/credential rotation or a deployment configuration change.
    Credential,
    /// Fix is investigating an incomplete or stale collection run.
    Integrity,
    /// Fix is a configuration change on the affected repository itself.
    Posture,
}

impl RedFlagCategory {
    /// Human-readable category label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Credential => "Credential",
            Self::Integrity => "Integrity",
            Self::Posture => "Posture",
        }
    }
}

/// Stable identity for a derived red-flag family.
///
/// Declaration order is the tie-breaking sort order used by
/// [`build_red_flags`] after severity and category. A single id may back
/// more than one [`RedFlag`] (for example, `DegradedCapability` emits one
/// flag per unavailable capability).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RedFlagId {
    /// `token_tier == TokenTier::Unknown`.
    TokenTierUnknown,
    /// An entry in `unavailable_capabilities`.
    DegradedCapability,
    /// Branch-protection reads suspected permission-denied.
    BranchProtectionPermissionSuspected,
    /// Secret-scanning reads denied by permissions.
    SecretScanningPermissionDenied,
    /// `auth_mode == AuthMode::Pat`.
    AuthModePat,
    /// Any check rate-limited during collection.
    RateLimitedCollection,
    /// Any check returned a transient or invalid response.
    UnstableCollectionResult,
    /// CODEOWNERS content parsing was skipped for one or more repos.
    CodeownersTruncated,
    /// The last completed run is older than twice the collection interval.
    CollectionRunStale,
    /// Secret scanning reports enabled but alert data is not observable.
    SecretScanningUnobservable,
    /// Branch protection confirmed absent on the default branch.
    BranchProtectionAbsent,
    /// Branch protection has a broad bypass actor.
    BroadBypassPresent,
    /// Branch protection does not bind repository administrators.
    AdminEnforcementNotEquivalent,
    /// Force pushes or branch deletion are not blocked on the default branch.
    IntegrityControlGap,
}

/// Magnitude or scope of the repositories affected by a red flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AffectedScope {
    /// A count of affected repositories or checks, without individual identity.
    Count(u32),
    /// The specific affected repositories, as `organization/name`.
    Repos(Vec<String>),
    /// The condition applies to the whole collection run, not specific repos.
    OrgWide,
}

/// What kind of action clears a red flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixTarget {
    /// Rotate or reconfigure the collection credential.
    Token,
    /// Change Cloud Run or other deployment configuration.
    CloudRunConfig,
    /// Change settings on the named repository.
    Repo(String),
    /// Re-run collection or inspect collector logs.
    Investigate,
}

/// Actionable remedy attached to a red flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remedy {
    /// Imperative instruction (for example, `"Rotate to a token with ..."`).
    pub summary: String,
    /// Deep-link anchor into `OPERATIONS.md`, when a matching section exists.
    pub anchor: Option<&'static str>,
    /// What kind of action clears the flag.
    pub fix_target: FixTarget,
}

/// A single derived red-flag finding for the admin diagnostics page.
///
/// Built by [`build_red_flags`] from already-collected evidence; carries no
/// state of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedFlag {
    /// Stable identity for the flag family.
    pub id: RedFlagId,
    /// Severity; drives colour and sort order.
    pub severity: Severity,
    /// Who is best placed to act on this flag.
    pub category: RedFlagCategory,
    /// Human-readable title.
    pub title: String,
    /// Cause-to-effect sentence explaining the finding.
    pub detail: String,
    /// Scope of repositories or checks affected.
    pub affected: AffectedScope,
    /// Actionable remedy.
    pub remedy: Remedy,
}

/// Count of derived red flags at each [`Severity`] level.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RedFlagSeverityCounts {
    /// Number of `Severity::Critical` flags.
    pub critical: u32,
    /// Number of `Severity::High` flags.
    pub high: u32,
    /// Number of `Severity::Medium` flags.
    pub medium: u32,
}

impl AdminDiagnosticsViewModel {
    /// Count of `red_flags` at each severity level, for the Red Flags
    /// section summary line.
    #[must_use]
    pub fn red_flag_severity_counts(&self) -> RedFlagSeverityCounts {
        let mut counts = RedFlagSeverityCounts::default();
        for flag in &self.red_flags {
            match flag.severity {
                Severity::Critical => counts.critical = counts.critical.saturating_add(1),
                Severity::High => counts.high = counts.high.saturating_add(1),
                Severity::Medium => counts.medium = counts.medium.saturating_add(1),
            }
        }
        counts
    }

    /// Most severe [`Severity`] among `red_flags`, or `None` when none fired.
    ///
    /// `Severity`'s declaration order is `Critical < High < Medium`, so the
    /// iterator minimum is the most severe flag present. Drives the admin
    /// nav badge's severity-class modifier.
    #[must_use]
    pub fn max_red_flag_severity(&self) -> Option<Severity> {
        self.red_flags.iter().map(|flag| flag.severity).min()
    }
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
    /// Unique identifier for the collection run.
    pub run_id: String,
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
    pub org_alert_collection_status: String,
    pub org_alert_total_open_secret_alerts: u64,

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
    /// Count of deleted repositories (drives nav link label).
    pub deleted_count: u32,

    /// Whether this report was rendered from a cached baseline (warm-start)
    /// rather than a fresh API collection.
    pub warm_start: bool,

    /// Read-only operator diagnostics for collection-health and credential limits.
    pub admin_diagnostics: AdminDiagnosticsViewModel,
}

struct OrgAlertDisplay {
    status: String,
    total_open_secret_alerts: u64,
}

struct HealthDisplay {
    score: Option<f64>,
    tier: CoverageTier,
    score_formatted: String,
    width_class: &'static str,
}

fn org_alert_display(evidence: &Evidence) -> OrgAlertDisplay {
    OrgAlertDisplay {
        status: evidence
            .secret_scanning_observability
            .collection_status
            .to_string(),
        total_open_secret_alerts: u64::from(
            evidence
                .secret_scanning_observability
                .total_open_secret_alerts,
        ),
    }
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
        let org_alert = org_alert_display(evidence);
        let admin_diagnostics = build_admin_diagnostics(metadata, m, &evidence.repositories);

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

        let health = health_display(m, stale_rate, tiers);

        Self {
            organization: metadata.organization.clone(),
            date: metadata.date.clone(),
            date_time: format_run_timestamp(&metadata.run_timestamp),
            run_id: metadata.run_id.clone(),
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
            org_alert_collection_status: org_alert.status,
            org_alert_total_open_secret_alerts: org_alert.total_open_secret_alerts,
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
            health_score: health.score,
            health_tier: health.tier,
            health_score_formatted: health.score_formatted,
            health_width_class: health.width_class,
            stale_rate,
            stale_rate_formatted,
            stale_tier,
            stale_width_class,
            archived_repos: archived,
            stale_active_repos,
            owners: None,
            top_security_teams: Vec::new(),
            orphaned_count: 0,
            deleted_count: u32::try_from(evidence.deleted.len()).unwrap_or(u32::MAX),
            warm_start: metadata.warm_start,
            admin_diagnostics,
        }
    }
}

fn health_display(
    metrics: &AggregatedMetrics,
    stale_rate: Option<f64>,
    tiers: &CoverageTiers,
) -> HealthDisplay {
    let score = compute_health_score(&[
        metrics.security_policy_coverage.rate,
        metrics.secret_scanning_coverage.rate,
        metrics.dependabot_security_updates_coverage.rate,
        metrics.branch_protection_coverage.rate,
        metrics.codeowners_coverage.rate,
        stale_rate,
    ]);

    HealthDisplay {
        score,
        tier: CoverageTier::from_rate(score, tiers),
        score_formatted: score.map_or_else(|| "N/A".to_string(), |s| format!("{s:.1}%")),
        width_class: rate_to_width_class(score),
    }
}

fn build_admin_diagnostics(
    metadata: &crate::domain::evidence::AssessmentMetadata,
    metrics: &AggregatedMetrics,
    repos: &[RepositoryEvidence],
) -> AdminDiagnosticsViewModel {
    let (collection_health_sections, technical_issues_total) =
        build_collection_health_sections(&metrics.collection_health_counts);

    AdminDiagnosticsViewModel {
        collection_health_sections,
        technical_issues_total,
        credentials: build_credential_limitations(metadata),
        red_flags: build_red_flags(metadata, metrics, repos),
    }
}

fn collection_health_label(kind: CollectionHealthCheckKind) -> &'static str {
    match kind {
        CollectionHealthCheckKind::BranchProtection => "Branch Protection",
        CollectionHealthCheckKind::SecretScanning => "Secret Scanning",
        CollectionHealthCheckKind::Dependabot => "Dependabot",
        CollectionHealthCheckKind::Codeowners => "CODEOWNERS",
        CollectionHealthCheckKind::SecurityPolicy => "Security Policy",
        CollectionHealthCheckKind::Inventory => "Inventory",
        CollectionHealthCheckKind::Rulesets => "Rulesets",
    }
}

fn build_collection_health_sections(
    counts: &[CollectionHealthCount],
) -> (Vec<CollectionHealthSection>, u32) {
    let mut grouped = BTreeMap::new();
    for count in counts {
        let key = (count.check_kind, count.reason);
        let total = grouped.entry(key).or_insert(0_u32);
        *total = total.saturating_add(count.count);
    }

    let mut sections = Vec::new();
    let technical_issues_total = grouped
        .values()
        .fold(0_u32, |total, count| total.saturating_add(*count));

    for ((check_kind, reason), count) in grouped {
        let row = CollectionHealthReasonRow {
            reason,
            label: reason.to_string(),
            count,
        };

        if let Some(section) = sections
            .last_mut()
            .filter(|section: &&mut CollectionHealthSection| section.check_kind == check_kind)
        {
            section.subtotal = section.subtotal.saturating_add(row.count);
            section.rows.push(row);
        } else {
            let subtotal = row.count;
            sections.push(CollectionHealthSection {
                check_kind,
                label: collection_health_label(check_kind),
                rows: vec![row],
                subtotal,
            });
        }
    }

    (sections, technical_issues_total)
}

fn capability_order(capability: Capability) -> u8 {
    match capability {
        Capability::OrgSecretScanningAlerts => 0,
        Capability::PrivateBranchProtectionRead => 1,
    }
}

fn build_credential_limitations(
    metadata: &crate::domain::evidence::AssessmentMetadata,
) -> CredentialLimitationsViewModel {
    let mut unavailable = metadata.unavailable_capabilities.clone();
    unavailable.sort_by_key(|capability| capability_order(*capability));

    CredentialLimitationsViewModel {
        auth_mode: metadata.auth_mode,
        auth_mode_label: metadata.auth_mode.to_string(),
        token_tier: metadata.token_tier,
        token_tier_label: metadata.token_tier.to_string(),
        unavailable_capabilities: unavailable
            .into_iter()
            .map(|capability| CredentialCapabilityLimitation {
                capability,
                label: capability.to_string(),
            })
            .collect(),
    }
}

/// Derive the admin diagnostics "Red Flags" from already-collected evidence.
///
/// Render-time-only: every input is already present in [`AssessmentMetadata`],
/// [`AggregatedMetrics`], and the per-repository [`RepositoryEvidence`] slice.
/// Adds no persisted field; the only input besides the three parameters is
/// the current time, used solely to detect a stale collection run
/// ([`RedFlagId::CollectionRunStale`]).
///
/// Sorted by severity (most severe first), then category, then id.
#[must_use]
pub fn build_red_flags(
    metadata: &AssessmentMetadata,
    metrics: &AggregatedMetrics,
    repos: &[RepositoryEvidence],
) -> Vec<RedFlag> {
    let mut flags = Vec::new();

    push_token_tier_unknown(&mut flags, metadata);
    push_degraded_capabilities(&mut flags, metadata);
    push_branch_protection_permission_suspected(&mut flags, metrics);
    push_secret_scanning_permission_denied(&mut flags, metrics);
    push_auth_mode_pat(&mut flags, metadata);
    push_rate_limited_collection(&mut flags, metrics);
    push_unstable_collection_result(&mut flags, metrics);
    push_codeowners_truncated(&mut flags, metrics);
    push_collection_run_stale(&mut flags, metadata);
    push_secret_scanning_unobservable(&mut flags, repos);
    push_branch_protection_posture_flags(&mut flags, metadata, repos);

    flags.sort_by_key(|flag| (flag.severity, flag.category, flag.id));
    flags
}

fn repo_full_name(metadata: &AssessmentMetadata, repo: &RepositoryEvidence) -> String {
    format!("{}/{}", metadata.organization, repo.repository.name)
}

fn sum_collection_health(
    counts: &[CollectionHealthCount],
    predicate: impl Fn(&CollectionHealthCount) -> bool,
) -> u32 {
    counts
        .iter()
        .filter(|count| predicate(count))
        .fold(0_u32, |total, count| total.saturating_add(count.count))
}

fn push_token_tier_unknown(flags: &mut Vec<RedFlag>, metadata: &AssessmentMetadata) {
    if metadata.token_tier != TokenTier::Unknown {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::TokenTierUnknown,
        severity: Severity::High,
        category: RedFlagCategory::Credential,
        title: "Token capability tier is unknown".to_string(),
        detail: "Token tier could not be classified from OAuth scopes; optional checks may be silently skipped without being flagged as failures.".to_string(),
        affected: AffectedScope::OrgWide,
        remedy: Remedy {
            summary: "Confirm the active token's effective permissions against the Required Permissions / Scopes table; fine-grained PATs and GitHub Apps are expected to show Unknown.".to_string(),
            anchor: Some("fine-grained-pat--github-app"),
            fix_target: FixTarget::Token,
        },
    });
}

fn degraded_capability_text(capability: Capability) -> (&'static str, &'static str) {
    match capability {
        Capability::OrgSecretScanningAlerts => (
            "Org-level secret scanning alerts capability unavailable",
            "Organization-wide secret-scanning alert observability is degraded; alert-based coverage metrics may undercount.",
        ),
        Capability::PrivateBranchProtectionRead => (
            "Private/internal branch-protection reads capability unavailable",
            "Private and internal repositories cannot be distinguished from not-found responses; branch protection scores Unknown for them.",
        ),
    }
}

fn push_degraded_capabilities(flags: &mut Vec<RedFlag>, metadata: &AssessmentMetadata) {
    let mut capabilities = metadata.unavailable_capabilities.clone();
    capabilities.sort_by_key(|capability| capability_order(*capability));

    for capability in capabilities {
        let (title, detail) = degraded_capability_text(capability);
        flags.push(RedFlag {
            id: RedFlagId::DegradedCapability,
            severity: Severity::High,
            category: RedFlagCategory::Credential,
            title: title.to_string(),
            detail: detail.to_string(),
            affected: AffectedScope::OrgWide,
            remedy: Remedy {
                summary: "Grant the missing scope or permission listed under Capability probes, or accept the degraded coverage for this credential tier.".to_string(),
                anchor: Some("capability-probes"),
                fix_target: FixTarget::Token,
            },
        });
    }
}

fn push_branch_protection_permission_suspected(
    flags: &mut Vec<RedFlag>,
    metrics: &AggregatedMetrics,
) {
    let count = sum_collection_health(&metrics.collection_health_counts, |count| {
        count.check_kind == CollectionHealthCheckKind::BranchProtection
            && count.reason == CollectionFailureReason::PermissionSuspected
    });
    if count == 0 {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::BranchProtectionPermissionSuspected,
        severity: Severity::Medium,
        category: RedFlagCategory::Credential,
        title: "Branch protection reads suspected permission-denied".to_string(),
        detail: "One or more branch-protection checks returned an ambiguous not-found response consistent with insufficient read access on a private repository.".to_string(),
        affected: AffectedScope::Count(count),
        remedy: Remedy {
            summary: "Verify the token has Repository administration: read (or classic repo scope) and retry collection.".to_string(),
            anchor: Some("capability-probes"),
            fix_target: FixTarget::Token,
        },
    });
}

fn push_secret_scanning_permission_denied(flags: &mut Vec<RedFlag>, metrics: &AggregatedMetrics) {
    let count = sum_collection_health(&metrics.collection_health_counts, |count| {
        count.check_kind == CollectionHealthCheckKind::SecretScanning
            && count.reason == CollectionFailureReason::PermissionDenied
    });
    if count == 0 {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::SecretScanningPermissionDenied,
        severity: Severity::Medium,
        category: RedFlagCategory::Credential,
        title: "Secret scanning reads denied by permissions".to_string(),
        detail: "One or more repositories returned an explicit permission-denied response for secret scanning; alert coverage for those repos is incomplete.".to_string(),
        affected: AffectedScope::Count(count),
        remedy: Remedy {
            summary: "Grant Secret scanning alerts: read (or classic repo + security_events scope) and retry collection.".to_string(),
            anchor: Some("capability-probes"),
            fix_target: FixTarget::Token,
        },
    });
}

fn push_auth_mode_pat(flags: &mut Vec<RedFlag>, metadata: &AssessmentMetadata) {
    if !matches!(metadata.auth_mode, AuthMode::Pat) {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::AuthModePat,
        severity: Severity::Medium,
        category: RedFlagCategory::Credential,
        title: "Running on Personal Access Token authentication".to_string(),
        detail: "PAT is a fallback credential class; GitHub App authentication is the recommended production mode.".to_string(),
        affected: AffectedScope::OrgWide,
        remedy: Remedy {
            summary: "Switch production deployments to GitHub App authentication.".to_string(),
            anchor: Some("1-github-app-recommended-for-production"),
            fix_target: FixTarget::CloudRunConfig,
        },
    });
}

fn push_rate_limited_collection(flags: &mut Vec<RedFlag>, metrics: &AggregatedMetrics) {
    let count = sum_collection_health(&metrics.collection_health_counts, |count| {
        count.reason == CollectionFailureReason::RateLimited
    });
    if count == 0 {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::RateLimitedCollection,
        severity: Severity::High,
        category: RedFlagCategory::Integrity,
        title: "Collection requests were rate-limited".to_string(),
        detail: "One or more checks were rate-limited by the GitHub API; the affected repositories' results are incomplete for this run.".to_string(),
        affected: AffectedScope::Count(count),
        remedy: Remedy {
            summary: "Re-run collection after the rate-limit window resets, or reduce the concurrent worker count.".to_string(),
            anchor: None,
            fix_target: FixTarget::Investigate,
        },
    });
}

fn push_unstable_collection_result(flags: &mut Vec<RedFlag>, metrics: &AggregatedMetrics) {
    let count = sum_collection_health(&metrics.collection_health_counts, |count| {
        matches!(
            count.reason,
            CollectionFailureReason::Transient | CollectionFailureReason::Invalid
        )
    });
    if count == 0 {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::UnstableCollectionResult,
        severity: Severity::Medium,
        category: RedFlagCategory::Integrity,
        title: "Collection responses were transient or invalid".to_string(),
        detail: "One or more checks failed with a retryable or malformed response; the affected repositories' results may not reflect current state.".to_string(),
        affected: AffectedScope::Count(count),
        remedy: Remedy {
            summary: "Re-run collection; if the condition persists, inspect collector logs for the affected check.".to_string(),
            anchor: None,
            fix_target: FixTarget::Investigate,
        },
    });
}

fn push_codeowners_truncated(flags: &mut Vec<RedFlag>, metrics: &AggregatedMetrics) {
    let count = metrics.codeowners_counts.truncated;
    if count == 0 {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::CodeownersTruncated,
        severity: Severity::Medium,
        category: RedFlagCategory::Integrity,
        title: "CODEOWNERS parsing was truncated".to_string(),
        detail: "One or more CODEOWNERS files were found but not parsed (encoding mismatch, oversized payload, or decode failure); owner coverage for those repos is incomplete.".to_string(),
        affected: AffectedScope::Count(count),
        remedy: Remedy {
            summary: "Inspect the affected CODEOWNERS files for encoding or size issues and re-run collection.".to_string(),
            anchor: None,
            fix_target: FixTarget::Investigate,
        },
    });
}

fn is_collection_run_stale(run_timestamp: &str) -> bool {
    let Some(run_ts) = parse_iso8601(run_timestamp) else {
        return false;
    };
    let threshold_secs =
        i64::try_from(config::COLLECTION_INTERVAL_SECS.saturating_mul(2)).unwrap_or(i64::MAX);
    Timestamp::now().duration_since(run_ts) > SignedDuration::from_secs(threshold_secs)
}

fn push_collection_run_stale(flags: &mut Vec<RedFlag>, metadata: &AssessmentMetadata) {
    if !is_collection_run_stale(&metadata.run_timestamp) {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::CollectionRunStale,
        severity: Severity::High,
        category: RedFlagCategory::Integrity,
        title: "Collection run is stale".to_string(),
        detail: "The last completed collection run is more than twice the scheduled collection interval old; the daemon may be stuck and this report may not reflect current state.".to_string(),
        affected: AffectedScope::OrgWide,
        remedy: Remedy {
            summary: "Check daemon liveness/readiness and Cloud Run min-instances, then restart the collection process if it is stuck.".to_string(),
            anchor: Some("kubernetes--knative-probe-configuration"),
            fix_target: FixTarget::Investigate,
        },
    });
}

fn count_secret_scanning_unobservable(repos: &[RepositoryEvidence]) -> u32 {
    let count = repos
        .iter()
        .filter(|repo| {
            repo.checks.secret_scanning.status == SecretScanningStatus::Enabled
                && !repo.checks.secret_scanning.alerts_observable
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn push_secret_scanning_unobservable(flags: &mut Vec<RedFlag>, repos: &[RepositoryEvidence]) {
    let count = count_secret_scanning_unobservable(repos);
    if count == 0 {
        return;
    }
    flags.push(RedFlag {
        id: RedFlagId::SecretScanningUnobservable,
        severity: Severity::Medium,
        category: RedFlagCategory::Integrity,
        title: "Secret scanning enabled but alerts not observable".to_string(),
        detail: "One or more repositories report secret scanning enabled but alert data is not observable; the enabled status is cosmetic for those repos.".to_string(),
        affected: AffectedScope::Count(count),
        remedy: Remedy {
            summary: "Verify organization-level secret-scanning alert access and re-run collection.".to_string(),
            anchor: Some("capability-probes"),
            fix_target: FixTarget::Token,
        },
    });
}

struct PostureScanResults {
    absent: Vec<String>,
    broad_bypass: Vec<String>,
    admin_not_bound: Vec<String>,
    integrity_gap: Vec<String>,
}

fn scan_branch_protection_posture(
    metadata: &AssessmentMetadata,
    repos: &[RepositoryEvidence],
) -> PostureScanResults {
    let mut results = PostureScanResults {
        absent: Vec::new(),
        broad_bypass: Vec::new(),
        admin_not_bound: Vec::new(),
        integrity_gap: Vec::new(),
    };

    for repo in repos {
        let details = &repo.checks.branch_protection.details;
        let name = repo_full_name(metadata, repo);

        if details.reason_kind == Some(CollectionFailureReason::NotFoundAbsent) {
            results.absent.push(name.clone());
        }
        if details.has_broad_bypass == Some(true) {
            results.broad_bypass.push(name.clone());
        }
        if details.admin_equivalent == Some(false) {
            results.admin_not_bound.push(name.clone());
        }
        if details.force_push_blocked == Some(false) || details.deletion_blocked == Some(false) {
            results.integrity_gap.push(name);
        }
    }

    results.absent.sort_unstable();
    results.broad_bypass.sort_unstable();
    results.admin_not_bound.sort_unstable();
    results.integrity_gap.sort_unstable();
    results
}

#[derive(Clone, Copy)]
struct PostureFlagSpec {
    id: RedFlagId,
    severity: Severity,
    title: &'static str,
    detail: &'static str,
    remedy_summary: &'static str,
}

fn push_posture_flag(flags: &mut Vec<RedFlag>, affected_repos: Vec<String>, spec: PostureFlagSpec) {
    let Some(primary) = affected_repos.first().cloned() else {
        return;
    };
    flags.push(RedFlag {
        id: spec.id,
        severity: spec.severity,
        category: RedFlagCategory::Posture,
        title: spec.title.to_string(),
        detail: spec.detail.to_string(),
        affected: AffectedScope::Repos(affected_repos),
        remedy: Remedy {
            summary: spec.remedy_summary.to_string(),
            anchor: Some("branch-protection-coverage"),
            fix_target: FixTarget::Repo(primary),
        },
    });
}

fn push_branch_protection_posture_flags(
    flags: &mut Vec<RedFlag>,
    metadata: &AssessmentMetadata,
    repos: &[RepositoryEvidence],
) {
    let scan = scan_branch_protection_posture(metadata, repos);

    push_posture_flag(
        flags,
        scan.absent,
        PostureFlagSpec {
            id: RedFlagId::BranchProtectionAbsent,
            severity: Severity::High,
            title: "Branch protection is absent",
            detail: "GitHub confirms no branch protection is configured on the default branch — this is a definitive absence, not a permission-limited read.",
            remedy_summary: "Configure branch protection on the default branch (required reviews, status checks, and push/deletion restrictions).",
        },
    );
    push_posture_flag(
        flags,
        scan.broad_bypass,
        PostureFlagSpec {
            id: RedFlagId::BroadBypassPresent,
            severity: Severity::High,
            title: "Branch protection has a broad bypass actor",
            detail: "A broad bypass actor, for example organization admins, can skip branch protection rules entirely on the default branch.",
            remedy_summary: "Remove or narrow the bypass actor so branch protection cannot be broadly circumvented.",
        },
    );
    push_posture_flag(
        flags,
        scan.admin_not_bound,
        PostureFlagSpec {
            id: RedFlagId::AdminEnforcementNotEquivalent,
            severity: Severity::Medium,
            title: "Administrators are exempt from branch protection",
            detail: "Branch protection does not apply to repository administrators on the default branch.",
            remedy_summary: "Enable enforcement for administrators on the default branch protection rule.",
        },
    );
    push_posture_flag(
        flags,
        scan.integrity_gap,
        PostureFlagSpec {
            id: RedFlagId::IntegrityControlGap,
            severity: Severity::Medium,
            title: "Default branch history is rewritable or deletable",
            detail: "Force pushes or branch deletion are not blocked on the default branch, allowing history rewrites or accidental or malicious deletion.",
            remedy_summary: "Enable force-push and deletion restrictions on the default branch protection rule.",
        },
    );
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
            let index = WIDTH_CLASSES
                .iter()
                .enumerate()
                .rfind(|(candidate, _)| {
                    f64::from(u32::try_from(*candidate).unwrap_or(u32::MAX)) <= bucket
                })
                .map_or(0, |(candidate, _)| candidate);
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
    use crate::domain::auth::{AuthMode, Capability, TokenTier};
    use crate::domain::checks::{
        BranchProtectionDetails, BranchProtectionResult, BranchProtectionStatus,
        CollectionFailureReason,
    };
    use crate::domain::metrics::{
        AggregatedMetrics, BranchProtectionCounts, CodeownersCounts, CollectionHealthCheckKind,
        CollectionHealthCount, DependabotCounts, PolicyCounts, RateMetric, SecretAlertCounts,
        SecretScanningCounts,
    };
    use crate::domain::repository::Visibility;
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
            collection_health_counts: vec![],
            team_rosters: vec![],
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
        assert_eq!(vm.run_id, evidence.assessment_metadata.run_id);
    }

    #[test]
    fn admin_diagnostics_groups_collection_health_counts_deterministically() {
        let mut evidence = sample_evidence();
        evidence.assessment_metadata.auth_mode = AuthMode::GitHubApp;
        evidence.assessment_metadata.token_tier = TokenTier::Limited;
        evidence.assessment_metadata.unavailable_capabilities = vec![
            Capability::PrivateBranchProtectionRead,
            Capability::OrgSecretScanningAlerts,
        ];
        evidence.metrics.collection_health_counts = vec![
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::Rulesets,
                reason: CollectionFailureReason::RateLimited,
                count: 4,
            },
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: CollectionFailureReason::Transient,
                count: 2,
            },
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: CollectionFailureReason::PermissionDenied,
                count: 3,
            },
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: CollectionFailureReason::PermissionSuspected,
                count: 1,
            },
        ];

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());
        let diagnostics = &vm.admin_diagnostics;

        assert!(diagnostics.has_technical_issues());
        assert_eq!(diagnostics.technical_issues_total, 10);
        assert_eq!(diagnostics.collection_health_sections.len(), 2);
        assert_eq!(
            diagnostics.collection_health_sections[0].check_kind,
            CollectionHealthCheckKind::BranchProtection
        );
        assert_eq!(diagnostics.collection_health_sections[0].subtotal, 6);
        assert_eq!(
            diagnostics.collection_health_sections[0]
                .rows
                .iter()
                .map(|row| (row.reason, row.count))
                .collect::<Vec<_>>(),
            vec![
                (CollectionFailureReason::PermissionDenied, 3),
                (CollectionFailureReason::PermissionSuspected, 1),
                (CollectionFailureReason::Transient, 2),
            ]
        );
        assert_eq!(
            diagnostics.collection_health_sections[1].check_kind,
            CollectionHealthCheckKind::Rulesets
        );
        assert_eq!(diagnostics.collection_health_sections[1].subtotal, 4);
        assert_eq!(diagnostics.credentials.auth_mode, AuthMode::GitHubApp);
        assert_eq!(diagnostics.credentials.auth_mode_label, "github_app");
        assert_eq!(diagnostics.credentials.token_tier, TokenTier::Limited);
        assert_eq!(diagnostics.credentials.token_tier_label, "Limited");
        assert!(diagnostics.credentials.has_degraded_capabilities());
        assert_eq!(
            diagnostics
                .credentials
                .unavailable_capabilities
                .iter()
                .map(|capability| capability.capability)
                .collect::<Vec<_>>(),
            vec![
                Capability::OrgSecretScanningAlerts,
                Capability::PrivateBranchProtectionRead,
            ]
        );
    }

    #[test]
    fn admin_diagnostics_zero_issues_has_neutral_badge_state() {
        let evidence = sample_evidence();
        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());
        let diagnostics = &vm.admin_diagnostics;

        assert!(!diagnostics.has_technical_issues());
        assert_eq!(diagnostics.technical_issues_total, 0);
        assert!(diagnostics.collection_health_sections.is_empty());
        assert!(!diagnostics.credentials.has_degraded_capabilities());
    }

    #[test]
    fn admin_diagnostics_wires_red_flags_from_evidence() {
        let mut metadata = neutral_metadata();
        metadata.token_tier = TokenTier::Unknown;
        let evidence = test_fixtures::make_full_evidence(
            metadata,
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            neutral_metrics(),
            test_fixtures::make_observability(),
            neutral_repos(),
        );

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert_eq!(vm.admin_diagnostics.red_flags.len(), 1);
        assert_eq!(
            vm.admin_diagnostics.red_flags[0].id,
            RedFlagId::TokenTierUnknown
        );
    }

    #[test]
    fn admin_diagnostics_red_flags_empty_when_nothing_fires() {
        let evidence = test_fixtures::make_full_evidence(
            neutral_metadata(),
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            neutral_metrics(),
            test_fixtures::make_observability(),
            neutral_repos(),
        );

        let vm = ReportViewModel::from_evidence(&evidence, &CoverageTiers::default());

        assert!(vm.admin_diagnostics.red_flags.is_empty());
    }

    fn flag_with_severity(severity: Severity) -> RedFlag {
        RedFlag {
            id: RedFlagId::TokenTierUnknown,
            severity,
            category: RedFlagCategory::Credential,
            title: "t".to_string(),
            detail: "d".to_string(),
            affected: AffectedScope::OrgWide,
            remedy: Remedy {
                summary: "s".to_string(),
                anchor: None,
                fix_target: FixTarget::Token,
            },
        }
    }

    fn diagnostics_with_flags(red_flags: Vec<RedFlag>) -> AdminDiagnosticsViewModel {
        AdminDiagnosticsViewModel {
            collection_health_sections: vec![],
            technical_issues_total: 0,
            credentials: CredentialLimitationsViewModel {
                auth_mode: AuthMode::GitHubApp,
                auth_mode_label: "github_app".to_string(),
                token_tier: TokenTier::Limited,
                token_tier_label: "Limited".to_string(),
                unavailable_capabilities: vec![],
            },
            red_flags,
        }
    }

    #[test]
    fn max_red_flag_severity_none_when_empty() {
        let diagnostics = diagnostics_with_flags(vec![]);
        assert_eq!(diagnostics.max_red_flag_severity(), None);
    }

    #[test]
    fn max_red_flag_severity_critical_wins_over_high_and_medium() {
        let diagnostics = diagnostics_with_flags(vec![
            flag_with_severity(Severity::Medium),
            flag_with_severity(Severity::Critical),
            flag_with_severity(Severity::High),
        ]);
        let severity = diagnostics.max_red_flag_severity();
        assert_eq!(severity, Some(Severity::Critical));
        assert_eq!(severity.unwrap().css_class(), "severity-critical");
    }

    #[test]
    fn max_red_flag_severity_only_medium_present() {
        let diagnostics = diagnostics_with_flags(vec![flag_with_severity(Severity::Medium)]);
        assert_eq!(diagnostics.max_red_flag_severity(), Some(Severity::Medium));
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

    fn neutral_metadata() -> AssessmentMetadata {
        let mut metadata = test_fixtures::make_metadata();
        metadata.auth_mode = AuthMode::GitHubApp;
        metadata.run_timestamp = Timestamp::now().to_string();
        metadata
    }

    fn neutral_metrics() -> AggregatedMetrics {
        test_fixtures::make_minimal_metrics()
    }

    fn neutral_repos() -> Vec<RepositoryEvidence> {
        vec![test_fixtures::all_passing_evidence("clean-repo")]
    }

    fn branch_protection_with(
        reason_kind: Option<CollectionFailureReason>,
        has_broad_bypass: Option<bool>,
        admin_equivalent: Option<bool>,
        force_push_blocked: Option<bool>,
        deletion_blocked: Option<bool>,
    ) -> BranchProtectionResult {
        BranchProtectionResult {
            status: BranchProtectionStatus::Partial,
            details: BranchProtectionDetails {
                default_branch: "main".to_string(),
                has_pr: Some(true),
                required_reviewers: Some(1),
                has_status_checks: Some(true),
                admin_equivalent,
                has_broad_bypass,
                reason: None,
                reason_kind,
                http_status: None,
                force_push_blocked,
                deletion_blocked,
            },
            timestamp: test_fixtures::make_timestamp(),
        }
    }

    fn repo_with_branch_protection(
        name: &str,
        branch: BranchProtectionResult,
    ) -> RepositoryEvidence {
        test_fixtures::make_repository_evidence(
            name,
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                branch,
                test_fixtures::codeowners_conforming(),
            ),
        )
    }

    #[test]
    fn build_red_flags_zero_flags_is_neutral() {
        let flags = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(flags.is_empty());
    }

    #[test]
    fn red_flag_token_tier_unknown_fires_and_absent() {
        let mut metadata = neutral_metadata();
        metadata.token_tier = TokenTier::Unknown;
        let fired = build_red_flags(&metadata, &neutral_metrics(), &neutral_repos());
        assert!(fired.iter().any(|f| f.id == RedFlagId::TokenTierUnknown));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(!absent.iter().any(|f| f.id == RedFlagId::TokenTierUnknown));
    }

    #[test]
    fn red_flag_degraded_capability_fires_and_absent() {
        let mut metadata = neutral_metadata();
        metadata.unavailable_capabilities = vec![
            Capability::PrivateBranchProtectionRead,
            Capability::OrgSecretScanningAlerts,
        ];
        let fired = build_red_flags(&metadata, &neutral_metrics(), &neutral_repos());
        let capability_flags: Vec<_> = fired
            .iter()
            .filter(|f| f.id == RedFlagId::DegradedCapability)
            .collect();
        assert_eq!(capability_flags.len(), 2);
        assert!(capability_flags[0].title.contains("Org-level"));
        assert!(capability_flags[1].title.contains("Private/internal"));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(!absent.iter().any(|f| f.id == RedFlagId::DegradedCapability));
    }

    #[test]
    fn red_flag_branch_protection_permission_suspected_fires_and_absent() {
        let mut metrics = neutral_metrics();
        metrics.collection_health_counts = vec![CollectionHealthCount {
            check_kind: CollectionHealthCheckKind::BranchProtection,
            reason: CollectionFailureReason::PermissionSuspected,
            count: 3,
        }];
        let fired = build_red_flags(&neutral_metadata(), &metrics, &neutral_repos());
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::BranchProtectionPermissionSuspected)
            .expect("flag should fire");
        assert_eq!(flag.affected, AffectedScope::Count(3));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::BranchProtectionPermissionSuspected)
        );
    }

    #[test]
    fn red_flag_secret_scanning_permission_denied_fires_and_absent() {
        let mut metrics = neutral_metrics();
        metrics.collection_health_counts = vec![CollectionHealthCount {
            check_kind: CollectionHealthCheckKind::SecretScanning,
            reason: CollectionFailureReason::PermissionDenied,
            count: 2,
        }];
        let fired = build_red_flags(&neutral_metadata(), &metrics, &neutral_repos());
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::SecretScanningPermissionDenied)
            .expect("flag should fire");
        assert_eq!(flag.affected, AffectedScope::Count(2));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::SecretScanningPermissionDenied)
        );
    }

    #[test]
    fn red_flag_auth_mode_pat_fires_and_absent() {
        let mut metadata = neutral_metadata();
        metadata.auth_mode = AuthMode::Pat;
        let fired = build_red_flags(&metadata, &neutral_metrics(), &neutral_repos());
        assert!(fired.iter().any(|f| f.id == RedFlagId::AuthModePat));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(!absent.iter().any(|f| f.id == RedFlagId::AuthModePat));
    }

    #[test]
    fn red_flag_rate_limited_collection_fires_and_absent() {
        let mut metrics = neutral_metrics();
        metrics.collection_health_counts = vec![CollectionHealthCount {
            check_kind: CollectionHealthCheckKind::Dependabot,
            reason: CollectionFailureReason::RateLimited,
            count: 4,
        }];
        let fired = build_red_flags(&neutral_metadata(), &metrics, &neutral_repos());
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::RateLimitedCollection)
            .expect("flag should fire");
        assert_eq!(flag.affected, AffectedScope::Count(4));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::RateLimitedCollection)
        );
    }

    #[test]
    fn red_flag_unstable_collection_result_fires_and_absent() {
        let mut metrics = neutral_metrics();
        metrics.collection_health_counts = vec![
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::Codeowners,
                reason: CollectionFailureReason::Invalid,
                count: 1,
            },
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::SecretScanning,
                reason: CollectionFailureReason::Transient,
                count: 2,
            },
        ];
        let fired = build_red_flags(&neutral_metadata(), &metrics, &neutral_repos());
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::UnstableCollectionResult)
            .expect("flag should fire");
        assert_eq!(flag.affected, AffectedScope::Count(3));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::UnstableCollectionResult)
        );
    }

    #[test]
    fn red_flag_codeowners_truncated_fires_and_absent() {
        let mut metrics = neutral_metrics();
        metrics.codeowners_counts.truncated = 5;
        let fired = build_red_flags(&neutral_metadata(), &metrics, &neutral_repos());
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::CodeownersTruncated)
            .expect("flag should fire");
        assert_eq!(flag.affected, AffectedScope::Count(5));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::CodeownersTruncated)
        );
    }

    #[test]
    fn red_flag_collection_run_stale_fires_and_absent() {
        let mut metadata = neutral_metadata();
        metadata.run_timestamp = "2000-01-01T00:00:00Z".to_string();
        let fired = build_red_flags(&metadata, &neutral_metrics(), &neutral_repos());
        assert!(fired.iter().any(|f| f.id == RedFlagId::CollectionRunStale));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(!absent.iter().any(|f| f.id == RedFlagId::CollectionRunStale));
    }

    #[test]
    fn red_flag_secret_scanning_unobservable_fires_and_absent() {
        let repos = vec![test_fixtures::make_repo_with_updated_at(
            "unobservable-repo",
            None,
            true,
            None,
            false,
            &[],
        )];
        let fired = build_red_flags(&neutral_metadata(), &neutral_metrics(), &repos);
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::SecretScanningUnobservable)
            .expect("flag should fire");
        assert_eq!(flag.affected, AffectedScope::Count(1));

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::SecretScanningUnobservable)
        );
    }

    #[test]
    fn red_flag_branch_protection_absent_fires_and_absent() {
        let repos = vec![repo_with_branch_protection(
            "absent-bp-repo",
            branch_protection_with(
                Some(CollectionFailureReason::NotFoundAbsent),
                Some(false),
                Some(true),
                Some(true),
                Some(true),
            ),
        )];
        let fired = build_red_flags(&neutral_metadata(), &neutral_metrics(), &repos);
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::BranchProtectionAbsent)
            .expect("flag should fire");
        assert_eq!(
            flag.affected,
            AffectedScope::Repos(vec!["TestOrg/absent-bp-repo".to_string()])
        );
        assert_eq!(
            flag.remedy.fix_target,
            FixTarget::Repo("TestOrg/absent-bp-repo".to_string())
        );

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::BranchProtectionAbsent)
        );
    }

    #[test]
    fn red_flag_broad_bypass_fires_and_absent() {
        let repos = vec![repo_with_branch_protection(
            "broad-bypass-repo",
            branch_protection_with(None, Some(true), Some(true), Some(true), Some(true)),
        )];
        let fired = build_red_flags(&neutral_metadata(), &neutral_metrics(), &repos);
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::BroadBypassPresent)
            .expect("flag should fire");
        assert_eq!(
            flag.affected,
            AffectedScope::Repos(vec!["TestOrg/broad-bypass-repo".to_string()])
        );

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(!absent.iter().any(|f| f.id == RedFlagId::BroadBypassPresent));
    }

    #[test]
    fn red_flag_admin_enforcement_not_equivalent_fires_and_absent() {
        let repos = vec![repo_with_branch_protection(
            "admin-gap-repo",
            branch_protection_with(None, Some(false), Some(false), Some(true), Some(true)),
        )];
        let fired = build_red_flags(&neutral_metadata(), &neutral_metrics(), &repos);
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::AdminEnforcementNotEquivalent)
            .expect("flag should fire");
        assert_eq!(
            flag.affected,
            AffectedScope::Repos(vec!["TestOrg/admin-gap-repo".to_string()])
        );

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::AdminEnforcementNotEquivalent)
        );
    }

    #[test]
    fn red_flag_integrity_control_gap_fires_and_absent() {
        let repos = vec![repo_with_branch_protection(
            "integrity-gap-repo",
            branch_protection_with(None, Some(false), Some(true), Some(false), Some(true)),
        )];
        let fired = build_red_flags(&neutral_metadata(), &neutral_metrics(), &repos);
        let flag = fired
            .iter()
            .find(|f| f.id == RedFlagId::IntegrityControlGap)
            .expect("flag should fire");
        assert_eq!(
            flag.affected,
            AffectedScope::Repos(vec!["TestOrg/integrity-gap-repo".to_string()])
        );

        let absent = build_red_flags(&neutral_metadata(), &neutral_metrics(), &neutral_repos());
        assert!(
            !absent
                .iter()
                .any(|f| f.id == RedFlagId::IntegrityControlGap)
        );
    }

    #[test]
    fn branch_protection_posture_flags_carry_operations_anchor() {
        let repos = vec![repo_with_branch_protection(
            "posture-anchor-repo",
            branch_protection_with(
                Some(CollectionFailureReason::NotFoundAbsent),
                Some(true),
                Some(false),
                Some(false),
                Some(false),
            ),
        )];
        let fired = build_red_flags(&neutral_metadata(), &neutral_metrics(), &repos);

        let posture_ids = [
            RedFlagId::BranchProtectionAbsent,
            RedFlagId::BroadBypassPresent,
            RedFlagId::AdminEnforcementNotEquivalent,
            RedFlagId::IntegrityControlGap,
        ];
        for id in posture_ids {
            let flag = fired
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("{id:?} should fire"));
            assert_eq!(flag.remedy.anchor, Some("branch-protection-coverage"));
        }
    }

    #[test]
    fn build_red_flags_sorted_by_severity_then_category_then_id() {
        let mut metadata = neutral_metadata();
        metadata.token_tier = TokenTier::Unknown;
        metadata.run_timestamp = "2000-01-01T00:00:00Z".to_string();

        let mut metrics = neutral_metrics();
        metrics.collection_health_counts = vec![CollectionHealthCount {
            check_kind: CollectionHealthCheckKind::BranchProtection,
            reason: CollectionFailureReason::PermissionSuspected,
            count: 2,
        }];

        let repos = vec![repo_with_branch_protection(
            "posture-repo",
            branch_protection_with(None, Some(false), Some(false), Some(true), Some(true)),
        )];

        let flags = build_red_flags(&metadata, &metrics, &repos);

        assert_eq!(
            flags.iter().map(|f| f.id).collect::<Vec<_>>(),
            vec![
                RedFlagId::TokenTierUnknown,
                RedFlagId::CollectionRunStale,
                RedFlagId::BranchProtectionPermissionSuspected,
                RedFlagId::AdminEnforcementNotEquivalent,
            ]
        );
    }
}
