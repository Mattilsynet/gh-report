//! HTML report rendering with Askama templates.
//!
//! Renders an [`Evidence`] artifact into a multi-page dashboard suitable
//! for internal publication. Askama auto-escapes all interpolated values,
//! preventing script injection in the published HTML.

use std::collections::HashMap;

use askama::Template;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use tracing::debug;

use crate::config;
use crate::config::dashboard::{CoverageTiers, DashboardConfig};
use crate::domain::checks::{
    BranchProtectionStatus, CodeownersStatus, DependabotStatus, ScoreCategory,
    SecretScanningStatus, SecurityPolicyStatus,
};
use crate::domain::evidence::{Evidence, RepositoryEvidence};
use crate::domain::metrics::{
    CollectionHealthCheckKind, OwnerType, ScoreExclusionCount, TeamRoster, TeamRosterStatus,
};
use crate::domain::time::{is_repo_stale, parse_iso8601};
use crate::error::ReportError;
use crate::report::view_model::{
    ControlCell, ControlColumn, CoverageTier, DeletedRepoRow, DeletedTeamRow, DeletedViewModel,
    OrphanedRepoRow, OrphanedTeamGroup, OrphanedViewModel, OwnerDetailViewModel, OwnerOverviewRow,
    OwnerRepoRow, OwnersViewModel, ReportViewModel, StatusDot, SummaryCard, TeamMemberRow,
    TeamRosterViewModel, TopNav, TopSecurityTeam, compute_health_score,
    coverage_control_column_tooltip, coverage_control_how_to_fix, format_exclusion, generate_slug,
    rate_to_width_class, strip_org_prefix,
};

/// Askama template for the security posture report.
///
/// Wraps [`ReportViewModel`] so the template accesses fields via `{{ vm.field }}`.
/// Askama auto-escapes all interpolated values (HTML mode is the default for
/// `.html` template files), so injected content like repository names or
/// organization names cannot produce script injection.
#[derive(Template)]
#[template(path = "report.html")]
struct ReportTemplate<'a> {
    vm: &'a ReportViewModel,
    nav: TopNav,
}

/// Askama template for the dashboard index page.
///
/// Shows headline scorecard metrics and links to detailed report pages.
#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate<'a> {
    vm: &'a ReportViewModel,
    nav: TopNav,
}

/// Askama template for the admin diagnostics page.
#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate<'a> {
    vm: &'a ReportViewModel,
    nav: TopNav,
}

/// Askama template for the owners overview page.
#[derive(Template)]
#[template(path = "owners.html")]
struct OwnersTemplate<'a> {
    vm: &'a OwnersViewModel,
    organization: String,
    total_repos: u32,
    nav: TopNav,
    /// When `true`, emits a `<meta http-equiv="refresh">` tag so the
    /// browser auto-reloads until fresh collection data replaces the
    /// warm-start cache.
    warm_start: bool,
}

/// Askama template for a single owner's detail page.
#[derive(Template)]
#[template(path = "owner_detail.html")]
struct OwnerDetailTemplate {
    vm: OwnerDetailViewModel,
    nav: TopNav,
    /// When `true`, emits a `<meta http-equiv="refresh">` tag so the
    /// browser auto-reloads until fresh collection data replaces the
    /// warm-start cache.
    warm_start: bool,
}

/// Askama template for the orphaned repositories page.
#[derive(Template)]
#[template(path = "orphans.html")]
struct OrphansTemplate {
    vm: OrphanedViewModel,
    nav: TopNav,
    /// When `true`, emits a `<meta http-equiv="refresh">` tag so the
    /// browser auto-reloads until fresh collection data replaces the
    /// warm-start cache.
    warm_start: bool,
}

/// Askama template for the deleted repositories page.
#[derive(Template)]
#[template(path = "deleted.html")]
struct DeletedTemplate {
    vm: DeletedViewModel,
    nav: TopNav,
    /// When `true`, emits a `<meta http-equiv="refresh">` tag.
    warm_start: bool,
}

/// Embedded CSS stylesheet, compiled into the binary at build time.
///
/// Published as `style.css` alongside the HTML pages so the server's
/// Content-Security-Policy can use `style-src 'self'` without `'unsafe-inline'`.
const STYLESHEET: &str = include_str!("../../templates/style.css");

/// Embedded WebSocket client script, compiled into the binary at build time.
///
/// Published as `ws.js` alongside the HTML pages. Provides auto-reconnect
/// and page-reload on server-pushed update events. Uses `script-src 'self'`
/// in CSP — no inline scripts needed.
const WS_CLIENT_JS: &str = include_str!("../../templates/ws.js");

/// Control names in canonical order for owner tables.
const CONTROL_NAMES: &[&str] = &[
    "security_policy",
    "secret_scanning",
    "dependabot_security_updates",
    "branch_protection",
];

/// All 6 security controls used for the per-owner Team Health score geometric mean.
///
/// Excludes `codeowners` — it is tautological at the per-owner level because
/// repos are associated with owners via CODEOWNERS parsing, so every owner's
/// codeowners rate is 100%.
///
/// Includes `non_stale` and `alert_free` — lifecycle-based metrics computed
/// by [`enrich_owner_metrics_with_lifecycle`] that measure repo freshness
/// and secret scanning cleanliness per owner.
///
/// [`enrich_owner_metrics_with_lifecycle`]: crate::aggregate::metrics::enrich_owner_metrics_with_lifecycle
const SEC_SCORE_CONTROLS: &[&str] = &[
    "security_policy",
    "secret_scanning",
    "dependabot_security_updates",
    "branch_protection",
    "non_stale",
    "alert_free",
];

/// Percent-encoding set for URL path segments.
///
/// Encodes characters that are unsafe in URL path segments per RFC 3986,
/// while leaving RFC 3986 unreserved characters (`- . _ ~`) and
/// sub-delimiters unencoded for readable URLs.
///
/// This is stricter than no encoding (defense-in-depth against tampered
/// evidence data) but avoids the cosmetic over-encoding of `NON_ALPHANUMERIC`
/// which would turn `my-repo` into `my%2Drepo`.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'/')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Percent-encode set for a URL query-string *value*. Extends
/// [`PATH_SEGMENT`] with the query sub-delimiters (`:`, `&`, `=`, `+`)
/// so a value like `team:foo bar` cannot alter query structure.
const QUERY_VALUE: &AsciiSet = &PATH_SEGMENT.add(b'&').add(b'=').add(b'+').add(b':');

/// Render an Askama template, mapping errors to [`ReportError`].
fn render_template<T: askama::Template>(tmpl: &T) -> Result<String, ReportError> {
    tmpl.render()
        .map_err(|e| ReportError::TemplateRenderFailed {
            reason: e.to_string(),
        })
}

/// Human-readable labels for control names.
fn control_display_name(key: &str) -> &'static str {
    match key {
        "security_policy" => "Security Policy",
        "secret_scanning" => "Secret Scanning",
        "dependabot_security_updates" => "Dependabot Status",
        "branch_protection" => "Branch Protection",
        "non_stale" => "Freshness",
        "alert_free" => "Alert-Free",
        _ => "Unknown",
    }
}

/// Render the complete multi-page dashboard from collected evidence.
///
/// Returns a map of page path → rendered content:
/// - `index.html` — Dashboard landing page with scorecard metrics.
/// - `report.html` — Detailed security posture report.
/// - `admin.html` — Read-only operator diagnostics.
/// - `style.css` — Shared stylesheet (external, enabling strict CSP).
/// - `orphans.html` — Repositories without identifiable code owners.
/// - `owners.html` — Owner coverage overview (if owner metrics available).
/// - `owners/{slug}.html` — Per-owner detail pages (if owner metrics available).
///
/// Convenience wrapper over [`render_dashboard_streaming`] that collects
/// every page into a `HashMap`. Test callers use this form; the production
/// cache-build path (`gh_report::app::collect::build_cached_pages`) calls
/// [`render_dashboard_streaming`] directly so peak memory holds ~one raw
/// page rather than the full page set.
///
/// # Errors
///
/// Returns [`ReportError::TemplateRenderFailed`] if any template rendering fails.
pub fn render_dashboard(
    evidence: &Evidence,
    config: &DashboardConfig,
) -> Result<HashMap<String, String>, ReportError> {
    let mut pages = HashMap::new();
    render_dashboard_streaming(evidence, config, |path, content| {
        pages.insert(path, content);
    })?;
    Ok(pages)
}

/// Render the complete multi-page dashboard, handing each rendered page to
/// `sink` as soon as it is produced instead of accumulating a full
/// `HashMap<String, String>`.
///
/// Same page set as [`render_dashboard`] (see its docs for the full list).
/// Callers that immediately convert each page into a smaller owned form
/// (e.g. a compressed [`cherry_pit_web::serve::CachedPage`]) and drop the
/// raw `String` keep peak memory at ~one raw page plus the accumulating
/// output, instead of the whole rendered set (mem-opt-cachedpage-2026-07-11,
/// Option 1).
///
/// # Errors
///
/// Returns [`ReportError::TemplateRenderFailed`] if any template rendering fails.
pub fn render_dashboard_streaming(
    evidence: &Evidence,
    config: &DashboardConfig,
    mut sink: impl FnMut(String, String),
) -> Result<(), ReportError> {
    debug!(org = %evidence.assessment_metadata.organization, "rendering dashboard pages");
    let tiers = &config.tiers;
    let warm_start = evidence.assessment_metadata.warm_start;

    let owners_vm = build_owners_view_model(&evidence.metrics.owner_metrics, tiers);

    let orphaned_vm = build_orphaned_view_model(
        &evidence.repositories,
        &evidence.assessment_metadata.organization,
        &evidence.assessment_metadata.run_timestamp,
        &evidence.metrics.team_rosters,
    );
    let orphaned_count = orphaned_vm.orphaned_count;
    let deleted_vm = build_deleted_view_model(
        &evidence.deleted,
        &evidence.assessment_metadata.organization,
        &evidence.repositories,
        &evidence.metrics.team_rosters,
    );

    let mut vm = ReportViewModel::from_evidence(evidence, tiers);
    vm.owners.clone_from(&owners_vm);
    vm.orphaned_count = orphaned_count;
    (vm.team_access_guidance, vm.team_access_help_links) =
        crate::report::view_model::compose_team_access_guidance(&config.org_help.team_access);

    if let Some(ref ov) = owners_vm {
        vm.top_security_teams = build_top_security_teams(ov);
    }

    let nav = TopNav {
        base: "",
        show_owners: owners_vm.is_some(),
        orphaned_count,
        deleted_count: vm.deleted_count,
        technical_issues_total: vm.admin_diagnostics.technical_issues_total,
    };

    let report = render_template(&ReportTemplate { vm: &vm, nav })?;
    let index = render_template(&IndexTemplate { vm: &vm, nav })?;
    let admin = render_template(&AdminTemplate { vm: &vm, nav })?;

    sink("report.html".to_string(), report);
    sink("index.html".to_string(), index);
    sink("admin.html".to_string(), admin);
    sink("style.css".to_string(), STYLESHEET.to_string());
    sink("ws.js".to_string(), WS_CLIENT_JS.to_string());
    sink("gh-report-web-client.js".to_string(), String::new());
    sink("gh-report-web-client_bg.wasm".to_string(), String::new());
    sink("sort-init.js".to_string(), String::new());

    if let Some(ref owners) = owners_vm {
        let owners_html = render_template(&OwnersTemplate {
            vm: owners,
            organization: evidence.assessment_metadata.organization.clone(),
            total_repos: evidence.collection_statistics.total_repos,
            nav,
            warm_start,
        })?;
        sink("owners.html".to_string(), owners_html);

        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            tiers,
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &evidence.metrics.team_rosters,
            &orphaned_vm.by_team,
        );
        let nested_nav = TopNav { base: "../", ..nav };
        for (slug, detail_vm) in &detail_vms {
            let detail_html = render_template(&OwnerDetailTemplate {
                vm: detail_vm.clone(),
                nav: nested_nav,
                warm_start,
            })?;
            sink(format!("owners/{slug}.html"), detail_html);
        }
    }

    let orphaned_html = render_template(&OrphansTemplate {
        vm: orphaned_vm,
        nav,
        warm_start,
    })?;
    sink("orphans.html".to_string(), orphaned_html);

    let deleted_html = render_template(&DeletedTemplate {
        vm: deleted_vm,
        nav,
        warm_start,
    })?;
    sink("deleted.html".to_string(), deleted_html);

    Ok(())
}

/// Build the top-3 security team podium from owner overview data.
///
/// Only team-type owners are eligible (individual users are excluded).
/// Podium order: `[Silver, Gold, Bronze]` — Gold is centered for visual
/// emphasis, matching a medal-ceremony layout.
fn build_top_security_teams(owners: &OwnersViewModel) -> Vec<TopSecurityTeam> {
    let mut ranked: Vec<&OwnerOverviewRow> = owners
        .rows
        .iter()
        .filter(|r| r.sec_score.is_some() && r.owner_type == OwnerType::Team)
        .collect();
    ranked.sort_by(|a, b| {
        b.sec_score
            .unwrap_or(0.0)
            .partial_cmp(&a.sec_score.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let top3: Vec<&OwnerOverviewRow> = ranked.into_iter().take(3).collect();

    let podium_order: Vec<(usize, &str, &str)> = match top3.len() {
        0 => vec![],
        1 => vec![(0, "rank-gold", "\u{1f947}")],
        2 => vec![
            (1, "rank-silver", "\u{1f948}"),
            (0, "rank-gold", "\u{1f947}"),
        ],
        _ => vec![
            (1, "rank-silver", "\u{1f948}"),
            (0, "rank-gold", "\u{1f947}"),
            (2, "rank-bronze", "\u{1f949}"),
        ],
    };

    podium_order
        .into_iter()
        .map(|(idx, rank_class, rank_emoji)| {
            let r = top3[idx];
            TopSecurityTeam {
                owner: r.owner.clone(),
                owner_short: r.owner_short.clone(),
                sec_score_formatted: r.sec_score_formatted.clone(),
                slug: r.slug.clone(),
                rank_class,
                rank_emoji,
                sec_score_tier: r.sec_score_tier,
                sec_score_width_class: r.sec_score_width_class,
            }
        })
        .collect()
}

/// Build the owners overview view model from per-owner metrics.
///
/// Returns `None` if no owner metrics are available.
fn build_owners_view_model(
    owner_metrics: &[crate::domain::metrics::OwnerMetrics],
    tiers: &CoverageTiers,
) -> Option<OwnersViewModel> {
    if owner_metrics.is_empty() {
        return None;
    }

    let owners: Vec<String> = owner_metrics
        .iter()
        .map(|m| m.display_name.clone())
        .collect();
    let slugs = crate::report::view_model::generate_unique_slugs(&owners);

    let control_columns: Vec<ControlColumn> = CONTROL_NAMES
        .iter()
        .map(|&k| ControlColumn {
            name: control_display_name(k),
            tooltip: coverage_control_column_tooltip(k).unwrap_or_default(),
        })
        .collect();

    let rows: Vec<OwnerOverviewRow> = owner_metrics
        .iter()
        .map(|m| {
            let slug = slugs
                .get(&m.display_name)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            let controls: Vec<ControlCell> = CONTROL_NAMES
                .iter()
                .map(|&key| {
                    build_control_cell(
                        &m.per_control_coverage,
                        &m.score_exclusion_counts,
                        key,
                        tiers,
                    )
                })
                .collect();

            let sec_rates: Vec<Option<f64>> = SEC_SCORE_CONTROLS
                .iter()
                .map(|&key| m.per_control_coverage.get(key).and_then(|rm| rm.rate))
                .collect();
            let sec_score = compute_health_score(&sec_rates);
            let sec_score_formatted = match sec_score {
                Some(s) => format!("{s:.1}%"),
                None => "N/A".to_string(),
            };
            let sec_score_tier = CoverageTier::from_rate(sec_score, tiers);
            let sec_score_width_class = rate_to_width_class(sec_score);

            OwnerOverviewRow {
                owner: m.display_name.clone(),
                owner_short: strip_org_prefix(&m.display_name),
                slug,
                owner_type: m.owner_type,
                repo_count: m.total_repos,
                controls,
                sec_score,
                sec_score_formatted,
                sec_score_tier,
                sec_score_width_class,
            }
        })
        .collect();

    Some(OwnersViewModel {
        rows,
        control_columns,
    })
}

/// Map an owner per-control coverage key to its `CollectionHealthCheckKind`
/// axis, for looking up that control's by-reason exclusion breakdown in
/// [`crate::domain::metrics::OwnerMetrics::score_exclusion_counts`]
/// (item6-03, bd bead `adr-fmt-orvyn`). `None` for keys with no
/// `ScoreExclusionCount` axis (the owner-only lifecycle controls `non_stale`
/// and `alert_free`, item6-04's D4 relabel).
fn control_key_to_check_kind(key: &str) -> Option<CollectionHealthCheckKind> {
    match key {
        "security_policy" => Some(CollectionHealthCheckKind::SecurityPolicy),
        "secret_scanning" => Some(CollectionHealthCheckKind::SecretScanning),
        "dependabot_security_updates" => Some(CollectionHealthCheckKind::Dependabot),
        "branch_protection" => Some(CollectionHealthCheckKind::BranchProtection),
        "codeowners" => Some(CollectionHealthCheckKind::Codeowners),
        _ => None,
    }
}

/// Build a [`ControlCell`] from an owner's per-control coverage map.
///
/// Shared by [`build_owners_view_model`] (overview table) and
/// [`build_owner_detail_view_models`] (summary cards). `score_exclusion_counts`
/// is the owner's by-reason exclusion breakdown (item6-03); controls with no
/// `ScoreExclusionCount` axis (see [`control_key_to_check_kind`]) get `0`/`"0
/// unmeasured"`.
fn build_control_cell(
    per_control_coverage: &std::collections::HashMap<String, crate::domain::metrics::RateMetric>,
    score_exclusion_counts: &[ScoreExclusionCount],
    key: &str,
    tiers: &CoverageTiers,
) -> ControlCell {
    let rate_metric = per_control_coverage.get(key);
    let rate = rate_metric.and_then(|rm| rm.rate);
    let formatted = rate_metric.map_or_else(|| "N/A".to_string(), ToString::to_string);
    let exclusion = control_key_to_check_kind(key)
        .map(|check_kind| format_exclusion(check_kind, score_exclusion_counts));
    let (excluded_total, excluded_formatted) = match exclusion {
        Some(e) => (e.total, e.formatted),
        None => (0, "0 unmeasured".to_string()),
    };
    ControlCell {
        rate_formatted: formatted,
        tier: CoverageTier::from_rate(rate, tiers),
        width_class: rate_to_width_class(rate),
        excluded_total,
        excluded_formatted,
    }
}

/// Build a [`TeamRosterViewModel`] from a fetched [`TeamRoster`] (B1).
fn build_team_roster_view_model(roster: &TeamRoster) -> TeamRosterViewModel {
    let mut members: Vec<TeamMemberRow> = roster
        .members
        .iter()
        .map(|member| TeamMemberRow {
            login: member.login.clone(),
            role_label: member.role,
            profile_url: format!(
                "{}/{}",
                config::DEFAULT_GITHUB_WEB_BASE_URL,
                utf8_percent_encode(&member.login, PATH_SEGMENT)
            ),
            in_org: member.in_org,
        })
        .collect();
    members.sort_by_cached_key(|m| m.login.to_lowercase());
    let member_count = u32::try_from(members.len()).unwrap_or(u32::MAX);

    let (is_complete, status_label) = match roster.status {
        TeamRosterStatus::Complete => (true, "Complete"),
        TeamRosterStatus::Deleted => (false, "Deleted"),
        TeamRosterStatus::PermissionDenied => (false, "Permission denied"),
        TeamRosterStatus::TransientError => (false, "Temporarily unavailable"),
    };

    TeamRosterViewModel {
        is_complete,
        status_label,
        members,
        member_count,
    }
}

/// Build the GitHub-hosted URL for an owner: an org team page for
/// team-type owners, a user profile page for user-type owners (UF2-3).
///
/// Built from `DEFAULT_GITHUB_WEB_BASE_URL` (already organization-agnostic —
/// every GitHub org lives under the same host) plus the already-generic
/// `org_encoded` and the owner's own canonical slug/login, so no
/// organization-specific literal is introduced (UF2-GEN).
///
/// Returns `None` only when `canonical_owner` is malformed: a team string
/// with no extractable slug, or a user string that is just `"@"`.
fn build_owner_github_url(
    owner_type: OwnerType,
    canonical_owner: &str,
    org_encoded: &str,
) -> Option<String> {
    match owner_type {
        OwnerType::Team => {
            let slug = crate::domain::metrics::team_slug_from_canonical_owner(canonical_owner)?;
            Some(format!(
                "{}/orgs/{}/teams/{}",
                config::DEFAULT_GITHUB_WEB_BASE_URL,
                org_encoded,
                utf8_percent_encode(slug, PATH_SEGMENT)
            ))
        }
        OwnerType::User => {
            let login = canonical_owner.strip_prefix('@')?;
            (!login.is_empty()).then(|| {
                format!(
                    "{}/{}",
                    config::DEFAULT_GITHUB_WEB_BASE_URL,
                    utf8_percent_encode(login, PATH_SEGMENT)
                )
            })
        }
    }
}

/// Build the GitHub security-overview deep-link for a team, filtered to
/// that team's non-archived repositories.
///
/// Only teams have a security-overview scope; user-type owners return
/// `None`. The query mirrors GitHub's own security-overview filter
/// syntax: `archived:false tool:github team:<slug>`.
fn build_team_security_url(
    owner_type: OwnerType,
    canonical_owner: &str,
    org_encoded: &str,
) -> Option<String> {
    let OwnerType::Team = owner_type else {
        return None;
    };
    let slug = crate::domain::metrics::team_slug_from_canonical_owner(canonical_owner)?;
    let query = format!("archived:false tool:github team:{slug}");
    Some(format!(
        "{}/orgs/{}/security/overview?query={}",
        config::DEFAULT_GITHUB_WEB_BASE_URL,
        org_encoded,
        utf8_percent_encode(&query, QUERY_VALUE)
    ))
}

/// Build per-owner detail view models with per-repo status rows.
///
/// Accepts a pre-computed `owner_repo_map` (built via
/// [`build_owner_repo_map`](crate::domain::metrics::build_owner_repo_map))
/// to avoid recomputing the owner→repo mapping that was already constructed
/// during metrics aggregation.
///
/// `run_timestamp` is the ISO 8601 assessment timestamp, used to compute
/// staleness (repos with `updated_at` > 2 years before this value).
///
/// `team_rosters` supplies the B1 member roster for team-type owners,
/// matched by canonical owner name; `None` for user-type owners or teams
/// B1 has not (yet) fetched a roster for.
///
/// `orphaned_by_team` supplies each team's orphan-repo rows (B2), joined
/// by canonical owner name (`group.team == owner_metrics[..].owner`), for
/// the collapsible orphan section on that team's own detail page (item 7).
///
/// Returns a list of (slug, detail view model) pairs.
fn build_owner_detail_view_models(
    owner_metrics: &[crate::domain::metrics::OwnerMetrics],
    owner_repo_map: &HashMap<String, (String, Vec<&RepositoryEvidence>)>,
    tiers: &CoverageTiers,
    organization: &str,
    run_timestamp: &str,
    team_rosters: &[TeamRoster],
    orphaned_by_team: &[OrphanedTeamGroup],
) -> Vec<(String, OwnerDetailViewModel)> {
    if owner_metrics.is_empty() {
        return Vec::new();
    }

    let owners: Vec<String> = owner_metrics
        .iter()
        .map(|m| m.display_name.clone())
        .collect();
    let slugs = crate::report::view_model::generate_unique_slugs(&owners);

    let control_columns: Vec<ControlColumn> = CONTROL_NAMES
        .iter()
        .map(|&k| ControlColumn {
            name: control_display_name(k),
            tooltip: coverage_control_column_tooltip(k).unwrap_or_default(),
        })
        .collect();

    owner_metrics
        .iter()
        .filter_map(|m| {
            let slug = slugs.get(&m.display_name)?.clone();

            let summary_cards: Vec<SummaryCard> = CONTROL_NAMES
                .iter()
                .map(|&key| SummaryCard {
                    key,
                    label: control_display_name(key).to_string(),
                    cell: build_control_cell(
                        &m.per_control_coverage,
                        &m.score_exclusion_counts,
                        key,
                        tiers,
                    ),
                    how_to_fix: coverage_control_how_to_fix(key).unwrap_or_default(),
                })
                .collect();

            let canonical_key = m.owner.clone();
            let org_encoded = utf8_percent_encode(organization, PATH_SEGMENT).to_string();
            let github_url = build_owner_github_url(m.owner_type, &m.owner, &org_encoded);
            let security_url = build_team_security_url(m.owner_type, &m.owner, &org_encoded);
            let mut repo_rows: Vec<OwnerRepoRow> = owner_repo_map
                .get(&canonical_key)
                .map(|(_, repos)| {
                    repos
                        .iter()
                        .map(|repo| build_owner_repo_row(repo, &org_encoded, run_timestamp, tiers))
                        .collect()
                })
                .unwrap_or_default();

            repo_rows.sort_by_cached_key(|r| r.repo_name.to_lowercase());

            let owner_type_label = m.owner_type.to_string();

            let has_stale_repos = repo_rows.iter().any(|r| r.is_stale);
            let stale_repo_count =
                u32::try_from(repo_rows.iter().filter(|r| r.is_stale).count()).unwrap_or(u32::MAX);
            let total_repo_count = u32::try_from(repo_rows.len()).unwrap_or(u32::MAX);

            let stale_pct = if total_repo_count == 0 {
                None
            } else {
                Some((f64::from(stale_repo_count) / f64::from(total_repo_count)) * 100.0)
            };
            let stale_width_class = rate_to_width_class(stale_pct);

            let roster = team_rosters
                .iter()
                .find(|r| r.canonical_owner == m.owner)
                .map(build_team_roster_view_model);

            let orphan_repo_rows: Vec<OrphanedRepoRow> = orphaned_by_team
                .iter()
                .find(|group| group.team == m.owner)
                .map(|group| group.rows.clone())
                .unwrap_or_default();

            let detail = OwnerDetailViewModel {
                owner: m.display_name.clone(),
                owner_short: strip_org_prefix(&m.display_name),
                owner_type_label,
                breadcrumb_label: m.display_name.clone(),
                repo_rows,
                control_columns: control_columns.clone(),
                summary_cards,
                has_stale_repos,
                stale_repo_count,
                total_repo_count,
                stale_width_class,
                roster,
                github_url,
                security_url,
                orphan_repo_rows,
                owner_in_org: m.in_org,
            };

            Some((slug, detail))
        })
        .collect()
}

/// Extracted last-commit metadata from a [`RepositoryEvidence`].
///
/// Used by both the owner-detail and orphaned-repos pages to avoid
/// duplicating the extraction logic.
struct LastCommitDisplay {
    login: String,
    url: String,
    date: String,
    /// `true` when a committer name is present but GitHub could not match
    /// the commit to any GitHub account (item9 Part A) — see
    /// [`extract_last_commit_display`] for the exact predicate.
    unregistered: bool,
}

const EM_DASH: &str = "\u{2014}";

/// Extract last-commit display fields from a [`RepositoryEvidence`].
///
/// `unregistered` distinguishes two states that both render an empty
/// `url` (item9 Part A): a committer name is present but
/// `committer_login` is `None` (GitHub could not match the commit to a
/// GitHub account — `unregistered: true`), versus no commit data at all
/// (`last_commit: None`, `login` falls back to [`EM_DASH`] —
/// `unregistered: false`, neutral dash). Keyed on `url.is_empty() &&
/// login != EM_DASH` so the two states never conflate.
fn extract_last_commit_display(repo: &RepositoryEvidence) -> LastCommitDisplay {
    match &repo.last_commit {
        Some(info) => {
            let login = info
                .committer_name
                .as_deref()
                .or(info.committer_login.as_deref())
                .unwrap_or(EM_DASH)
                .to_string();
            let url = info
                .committer_login
                .as_ref()
                .map(|l| {
                    let encoded = utf8_percent_encode(l, PATH_SEGMENT);
                    format!("{}/{}", config::DEFAULT_GITHUB_WEB_BASE_URL, encoded)
                })
                .unwrap_or_default();
            let date = format_date_prefix(info.commit_date.as_deref());
            let unregistered = url.is_empty() && login != EM_DASH;
            LastCommitDisplay {
                login,
                url,
                date,
                unregistered,
            }
        }
        None => LastCommitDisplay {
            login: EM_DASH.to_string(),
            url: String::new(),
            date: EM_DASH.to_string(),
            unregistered: false,
        },
    }
}

/// Build display name and URL for a repository.
fn build_repo_display(
    repo: &RepositoryEvidence,
    org_encoded: &str,
    name_encoded: &percent_encoding::PercentEncode<'_>,
) -> (String, String) {
    let name = repo.repository.name.clone();
    let url = format!(
        "{}/{}/{}",
        config::DEFAULT_GITHUB_WEB_BASE_URL,
        org_encoded,
        name_encoded,
    );
    (name, url)
}

/// Build a single [`OwnerRepoRow`] from repository evidence.
///
/// Extracts last-commit metadata and assembles the full row used in
/// per-owner detail pages.
fn build_owner_repo_row(
    repo: &RepositoryEvidence,
    org_encoded: &str,
    run_timestamp: &str,
    tiers: &CoverageTiers,
) -> OwnerRepoRow {
    let name_encoded = utf8_percent_encode(&repo.repository.name, PATH_SEGMENT);
    let commit = extract_last_commit_display(repo);
    let (repo_name, repo_url) = build_repo_display(repo, org_encoded, &name_encoded);

    let (repo_score_val, repo_score_fmt, repo_score_tier, repo_score_width_class) =
        compute_repo_score(&repo.checks, tiers);

    OwnerRepoRow {
        repo_name,
        repo_url,
        visibility: repo.repository.visibility.to_string(),
        controls: build_status_dots(&repo.checks),

        description: repo
            .repository
            .description
            .as_deref()
            .unwrap_or(EM_DASH)
            .to_string(),
        language: repo
            .repository
            .language
            .as_deref()
            .unwrap_or(EM_DASH)
            .to_string(),
        is_fork: repo.repository.fork,
        is_empty: repo.repository.is_empty,
        license: repo
            .repository
            .license_spdx
            .as_deref()
            .unwrap_or(EM_DASH)
            .to_string(),
        pushed_at: format_date_prefix(repo.repository.pushed_at.as_deref()),
        created_at: format_date_prefix(repo.repository.created_at.as_deref()),
        last_committer_login: commit.login,
        last_committer_url: commit.url,
        last_committer_unregistered: commit.unregistered,
        last_commit_date: commit.date,
        is_stale: is_repo_stale(repo.repository.updated_at.as_deref(), run_timestamp),
        repo_score: repo_score_val,
        repo_score_formatted: repo_score_fmt,
        repo_score_tier,
        repo_score_width_class,
    }
}

/// Compute a per-repo score from check results.
///
/// Counts each control as pass (1) or fail (0). Controls with unknown,
/// indeterminate, `NotApplicable`, or `PermissionDenied` status are excluded
/// from both numerator and denominator.
///
/// Returns `(score: Option<f64>, formatted: String, tier, width_class)`.
fn compute_repo_score(
    checks: &crate::domain::checks::RepositoryChecks,
    tiers: &CoverageTiers,
) -> (Option<f64>, String, CoverageTier, &'static str) {
    let categories = [
        ScoreCategory::from(checks.security_policy.status),
        ScoreCategory::from(checks.secret_scanning.status),
        ScoreCategory::from(checks.dependabot_security_updates.status),
        checks.branch_protection.score_category(),
        ScoreCategory::from(checks.codeowners.status),
    ];

    let mut pass = 0u32;
    let mut total = 0u32;
    for cat in &categories {
        match cat {
            ScoreCategory::Pass => {
                pass += 1;
                total += 1;
            }
            ScoreCategory::Fail => {
                total += 1;
            }
            ScoreCategory::Excluded(_) => {}
        }
    }

    if total == 0 {
        return (None, "N/A".to_string(), CoverageTier::Na, "w-0");
    }

    let score = (f64::from(pass) / f64::from(total)) * 100.0;
    let score_rounded = (score * 10.0).round() / 10.0;
    let formatted = format!("{score_rounded:.1}%");
    let tier = CoverageTier::from_rate(Some(score_rounded), tiers);
    let width_class = rate_to_width_class(Some(score_rounded));

    (Some(score_rounded), formatted, tier, width_class)
}

/// Determine whether a repository is orphaned.
///
/// Orphan predicate:
/// - `CodeownersStatus::Absent` → orphaned (no CODEOWNERS file at all).
/// - `parsed.is_some()` AND `unique_owners.is_empty()` → orphaned
///   (file exists but contains no `@`-prefixed owners).
/// - `CodeownersStatus::Unknown` → NOT orphaned (can't determine).
/// - `CodeownersStatus::Conforming` or `NonConforming` with `parsed: None`
///   (file found but not downloaded) → NOT orphaned.
fn is_orphaned(repo: &RepositoryEvidence) -> bool {
    let codeowners = &repo.checks.codeowners;
    match codeowners.status {
        CodeownersStatus::Absent => true,
        CodeownersStatus::Unknown => false,
        CodeownersStatus::Conforming | CodeownersStatus::NonConforming => codeowners
            .parsed
            .as_ref()
            .is_some_and(|p| p.unique_owners.is_empty()),
    }
}

/// Find the roster whose members list `committer_login` (case-insensitive).
///
/// B2: this is the sole join between team membership (B1, render-time
/// fetch) and orphan attribution — no persisted surface is involved.
fn attributed_roster<'a>(
    committer_login: Option<&str>,
    team_rosters: &'a [TeamRoster],
) -> Option<&'a TeamRoster> {
    let login = committer_login?;
    team_rosters.iter().find(|roster| {
        roster
            .members
            .iter()
            .any(|m| m.login.eq_ignore_ascii_case(login))
    })
}

/// Build the orphaned repositories view model.
///
/// Filters repos by the orphan predicate, builds display rows, and sorts
/// by last committer login (ascending, case-insensitive) then repo name
/// (ascending, case-insensitive).
fn build_orphaned_view_model(
    repositories: &[RepositoryEvidence],
    organization: &str,
    run_timestamp: &str,
    team_rosters: &[TeamRoster],
) -> OrphanedViewModel {
    let org_encoded = utf8_percent_encode(organization, PATH_SEGMENT).to_string();

    let mut rows: Vec<OrphanedRepoRow> = repositories
        .iter()
        .filter(|r| !r.repository.archived && is_orphaned(r))
        .map(|repo| {
            let name_encoded = utf8_percent_encode(&repo.repository.name, PATH_SEGMENT);
            let commit = extract_last_commit_display(repo);
            let (repo_name, repo_url) = build_repo_display(repo, &org_encoded, &name_encoded);
            let raw_committer_login = repo
                .last_commit
                .as_ref()
                .and_then(|info| info.committer_login.as_deref());
            let attributed = attributed_roster(raw_committer_login, team_rosters);

            OrphanedRepoRow {
                repo_name,
                repo_url,
                visibility: repo.repository.visibility.to_string(),
                description: repo
                    .repository
                    .description
                    .as_deref()
                    .unwrap_or(EM_DASH)
                    .to_string(),
                language: repo
                    .repository
                    .language
                    .as_deref()
                    .unwrap_or(EM_DASH)
                    .to_string(),
                is_empty: repo.repository.is_empty,
                last_committer_login: commit.login,
                last_committer_url: commit.url,
                last_committer_unregistered: commit.unregistered,
                last_commit_date: commit.date,
                is_stale: is_repo_stale(repo.repository.updated_at.as_deref(), run_timestamp),
                attributed_team: attributed.map(|r| r.canonical_owner.clone()),
                attributed_team_slug: attributed.map(|r| generate_slug(&r.canonical_owner)),
            }
        })
        .collect();

    rows.sort_by_cached_key(|r| {
        (
            r.last_committer_login.to_lowercase(),
            r.repo_name.to_lowercase(),
        )
    });

    let has_stale_repos = rows.iter().any(|r| r.is_stale);
    let orphaned_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let by_team = build_orphaned_by_team(&rows);

    OrphanedViewModel {
        rows,
        organization: organization.to_string(),
        orphaned_count,
        has_stale_repos,
        by_team,
    }
}

/// Group orphan rows by attributed team (B2), sorted by team name.
///
/// Only rows with an [`OrphanedRepoRow::attributed_team`] match contribute;
/// each group's rows are sorted by repo name.
fn build_orphaned_by_team(rows: &[OrphanedRepoRow]) -> Vec<OrphanedTeamGroup> {
    let mut by_team: HashMap<String, Vec<OrphanedRepoRow>> = HashMap::new();
    for row in rows {
        if let Some(team) = &row.attributed_team {
            by_team.entry(team.clone()).or_default().push(row.clone());
        }
    }

    let mut groups: Vec<OrphanedTeamGroup> = by_team
        .into_iter()
        .map(|(team, mut team_rows)| {
            team_rows.sort_by_cached_key(|r| r.repo_name.to_lowercase());
            let slug = team_rows
                .first()
                .and_then(|r| r.attributed_team_slug.clone())
                .unwrap_or_default();
            OrphanedTeamGroup {
                team_short: strip_org_prefix(&team),
                team,
                slug,
                rows: team_rows,
            }
        })
        .collect();
    groups.sort_by(|a, b| a.team.cmp(&b.team));
    groups
}

/// Build the deleted-repositories-and-teams page view model.
///
/// `rows` (deleted repos) comes from the persisted, event-sourced
/// [`crate::projection::DeletedRepoRecord`] set. `deleted_teams` is the
/// opposite: render-time-only (oracle adr-fmt-kqavx), rebuilt fresh every
/// call from `team_rosters` and `repositories` — never persisted. A
/// CODEOWNERS-referenced team whose roster fetch classified `Deleted` (404)
/// is joined to its referencing repos via
/// [`crate::domain::metrics::build_owner_repo_map`], keyed by the team's
/// full lowercased canonical owner (`@org/slug`), not its bare GitHub API
/// slug — the two are different strings and only the canonical form is a
/// valid map key.
fn build_deleted_view_model(
    deleted: &[crate::projection::DeletedRepoRecord],
    organization: &str,
    repositories: &[RepositoryEvidence],
    team_rosters: &[TeamRoster],
) -> DeletedViewModel {
    let mut rows: Vec<DeletedRepoRow> = deleted
        .iter()
        .map(|record| DeletedRepoRow {
            repo_name: record.repo_name.clone(),
            detected_at: record.detected_at.clone(),
        })
        .collect();
    rows.sort_by_cached_key(|row| row.repo_name.to_lowercase());
    let deleted_count = u32::try_from(rows.len()).unwrap_or(u32::MAX);

    let org_encoded = utf8_percent_encode(organization, PATH_SEGMENT).to_string();
    let owner_repo_map = crate::domain::metrics::build_owner_repo_map(repositories);
    let mut deleted_teams: Vec<DeletedTeamRow> = team_rosters
        .iter()
        .filter(|roster| roster.status == TeamRosterStatus::Deleted)
        .map(|roster| {
            let mut referencing_repos: Vec<String> = owner_repo_map
                .get(&roster.canonical_owner.to_lowercase())
                .map(|(_, repos)| repos.iter().map(|r| r.repository.name.clone()).collect())
                .unwrap_or_default();
            referencing_repos.sort_by_cached_key(|name| name.to_lowercase());
            let team_url =
                build_owner_github_url(OwnerType::Team, &roster.canonical_owner, &org_encoded)
                    .unwrap_or_default();
            DeletedTeamRow {
                team_slug: roster.team_slug.clone(),
                team_url,
                referencing_repos,
            }
        })
        .collect();
    deleted_teams.sort_by_cached_key(|row| row.team_slug.to_lowercase());

    DeletedViewModel {
        rows,
        organization: organization.to_string(),
        deleted_count,
        deleted_teams,
    }
}

/// Extract the `YYYY-MM-DD` date from an optional ISO 8601 timestamp,
/// converting to UTC first so that date-boundary crossings are correct.
///
/// Falls back to a naive `YYYY-MM-DD` string slice when the timestamp
/// cannot be parsed (e.g. date-only strings like `"2026-04-09"`).
/// Returns `"—"` when the value is `None` or too short.
fn format_date_prefix(iso_ts: Option<&str>) -> String {
    if let Some(dt) = iso_ts.and_then(parse_iso8601) {
        return dt.strftime("%Y-%m-%d").to_string();
    }
    if let Some(d) = iso_ts.and_then(|s| s.get(..10)) {
        return d.to_string();
    }
    "\u{2014}".to_string()
}

/// Detect whether a repository's checks represent a "pending" partial
/// report entry (budget pause, not yet evaluated).
///
/// Checks `secret_scanning.reason` as the canonical indicator — all
/// checks produced by `failure_evidence_with_reason(_, _, "pending")`
/// share this marker.
fn is_pending_repo(checks: &crate::domain::checks::RepositoryChecks) -> bool {
    checks.secret_scanning.reason.as_deref() == Some("pending")
}

/// Return a pending or unknown status dot based on the `pending` flag.
///
/// Shared by all four check-type `Unknown` arms in [`build_status_dots`]
/// to avoid repeating the same if/else block.
fn unknown_or_pending_dot(pending: bool) -> StatusDot {
    if pending {
        StatusDot {
            css_class: "status-pending",
            label: "Pending",
        }
    } else {
        StatusDot {
            css_class: "status-unknown",
            label: "unknown",
        }
    }
}

/// Map a repository's check results to status dots for each control.
///
/// Returns one [`StatusDot`] per control in [`CONTROL_NAMES`] order:
/// 1. Security Policy
/// 2. Secret Scanning
/// 3. Dependabot Status
/// 4. Branch Protection
///
/// Uses exhaustive `match` arms (no wildcards) so the compiler catches
/// any new status variants added in the future.
fn build_status_dots(checks: &crate::domain::checks::RepositoryChecks) -> Vec<StatusDot> {
    let pending = is_pending_repo(checks);

    let policy_dot = match checks.security_policy.status {
        SecurityPolicyStatus::Pass => StatusDot {
            css_class: "status-pass",
            label: "pass",
        },
        SecurityPolicyStatus::Fail => StatusDot {
            css_class: "status-fail",
            label: "fail",
        },
        SecurityPolicyStatus::Unknown => unknown_or_pending_dot(pending),
        SecurityPolicyStatus::NotApplicable => StatusDot {
            css_class: "status-na",
            label: "N/A",
        },
    };

    let secret_dot = match checks.secret_scanning.status {
        SecretScanningStatus::Enabled => StatusDot {
            css_class: "status-pass",
            label: "enabled",
        },
        SecretScanningStatus::Disabled => StatusDot {
            css_class: "status-fail",
            label: "disabled",
        },
        SecretScanningStatus::PermissionDenied => StatusDot {
            css_class: "status-unknown",
            label: "permission denied",
        },
        SecretScanningStatus::Unknown => unknown_or_pending_dot(pending),
    };

    let dependabot_dot = match checks.dependabot_security_updates.status {
        DependabotStatus::Enabled => StatusDot {
            css_class: "status-pass",
            label: "enabled",
        },
        DependabotStatus::Paused => StatusDot {
            css_class: "status-warn",
            label: "paused",
        },
        DependabotStatus::Disabled => StatusDot {
            css_class: "status-fail",
            label: "disabled",
        },
        DependabotStatus::Unknown => unknown_or_pending_dot(pending),
    };

    let branch_dot = match checks.branch_protection.status {
        BranchProtectionStatus::Pass => StatusDot {
            css_class: "status-pass",
            label: "pass",
        },
        BranchProtectionStatus::Partial => StatusDot {
            css_class: "status-warn",
            label: "partial",
        },
        BranchProtectionStatus::Fail => StatusDot {
            css_class: "status-fail",
            label: "fail",
        },
        BranchProtectionStatus::Unknown => unknown_or_pending_dot(pending),
    };

    vec![policy_dot, secret_dot, dependabot_dot, branch_dot]
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use crate::domain::auth::{AuthMode, Capability, TokenTier};
    use crate::domain::checks::{CollectionFailureReason, ExclusionReason};
    use crate::domain::metrics::{
        AggregatedMetrics, BranchProtectionCounts, CodeownersCounts, CollectionHealthCheckKind,
        CollectionHealthCount, DependabotCounts, PolicyCounts, RateMetric, ScoreExclusionCount,
        SecretAlertCounts, SecretScanningCounts,
    };
    use crate::domain::repository::Visibility;
    use cherry_pit_core::ReadPort;

    use crate::projection::{
        EvidenceProjection, EvidenceProjectionQuery, EvidenceProjectionReadPort,
        EvidenceProjectionResponse,
    };
    use crate::test_fixtures;

    fn sample_metrics() -> AggregatedMetrics {
        AggregatedMetrics {
            security_policy_coverage: RateMetric::new(2, 3)
                .with_extra("observable_repositories", 3)
                .with_extra("unknown", 0),
            policy_counts: PolicyCounts {
                via_setting: 1,
                via_file: 1,
                unknown: 0,
                missing: 1,
            },
            secret_scanning_coverage: RateMetric::new(4, 5)
                .with_extra("disabled", 1)
                .with_extra("permission_denied", 0)
                .with_extra("unknown", 0)
                .with_extra("observable_repositories", 5),
            secret_scanning_counts: SecretScanningCounts {
                enabled: 4,
                disabled: 1,
                permission_denied: 0,
                unknown: 0,
            },
            dependabot_security_updates_coverage: RateMetric::new(3, 5)
                .with_extra("disabled", 1)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 4),
            dependabot_security_updates_counts: DependabotCounts {
                enabled: 3,
                paused: 0,
                disabled: 1,
                unknown: 1,
            },
            open_secret_alert_prevalence: RateMetric::new(1, 4)
                .with_extra("repos_without_open_alerts", 3)
                .with_extra("unobservable", 1),
            secret_alert_counts: SecretAlertCounts {
                repos_with_open_alerts: 1,
                repos_without_open_alerts: 3,
                unobservable: 1,
            },
            branch_protection_coverage: RateMetric::new(3, 5)
                .with_extra("insufficient", 1)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 4),
            branch_protection_counts: BranchProtectionCounts {
                pass: 3,
                partial: 1,
                fail: 0,
                unknown: 1,
            },
            codeowners_coverage: RateMetric::new(4, 5)
                .with_extra("non_conforming", 1)
                .with_extra("absent", 0)
                .with_extra("unknown", 1)
                .with_extra("observable_repositories", 4),
            codeowners_counts: CodeownersCounts {
                conforming: 3,
                non_conforming: 1,
                absent: 0,
                unknown: 1,
                truncated: 0,
            },
            owner_metrics: vec![],
            collection_health_counts: vec![],
            score_exclusion_counts: vec![
                ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::Dependabot,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                },
                ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::BranchProtection,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                },
                ScoreExclusionCount {
                    check_kind: CollectionHealthCheckKind::Codeowners,
                    reason: ExclusionReason::Unknown,
                    count: 1,
                },
            ],
            team_rosters: vec![],
        }
    }

    /// Returns `true` for non-HTML assets (`.css`, `.js`) that should be
    /// skipped when asserting on HTML content in rendered dashboards.
    fn is_non_html_asset(name: &str) -> bool {
        std::path::Path::new(name).extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("css")
                || ext.eq_ignore_ascii_case("js")
                || ext.eq_ignore_ascii_case("wasm")
        })
    }

    fn sample_evidence() -> Evidence {
        let mut observability = test_fixtures::make_observability();
        observability.total_open_secret_alerts = 1;
        observability.observable_enabled_repositories = 4;
        observability.unobservable_repositories = 1;

        test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            test_fixtures::make_collection_statistics(5, 3, 1, 1),
            sample_metrics(),
            observability,
            vec![test_fixtures::all_passing_evidence("repo-1")],
        )
    }

    fn sample_evidence_with_admin_diagnostics() -> Evidence {
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
                reason: CollectionFailureReason::PermissionDenied,
                count: 3,
            },
            CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: CollectionFailureReason::PermissionSuspected,
                count: 1,
            },
        ];

        evidence
    }

    #[test]
    fn dashboard_report_produces_valid_html() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<html lang=\"en\">"));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn dashboard_report_includes_organization_name() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(html.contains("TestOrg GitHub Governance Overview"));
        assert!(html.contains("<code>TestOrg</code>"));
    }

    #[test]
    fn dashboard_report_shows_by_reason_exclusion_breakdown_per_control() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(
            html.contains("1 unmeasured (1 unknown)"),
            "expected the dependabot/branch_protection/codeowners tables to show their \
             1-unknown exclusion from sample_metrics(); report.html:\n{html}"
        );
        assert_eq!(
            html.matches("1 unmeasured (1 unknown)").count(),
            3,
            "dependabot, branch_protection, and codeowners each carry exactly 1 unknown \
             exclusion in sample_metrics()"
        );
        assert!(
            html.contains("0 unmeasured"),
            "security_policy and secret_scanning carry 0 exclusions in sample_metrics()"
        );
    }

    #[test]
    fn dashboard_index_shows_by_reason_exclusion_breakdown_on_scorecard() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["index.html"];

        assert!(
            html.contains("1 unmeasured (1 unknown)"),
            "expected the scorecard to surface the by-reason exclusion breakdown; \
             index.html:\n{html}"
        );
        assert!(
            !html.contains("reserved 0"),
            "the dead always-zero 'reserved' branch-protection label must be gone \
             now that the bucket is live (pcoqb fix)"
        );
    }

    #[test]
    fn dashboard_report_includes_coverage_metrics() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(html.contains("66.7% (2/3)"));
        assert!(html.contains("80.0% (4/5)"));
        assert!(html.contains("60.0% (3/5)"));
    }

    #[test]
    fn dashboard_index_archival_coverage_shows_truthful_ratio() {
        let mut evidence = sample_evidence();
        evidence.repositories[0].repository.updated_at = Some("2023-01-01T00:00:00Z".to_string());
        evidence.collection_statistics.archived_repos = 3;

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("75.0% (3/4)"),
            "Archival Coverage card must show archived/(archived+stale) as (n/d), matching \
             the sibling coverage cards' RateMetric-derived format"
        );
        assert!(
            index.contains("3 archived · 1 stale"),
            "card-detail sub-counts must stay consistent with the (n/d) numerator/denominator"
        );
    }

    /// UF2-6: the security-policy caption must state the population the
    /// code actually computes (`total_public`, `Visibility::Public` incl.
    /// archived — metrics.rs:98-101,173) rather than the stale
    /// "non-archived public repos only" claim it previously carried.
    #[test]
    fn security_policy_caption_matches_computed_population() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(
            html.contains("(public repos only, including archived)"),
            "security-policy caption must state the true population: public, including archived"
        );
        assert!(
            !html.contains("non-archived public repos only"),
            "must not claim archived repos are excluded from security-policy coverage"
        );
    }

    #[test]
    fn dashboard_report_has_no_operations_read_more_links() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(!html.contains("OPERATIONS.html"));
        assert!(!html.contains("Read more"));
    }

    #[test]
    fn dashboard_index_has_no_operations_read_more_links() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["index.html"];

        assert!(!html.contains("OPERATIONS.html"));
        assert!(
            !html.contains("Read more"),
            "control cards must no longer emit Read-more links now that OPERATIONS.html is removed"
        );
    }

    #[test]
    fn dashboard_report_codeowners_prefers_team_over_user() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(html.contains("Prefer a GitHub <strong>team</strong>"));
        assert!(html.contains("top security teams"));
    }

    #[test]
    fn dashboard_report_add_member_guidance_is_generic_by_default() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(html.contains("your GitHub organization"));
        assert!(html.contains("administrators"));
        assert!(html.contains("not a configuration file"));
        assert!(!html.to_lowercase().contains("mattilsynet"));
        assert!(!html.to_lowercase().contains("open a pr"));
        assert!(!html.to_lowercase().contains("pull request to"));
    }

    /// UF2-GEN proof: swapping the org-derived config to a different
    /// organization's values renders that organization's guidance, and
    /// leaks zero "Mattilsynet" strings anywhere in the multi-page output —
    /// proving remediation copy is config-derived, not hardcoded.
    #[test]
    fn org_help_config_swap_renders_configured_org_with_no_mattilsynet_leak() {
        let evidence = sample_evidence();
        let config = DashboardConfig {
            org_help: config::org::OrgHelpConfig {
                team_access: config::org::TeamAccessGuidance {
                    contact: Some("#it-helpdesk on the Acme Slack".to_string()),
                    governance_model: Some("an Acme Identity Center group".to_string()),
                    help_links: vec![config::org::HelpLink {
                        label: "Acme access guide".to_string(),
                        url: "https://acme.example/access".to_string(),
                    }],
                },
            },
            ..DashboardConfig::default()
        };
        let pages = render_dashboard(&evidence, &config).unwrap();
        let report_html = &pages["report.html"];

        assert!(report_html.contains("#it-helpdesk on the Acme Slack"));
        assert!(report_html.contains("an Acme Identity Center group"));
        assert!(report_html.contains(r#"href="https://acme.example/access""#));
        assert!(report_html.contains("Acme access guide"));

        for (page_name, body) in &pages {
            assert!(
                !body.to_lowercase().contains("mattilsynet"),
                "page {page_name} leaked a Mattilsynet string after org config swap"
            );
        }
    }

    #[test]
    fn dashboard_report_snapshot() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_report", &pages["report.html"]);
    }

    #[test]
    fn render_dashboard_index_snapshot() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_index", &pages["index.html"]);
    }

    #[test]
    fn render_dashboard_admin_snapshot() {
        let evidence = sample_evidence_with_admin_diagnostics();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_admin", &pages["admin.html"]);
    }

    #[test]
    fn render_dashboard_index_badge_snapshot() {
        let evidence = sample_evidence_with_admin_diagnostics();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_index_badge", &pages["index.html"]);
    }

    #[test]
    fn render_dashboard_index_zero_badge_snapshot() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_index_zero_badge", &pages["index.html"]);
    }

    #[test]
    fn render_dashboard_admin_page_contains_read_only_diagnostics() {
        let evidence = sample_evidence_with_admin_diagnostics();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let admin = &pages["admin.html"];

        assert!(admin.contains("Admin Diagnostics"));
        assert!(admin.contains("Branch Protection"));
        assert!(admin.contains("permission_denied"));
        assert!(admin.contains("Subtotal"));
        assert!(admin.contains("running with github_app/Limited"));
        assert!(admin.contains("org_secret_scanning_alerts"));
        assert!(!admin.contains("<form"));
        assert!(!admin.contains("method=\"post\""));
        assert_eq!(
            admin.matches("<script").count(),
            1,
            "admin page carries only the sort-init.js progressive-enhancement loader"
        );
        assert!(admin.contains("<script type=\"module\" src=\"sort-init.js\"></script>"));
    }

    #[test]
    fn projection_current_state_renders_stable_html() {
        let mut projection = EvidenceProjection::default();
        let mut active = test_fixtures::all_passing_evidence("active-repo");
        active.checks.codeowners = test_fixtures::codeowners_absent();
        let removed = test_fixtures::all_passing_evidence("removed-repo");

        projection.load_baseline(vec![active.clone(), removed.clone()]);
        projection
            .repositories
            .remove(&removed.repository.inventory_key);

        let repositories = match EvidenceProjectionReadPort::resolve(
            &projection,
            EvidenceProjectionQuery::SortedSnapshot,
        ) {
            EvidenceProjectionResponse::Many(repositories) => repositories,
            _ => Vec::new(),
        };

        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            sample_metrics(),
            test_fixtures::make_observability(),
            repositories,
        );
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let orphaned_html = &pages["orphans.html"];
        assert!(
            orphaned_html.contains("active-repo"),
            "orphans.html should render the surviving projection repository"
        );
        assert!(
            !orphaned_html.contains("removed-repo"),
            "orphans.html must not render the tombstoned repository"
        );

        insta::assert_snapshot!("projection_current_state_index", &pages["index.html"]);
        insta::assert_snapshot!("projection_current_state_orphans", &pages["orphans.html"]);
        insta::assert_snapshot!("projection_current_state_report", &pages["report.html"]);
    }

    #[test]
    fn dashboard_report_escapes_html_in_org_name() {
        let mut evidence = sample_evidence();
        evidence.assessment_metadata.organization = "<script>alert('xss')</script>".to_string();

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let html = &pages["report.html"];

        assert!(
            !html.contains("<script>alert('xss')</script>"),
            "raw script tag must be escaped"
        );
        assert!(
            html.contains("&#60;script&#62;") || html.contains("&lt;script&gt;"),
            "expected escaped angle brackets in output"
        );
    }

    #[test]
    fn render_dashboard_produces_all_pages() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        assert!(pages.contains_key("index.html"));
        assert!(pages.contains_key("report.html"));
        assert!(pages.contains_key("admin.html"));
        assert!(pages.contains_key("style.css"));
        assert!(pages.contains_key("ws.js"));
        assert!(pages.contains_key("gh-report-web-client.js"));
        assert!(pages.contains_key("gh-report-web-client_bg.wasm"));
        assert!(pages.contains_key("sort-init.js"));
        assert!(pages.contains_key("orphans.html"));
        assert!(pages.contains_key("deleted.html"));
        assert!(!pages.contains_key("OPERATIONS.html"));
        assert_eq!(pages.len(), 10);
    }

    #[test]
    fn render_dashboard_streaming_produces_same_key_set_as_render_dashboard() {
        let evidence = evidence_with_owner_repos();
        let via_map = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let mut via_sink = std::collections::HashSet::new();
        render_dashboard_streaming(&evidence, &DashboardConfig::default(), |path, _content| {
            via_sink.insert(path);
        })
        .unwrap();

        let map_keys: std::collections::HashSet<String> = via_map.keys().cloned().collect();
        assert_eq!(
            via_sink, map_keys,
            "streaming sink page-key set must match the HashMap-collecting wrapper"
        );
    }

    #[test]
    fn every_html_page_has_balanced_script_tags() {
        let cases = [
            sample_evidence(),
            sample_evidence_with_admin_diagnostics(),
            evidence_with_owner_repos(),
        ];
        for evidence in &cases {
            let pages = render_dashboard(evidence, &DashboardConfig::default()).unwrap();
            for (name, body) in &pages {
                let is_html = std::path::Path::new(name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("html"));
                if !is_html {
                    continue;
                }
                let opens = body.matches("<script").count();
                let closes = body.matches("</script>").count();
                assert_eq!(
                    opens, closes,
                    "{name} has {opens} <script> vs {closes} </script>; an unclosed \
                     script tag swallows the rest of the document as script text"
                );
            }
        }
    }

    #[test]
    fn render_dashboard_index_badge_counts_admin_technical_issues() {
        let evidence = sample_evidence_with_admin_diagnostics();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(index.contains("href=\"admin.html\""));
        assert!(index.contains("Admin (8)"));
    }

    #[test]
    fn render_dashboard_index_omits_warning_badge_when_zero_issues() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(index.contains("href=\"admin.html\""));
        assert!(!index.contains("Admin ("));
    }

    #[test]
    fn render_dashboard_includes_stylesheet() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let css = &pages["style.css"];

        assert!(css.contains(":root"));
        assert!(css.contains(".scorecard"));
        assert!(css.contains(".card"));
    }

    #[test]
    fn render_dashboard_index_contains_scorecard() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(index.contains("<!DOCTYPE html>"));
        assert!(!index.contains("<h1>Repo governance dashboard</h1>"));
        assert!(
            !index.contains("<h1>"),
            "index page should have no h1 heading"
        );
        assert!(index.contains("66.7% (2/3)"));
        assert!(index.contains("80.0% (4/5)"));
    }

    #[test]
    fn render_dashboard_index_contains_health_score() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("Org Governance"),
            "index should contain the Org Governance card label"
        );
        assert!(
            !index.contains("Organisation Governance Score"),
            "the old 'Organisation Governance Score' label must be fully replaced (item6-04)"
        );
        assert!(
            index.contains("health-score"),
            "index should contain health-score CSS class"
        );
        assert!(
            index.contains("68.8%"),
            "health score should display the geometric mean: 68.8%"
        );
        assert!(
            index.contains("tier-warn"),
            "health score 68.6% should be classified as warn tier (< 80 threshold)"
        );
    }

    #[test]
    fn render_dashboard_index_org_governance_tooltip_states_formula_and_exclusion_rule() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("Geometric mean of measured control rates across six controls"),
            "Org Governance tooltip must state its exact formula; index.html:\n{index}"
        );
        assert!(
            index.contains(
                "Security Policy, Secret Scanning, Dependabot, Branch Protection, CODEOWNERS, Archival Coverage"
            ),
            "Org Governance tooltip must state its six-control set"
        );
        assert!(
            index.contains("Unmeasured controls are excluded from each rate's denominator"),
            "Org Governance tooltip must state the exclusion rule"
        );
    }

    #[test]
    fn render_dashboard_index_archival_coverage_tooltip_states_formula() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains(
                "Archived / (archived + stale-active) — fraction of stale-lifecycle repos that have been archived"
            ),
            "Archival Coverage tooltip must state its exact formula; index.html:\n{index}"
        );
    }

    #[test]
    fn render_dashboard_index_health_score_na_when_all_zero_denom() {
        let mut evidence = sample_evidence();
        evidence.metrics.security_policy_coverage = RateMetric::new(0, 0);
        evidence.metrics.secret_scanning_coverage = RateMetric::new(0, 0);
        evidence.metrics.dependabot_security_updates_coverage = RateMetric::new(0, 0);
        evidence.metrics.branch_protection_coverage = RateMetric::new(0, 0);
        evidence.metrics.codeowners_coverage = RateMetric::new(0, 0);

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("Org Governance"),
            "Org Governance card should still appear when N/A"
        );
        assert!(
            index.contains("tier-na"),
            "health score card should have tier-na class when all rates are N/A"
        );
    }

    #[test]
    fn render_dashboard_index_links_to_report() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(index.contains("href=\"report.html\""));
    }

    #[test]
    fn render_dashboard_index_escapes_org_name() {
        let mut evidence = sample_evidence();
        evidence.assessment_metadata.organization = "<script>xss</script>".to_string();

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            !index.contains("<script>xss</script>"),
            "raw script tag must be escaped in index"
        );
    }

    use crate::domain::checks::{
        BranchProtectionDetails, BranchProtectionResult, CodeownersResult, CodeownersStatus,
        DependabotResult, RepositoryChecks, SecretScanningResult, SecurityPolicyEvidence,
        SecurityPolicyResult,
    };

    fn make_checks_with_statuses(
        policy: SecurityPolicyStatus,
        secret: SecretScanningStatus,
        dependabot: DependabotStatus,
        branch: BranchProtectionStatus,
    ) -> RepositoryChecks {
        let branch_details = match branch {
            BranchProtectionStatus::Pass => BranchProtectionDetails {
                default_branch: "main".to_string(),
                has_pr: Some(true),
                required_reviewers: Some(1),
                has_status_checks: Some(false),
                admin_equivalent: Some(false),
                has_broad_bypass: Some(false),
                reason: None,
                reason_kind: None,
                http_status: None,
                force_push_blocked: Some(true),
                deletion_blocked: Some(true),
            },
            BranchProtectionStatus::Partial => BranchProtectionDetails {
                default_branch: "main".to_string(),
                has_pr: Some(false),
                required_reviewers: Some(0),
                has_status_checks: Some(false),
                admin_equivalent: Some(true),
                has_broad_bypass: Some(false),
                reason: None,
                reason_kind: None,
                http_status: None,
                force_push_blocked: Some(true),
                deletion_blocked: Some(true),
            },
            BranchProtectionStatus::Fail => BranchProtectionDetails {
                default_branch: "main".to_string(),
                has_pr: None,
                required_reviewers: None,
                has_status_checks: None,
                admin_equivalent: None,
                has_broad_bypass: None,
                reason: None,
                reason_kind: None,
                http_status: None,
                force_push_blocked: None,
                deletion_blocked: None,
            },
            BranchProtectionStatus::Unknown => BranchProtectionDetails {
                default_branch: "main".to_string(),
                has_pr: None,
                required_reviewers: None,
                has_status_checks: None,
                admin_equivalent: None,
                has_broad_bypass: None,
                reason: Some("permission_denied".to_string()),
                reason_kind: Some(CollectionFailureReason::PermissionDenied),
                http_status: Some(403),
                force_push_blocked: None,
                deletion_blocked: None,
            },
        };

        RepositoryChecks {
            security_policy: SecurityPolicyResult {
                status: policy,
                evidence: SecurityPolicyEvidence::Setting,
                path: None,
                timestamp: test_fixtures::make_timestamp(),
            },
            secret_scanning: SecretScanningResult {
                status: secret,
                has_open_alerts: None,
                alerts_observable: false,
                reason: None,
                timestamp: test_fixtures::make_timestamp(),
            },
            dependabot_security_updates: DependabotResult {
                status: dependabot,
                reason: None,
                timestamp: test_fixtures::make_timestamp(),
            },
            branch_protection: BranchProtectionResult {
                status: branch,
                details: branch_details,
                timestamp: test_fixtures::make_timestamp(),
            },
            codeowners: CodeownersResult {
                status: CodeownersStatus::Conforming,
                path: Some(".github/CODEOWNERS".to_string()),
                timestamp: test_fixtures::make_timestamp(),
                parsed: None,
                truncation: None,
            },
        }
    }

    #[test]
    fn status_dots_all_passing() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        let dots = build_status_dots(&checks);

        assert_eq!(dots.len(), 4);
        assert_eq!(dots[0].css_class, "status-pass");
        assert_eq!(dots[0].label, "pass");
        assert_eq!(dots[1].css_class, "status-pass");
        assert_eq!(dots[1].label, "enabled");
        assert_eq!(dots[2].css_class, "status-pass");
        assert_eq!(dots[2].label, "enabled");
        assert_eq!(dots[3].css_class, "status-pass");
        assert_eq!(dots[3].label, "pass");
    }

    #[test]
    fn status_dots_all_failing() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Fail,
            SecretScanningStatus::Disabled,
            DependabotStatus::Disabled,
            BranchProtectionStatus::Fail,
        );
        let dots = build_status_dots(&checks);

        assert_eq!(dots[0].css_class, "status-fail");
        assert_eq!(dots[0].label, "fail");
        assert_eq!(dots[1].css_class, "status-fail");
        assert_eq!(dots[1].label, "disabled");
        assert_eq!(dots[2].css_class, "status-fail");
        assert_eq!(dots[2].label, "disabled");
        assert_eq!(dots[3].css_class, "status-fail");
        assert_eq!(dots[3].label, "fail");
    }

    #[test]
    fn status_dots_all_unknown() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        let dots = build_status_dots(&checks);

        for dot in &dots {
            assert_eq!(dot.css_class, "status-unknown");
        }
        assert_eq!(dots[0].label, "unknown");
        assert_eq!(dots[1].label, "unknown");
        assert_eq!(dots[2].label, "unknown");
        assert_eq!(dots[3].label, "unknown");
    }

    #[test]
    fn status_dots_branch_partial() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Partial,
        );
        let dots = build_status_dots(&checks);

        assert_eq!(dots[3].css_class, "status-warn");
        assert_eq!(dots[3].label, "partial");
    }

    #[test]
    fn status_dots_secret_scanning_permission_denied() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::PermissionDenied,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        let dots = build_status_dots(&checks);

        assert_eq!(dots[1].css_class, "status-unknown");
        assert_eq!(dots[1].label, "permission denied");
    }

    use crate::domain::codeowners::ParsedCodeowners;

    fn evidence_with_owner_repos() -> Evidence {
        let repos = vec![
            test_fixtures::make_repository_evidence(
                "beta-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/team-a"]),
                ),
            ),
            test_fixtures::make_repository_evidence(
                "alpha-repo",
                Visibility::Private,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_fail(),
                    test_fixtures::secret_disabled(),
                    test_fixtures::dependabot_disabled(),
                    test_fixtures::branch_fail(),
                    test_fixtures::codeowners_with_owners(&["@org/team-a"]),
                ),
            ),
        ];

        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);

        test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        )
    }

    /// Owner-scoped variant of [`evidence_with_owner_repos`] where one
    /// repo's `security_policy` status is `Unknown` (item6-03, bd bead
    /// `adr-fmt-orvyn`) — exercises the by-reason exclusion breakdown
    /// surfaced on the owners overview tooltip and the owner detail summary
    /// card, distinct from the clean (no-exclusion) fixture used by the
    /// locked `dashboard_owners`/`dashboard_owner_detail` snapshots.
    fn evidence_with_owner_repo_exclusions() -> Evidence {
        let repos = vec![
            test_fixtures::make_repository_evidence(
                "beta-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/team-a"]),
                ),
            ),
            test_fixtures::make_repository_evidence(
                "gamma-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_unknown(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/team-a"]),
                ),
            ),
        ];

        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);

        test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        )
    }

    fn evidence_with_full_nav_surface() -> Evidence {
        let mut evidence = evidence_with_owner_repos();
        evidence
            .repositories
            .push(test_fixtures::make_repository_evidence(
                "orphan-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_absent(),
                ),
            ));
        evidence.metrics = crate::aggregate::metrics::aggregate_metrics(&evidence.repositories);
        evidence.collection_statistics =
            crate::aggregate::metrics::build_collection_statistics(&evidence.repositories);
        evidence.deleted = vec![crate::projection::DeletedRepoRecord {
            repo_name: "deleted-repo".to_string(),
            detected_at: "2026-06-24T00:00:00Z".to_string(),
        }];
        evidence.metrics.collection_health_counts = vec![CollectionHealthCount {
            check_kind: CollectionHealthCheckKind::Rulesets,
            reason: CollectionFailureReason::RateLimited,
            count: 4,
        }];
        evidence
    }

    fn extract_top_nav(html: &str) -> &str {
        let start = html
            .find("<nav class=\"top-nav\">")
            .expect("page should render a top-nav element");
        let end = html[start..]
            .find("</nav>")
            .expect("top-nav element should close");
        &html[start..start + end + "</nav>".len()]
    }

    #[test]
    fn nav_identical_across_all_page_types() {
        let evidence = evidence_with_full_nav_surface();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let canonical = extract_top_nav(&pages["index.html"]);
        assert!(
            canonical.contains("Orphans ("),
            "canonical nav should show an orphans count"
        );
        assert!(
            canonical.contains("Deleted ("),
            "canonical nav should show a deleted count"
        );
        assert!(
            canonical.contains("Admin ("),
            "canonical nav should show the admin technical-issues count"
        );
        assert!(
            canonical.contains(">Owners<"),
            "canonical nav should show the Owners link when owner data exists"
        );

        for page in [
            "report.html",
            "owners.html",
            "orphans.html",
            "deleted.html",
            "admin.html",
        ] {
            assert_eq!(
                extract_top_nav(&pages[page]),
                canonical,
                "{page} top-nav must be byte-identical to index.html's canonical nav"
            );
        }

        let detail_page = &pages["owners/org-team-a.html"];
        let detail_nav = extract_top_nav(detail_page).replace("../", "");
        assert_eq!(
            detail_nav, canonical,
            "owner_detail.html top-nav must match the canonical nav once its ../ prefix is stripped"
        );
    }

    fn extract_attr_values<'a>(html: &'a str, attr: &str) -> Vec<&'a str> {
        let needle = format!("{attr}=\"");
        let mut out = Vec::new();
        let mut rest = html;
        while let Some(pos) = rest.find(&needle) {
            let after = &rest[pos + needle.len()..];
            let Some(end) = after.find('"') else { break };
            out.push(&after[..end]);
            rest = &after[end + 1..];
        }
        out
    }

    fn resolve_href_target(href: &str, current_page: &str) -> (String, Option<String>) {
        let (page_part, fragment) = match href.split_once('#') {
            Some((page, frag)) => (page, Some(frag.to_string())),
            None => (href, None),
        };
        let target_page = if let Some(stripped) = page_part.strip_prefix("../") {
            stripped.to_string()
        } else if page_part.is_empty() {
            current_page.to_string()
        } else {
            page_part.to_string()
        };
        (target_page, fragment)
    }

    fn is_servable_page_reference(target: &str) -> bool {
        std::path::Path::new(target).extension().is_some_and(|ext| {
            ext.eq_ignore_ascii_case("html")
                || ext.eq_ignore_ascii_case("css")
                || ext.eq_ignore_ascii_case("js")
        })
    }

    /// UF3-2 "links in general" guard (mirrors COM-0027/COM-0017 and the
    /// adr-fmt link-integrity discipline): renders the full page set,
    /// extracts every internal `href`, and asserts each resolves to a
    /// served page and — for fragments — an existing `id=` anchor on that
    /// page.
    #[test]
    fn served_pages_have_no_dangling_internal_links() {
        let mut evidence = evidence_with_full_nav_surface();
        evidence.assessment_metadata.auth_mode = AuthMode::Pat;
        evidence.assessment_metadata.token_tier = TokenTier::Unknown;
        evidence.assessment_metadata.unavailable_capabilities = vec![
            Capability::PrivateBranchProtectionRead,
            Capability::OrgSecretScanningAlerts,
        ];
        evidence
            .metrics
            .collection_health_counts
            .push(CollectionHealthCount {
                check_kind: CollectionHealthCheckKind::BranchProtection,
                reason: CollectionFailureReason::PermissionSuspected,
                count: 1,
            });

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let ids_by_page: HashMap<&str, HashSet<&str>> = pages
            .iter()
            .map(|(key, html)| {
                (
                    key.as_str(),
                    extract_attr_values(html, "id").into_iter().collect(),
                )
            })
            .collect();

        let mut dangling = Vec::new();
        for (page, html) in &pages {
            let is_html_page = std::path::Path::new(page)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("html"));
            if !is_html_page {
                continue;
            }
            for href in extract_attr_values(html, "href") {
                if href.starts_with("http://")
                    || href.starts_with("https://")
                    || href.starts_with("mailto:")
                {
                    continue;
                }
                let (target_page, fragment) = resolve_href_target(href, page);
                if !is_servable_page_reference(&target_page) {
                    continue;
                }
                if !pages.contains_key(target_page.as_str()) {
                    dangling.push(format!(
                        "{page}: href=\"{href}\" targets unserved page {target_page:?}"
                    ));
                    continue;
                }
                if let Some(frag) = fragment
                    && !ids_by_page[target_page.as_str()].contains(frag.as_str())
                {
                    dangling.push(format!(
                        "{page}: href=\"{href}\" fragment #{frag} has no id= on {target_page}"
                    ));
                }
            }
        }
        assert!(
            dangling.is_empty(),
            "dangling internal links found:\n{}",
            dangling.join("\n")
        );

        assert!(
            !ids_by_page.contains_key("OPERATIONS.html"),
            "OPERATIONS.html must no longer be a served page"
        );
    }

    #[test]
    fn detail_vm_control_columns_populated() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        assert_eq!(detail_vms.len(), 1);
        let (_, vm) = &detail_vms[0];
        let names: Vec<&str> = vm.control_columns.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            vec![
                "Security Policy",
                "Secret Scanning",
                "Dependabot Status",
                "Branch Protection"
            ]
        );
        assert_eq!(
            vm.control_columns[0].tooltip,
            coverage_control_column_tooltip("security_policy").unwrap()
        );
        assert!(vm.control_columns.iter().all(|c| !c.tooltip.is_empty()));
    }

    #[test]
    fn detail_vm_summary_cards_have_labels() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        assert_eq!(vm.summary_cards.len(), 4);
        assert_eq!(vm.summary_cards[0].label, "Security Policy");
        assert_eq!(vm.summary_cards[1].label, "Secret Scanning");
        assert_eq!(vm.summary_cards[2].label, "Dependabot Status");
        assert_eq!(vm.summary_cards[3].label, "Branch Protection");
        assert!(vm.summary_cards[0].cell.rate_formatted.contains('%'));
    }

    #[test]
    fn detail_vm_summary_cards_have_no_operations_anchor_field() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(!detail_page.contains("OPERATIONS.html"));
        assert!(!detail_page.contains("Read more"));
    }

    #[test]
    fn detail_vm_repo_rows_populated() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        assert_eq!(vm.repo_rows.len(), 2);
        assert_eq!(vm.repo_rows[0].controls.len(), 4);
        assert_eq!(vm.repo_rows[1].controls.len(), 4);
        for row in &vm.repo_rows {
            assert!(
                row.repo_url
                    .starts_with(config::DEFAULT_GITHUB_WEB_BASE_URL),
                "repo_url should start with the GitHub web base URL"
            );
            assert!(
                row.repo_url.contains("/TestOrg/"),
                "repo_url should contain the organization name"
            );
        }
    }

    #[test]
    fn detail_vm_repo_rows_sorted_case_insensitive() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        assert_eq!(vm.repo_rows[0].repo_name, "alpha-repo");
        assert_eq!(vm.repo_rows[1].repo_name, "beta-repo");
    }

    #[test]
    fn detail_vm_repo_rows_status_dots_correct() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        let alpha = &vm.repo_rows[0];
        assert_eq!(alpha.controls[0].css_class, "status-fail");
        assert_eq!(alpha.controls[1].css_class, "status-fail");
        assert_eq!(alpha.controls[2].css_class, "status-fail");
        assert_eq!(alpha.controls[3].css_class, "status-fail");

        let beta = &vm.repo_rows[1];
        assert_eq!(beta.controls[0].css_class, "status-pass");
        assert_eq!(beta.controls[1].css_class, "status-pass");
        assert_eq!(beta.controls[2].css_class, "status-pass");
        assert_eq!(beta.controls[3].css_class, "status-pass");
    }

    #[test]
    fn detail_vm_no_matching_repos_shows_empty() {
        use crate::domain::metrics::{OwnerMetrics, OwnerType};

        let owner_metrics = vec![OwnerMetrics {
            owner: "@org/phantom".to_string(),
            display_name: "@org/phantom".to_string(),
            owner_type: OwnerType::Team,
            total_repos: 1,
            per_control_coverage: std::collections::HashMap::new(),
            score_exclusion_counts: Vec::new(),
            in_org: None,
        }];

        let empty_repos: &[RepositoryEvidence] = &[];
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(empty_repos);
        let detail_vms = build_owner_detail_view_models(
            &owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            "TestOrg",
            "2026-04-09T12:00:00+00:00",
            &[],
            &[],
        );

        assert_eq!(detail_vms.len(), 1);
        let (_, vm) = &detail_vms[0];
        assert!(vm.repo_rows.is_empty());
    }

    #[test]
    fn detail_vm_multi_owner_repo_appears_in_both() {
        let repos = vec![test_fixtures::make_repository_evidence(
            "shared-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-a", "@org/team-b"]),
            ),
        )];

        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        assert_eq!(detail_vms.len(), 2);
        let expected_url = format!(
            "{}/TestOrg/shared-repo",
            config::DEFAULT_GITHUB_WEB_BASE_URL
        );
        for (_, vm) in &detail_vms {
            assert_eq!(vm.repo_rows.len(), 1);
            assert_eq!(vm.repo_rows[0].repo_name, "shared-repo");
            assert_eq!(vm.repo_rows[0].repo_url, expected_url);
        }
    }

    #[test]
    fn detail_vm_repo_url_points_to_github() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        assert_eq!(
            vm.repo_rows[0].repo_url,
            format!("{}/TestOrg/alpha-repo", config::DEFAULT_GITHUB_WEB_BASE_URL),
        );
        assert_eq!(
            vm.repo_rows[1].repo_url,
            format!("{}/TestOrg/beta-repo", config::DEFAULT_GITHUB_WEB_BASE_URL),
        );
    }

    #[test]
    fn detail_vm_repo_url_percent_encodes_special_chars() {
        let repos = vec![test_fixtures::make_repository_evidence(
            "my repo#1",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-a"]),
            ),
        )];

        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            "My Org",
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        assert_eq!(detail_vms.len(), 1);
        let (_, vm) = &detail_vms[0];
        assert_eq!(vm.repo_rows.len(), 1);
        assert_eq!(
            vm.repo_rows[0].repo_url,
            format!(
                "{}/My%20Org/my%20repo%231",
                config::DEFAULT_GITHUB_WEB_BASE_URL
            ),
        );
    }

    #[test]
    fn render_owner_detail_html_repo_links_contain_href() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("alpha-repo"),
            "private repo name should appear in detail page"
        );
        assert!(
            !detail_page.contains("[private repo]"),
            "detail page should not show [private repo] placeholder"
        );
        assert!(
            detail_page.contains("href=\"https://github.com/TestOrg/alpha-repo\""),
            "detail page should contain href for alpha-repo"
        );
        assert!(
            detail_page.contains("href=\"https://github.com/TestOrg/beta-repo\""),
            "detail page should contain href for beta-repo"
        );
        assert!(
            detail_page.contains("target=\"_blank\""),
            "repo links should open in a new tab"
        );
        assert!(
            detail_page.contains("rel=\"noopener noreferrer\""),
            "repo links should have noopener noreferrer"
        );
    }

    #[test]
    fn render_dashboard_with_owners_produces_detail_pages() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        assert!(pages.contains_key("owners.html"));
        let detail_pages: Vec<_> = pages.keys().filter(|k| k.starts_with("owners/")).collect();
        assert!(
            !detail_pages.is_empty(),
            "expected at least one owner detail page"
        );
        let owners_html = &pages["owners.html"];
        assert!(
            owners_html.contains("Orphans ("),
            "owners.html should have orphans nav link"
        );
    }

    #[test]
    fn owners_page_has_no_operations_read_more_link() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let owners_html = &pages["owners.html"];

        assert!(!owners_html.contains("OPERATIONS.html"));
        assert!(!owners_html.contains("Read more"));
    }

    #[test]
    fn render_dashboard_owners_snapshot() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_owners", &pages["owners.html"]);
    }

    #[test]
    fn render_dashboard_owner_detail_snapshot() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        insta::assert_snapshot!("dashboard_owner_detail", &pages["owners/org-team-a.html"]);
    }

    #[test]
    fn owners_page_team_health_tooltip_states_formula_and_exclusion_rule() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let owners_html = &pages["owners.html"];

        assert!(
            owners_html.contains("Team Health"),
            "owners.html should contain the Team Health column label"
        );
        assert!(
            !owners_html.contains("Sec Score"),
            "the old 'Sec Score' label must be fully replaced (item6-04)"
        );
        assert!(
            owners_html.contains("Geometric mean of measured control rates across six controls"),
            "Team Health tooltip must state its exact formula; owners.html:\n{owners_html}"
        );
        assert!(
            owners_html.contains(
                "Security Policy, Secret Scanning, Dependabot, Branch Protection, Freshness, Alert-Free"
            ),
            "Team Health tooltip must state its six-control set using the new Freshness label"
        );
        assert!(
            !owners_html.contains("Non-Stale"),
            "the old 'Non-Stale' control label must be fully replaced by 'Freshness' (item6-04 D4)"
        );
        assert!(
            owners_html
                .contains("Unmeasured controls are excluded from each rate&#39;s denominator"),
            "Team Health tooltip must state the exclusion rule"
        );
    }

    #[test]
    fn owners_page_surfaces_by_reason_exclusion_in_tooltip() {
        let evidence = evidence_with_owner_repo_exclusions();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let owners_html = &pages["owners.html"];

        assert!(
            owners_html.contains("1 unmeasured (1 unknown)"),
            "expected the security_policy status-dot tooltip to surface the \
             1-unknown exclusion for @org/team-a; owners.html:\n{owners_html}"
        );
    }

    #[test]
    fn owner_detail_page_surfaces_by_reason_exclusion_on_summary_card() {
        let evidence = evidence_with_owner_repo_exclusions();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_html = &pages["owners/org-team-a.html"];

        assert!(
            detail_html.contains("1 unmeasured (1 unknown)"),
            "expected the security_policy summary card to surface the \
             1-unknown exclusion; owner detail html:\n{detail_html}"
        );
    }

    #[test]
    fn owners_page_clean_owner_omits_exclusion_text_unconditionally() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        assert!(
            !pages["owners.html"].contains("unmeasured"),
            "no control is excluded in evidence_with_owner_repos(); the \
             tooltip addition must stay silent (gated on excluded_total > 0), \
             not print '0 unmeasured' unconditionally"
        );
        assert!(
            !pages["owners/org-team-a.html"].contains("unmeasured"),
            "no control is excluded in evidence_with_owner_repos(); the \
             summary-card addition must stay silent, not print '0 unmeasured' \
             unconditionally"
        );
    }

    /// Item 7: a team's attributed orphan repos render in a default-
    /// collapsed `<details>` section at the bottom of its own detail page,
    /// joined by canonical owner name (not slug) — and a sibling team with
    /// zero attributed orphans omits the section entirely on its own page.
    #[test]
    fn render_owner_detail_html_contains_orphan_repositories_section() {
        use crate::domain::evidence::LastCommitInfo;
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut evidence = evidence_with_owner_repos();
        evidence
            .repositories
            .push(test_fixtures::make_repository_evidence(
                "gamma-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/team-b"]),
                ),
            ));

        let mut orphan = test_fixtures::make_repository_evidence(
            "orphan-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        orphan.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: None,
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });
        evidence.repositories.push(orphan);

        evidence.metrics = crate::aggregate::metrics::aggregate_metrics(&evidence.repositories);
        evidence.collection_statistics =
            crate::aggregate::metrics::build_collection_statistics(&evidence.repositories);
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Maintainer,
                in_org: None,
            }],
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let attributed_page = &pages["owners/org-team-a.html"];
        assert!(
            attributed_page.contains("<details>"),
            "team-a has one attributed orphan; expected a details section"
        );
        assert!(
            !attributed_page.contains("<details open"),
            "the orphan section must be default-collapsed (no open attribute)"
        );
        assert!(
            attributed_page.contains("Orphan repositories (1)"),
            "expected the orphan count in the summary"
        );
        assert!(
            attributed_page.contains("orphan-repo"),
            "expected the attributed orphan repo row to render"
        );

        let unattributed_page = &pages["owners/org-team-b.html"];
        assert!(
            !unattributed_page.contains("<details>"),
            "team-b has zero attributed orphans; the section must be omitted entirely"
        );
    }

    /// UF2-3 rendering test: the owner-detail heading renders the team
    /// handle as a hyperlink to its GitHub team page, with the link base
    /// derived from the already-generic `DEFAULT_GITHUB_WEB_BASE_URL` seam
    /// (org name comes from `vm.organization`, never a literal).
    #[test]
    fn render_owner_detail_html_team_handle_links_to_github_team_page() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = &pages["owners/org-team-a.html"];

        assert!(
            detail_page.contains(r#"<a href="https://github.com/orgs/TestOrg/teams/team-a""#),
            "expected the H1 team handle to link to the GitHub team page; got: {detail_page}"
        );
    }

    #[test]
    fn render_owner_detail_html_summary_cards_have_no_operations_link() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = &pages["owners/org-team-a.html"];

        assert!(!detail_page.contains("OPERATIONS.html"));
        assert!(
            !detail_page.contains("Read more"),
            "owner detail summary cards must no longer emit Read-more links"
        );
    }

    #[test]
    fn build_owner_github_url_team_type_uses_org_teams_path() {
        let url = build_owner_github_url(OwnerType::Team, "@acme/security-team", "acme");
        assert_eq!(
            url.as_deref(),
            Some("https://github.com/orgs/acme/teams/security-team")
        );
    }

    #[test]
    fn build_owner_github_url_user_type_uses_profile_path() {
        let url = build_owner_github_url(OwnerType::User, "@octocat", "acme");
        assert_eq!(url.as_deref(), Some("https://github.com/octocat"));
    }

    #[test]
    fn build_owner_github_url_malformed_team_returns_none() {
        let url = build_owner_github_url(OwnerType::Team, "@team-with-no-slash", "acme");
        assert_eq!(url, None);
    }

    #[test]
    fn build_owner_github_url_bare_at_user_returns_none() {
        let url = build_owner_github_url(OwnerType::User, "@", "acme");
        assert_eq!(url, None);
    }

    #[test]
    fn build_team_security_url_team_type_targets_security_overview() {
        let url = build_team_security_url(OwnerType::Team, "@acme/app-platform", "acme");
        assert_eq!(
            url.as_deref(),
            Some(
                "https://github.com/orgs/acme/security/overview?query=archived%3Afalse%20tool%3Agithub%20team%3Aapp-platform"
            )
        );
    }

    #[test]
    fn build_team_security_url_user_type_returns_none() {
        assert_eq!(
            build_team_security_url(OwnerType::User, "@octocat", "acme"),
            None
        );
    }

    #[test]
    fn build_team_security_url_malformed_team_returns_none() {
        assert_eq!(
            build_team_security_url(OwnerType::Team, "@team-with-no-slash", "acme"),
            None
        );
    }

    /// B1: a realistic multi-member team roster renders on the owner
    /// detail page — both a maintainer and a plain member, with role
    /// labels distinguishing them.
    #[test]
    fn render_owner_detail_html_contains_team_roster() {
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut evidence = evidence_with_owner_repos();
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![
                TeamMember {
                    login: "alice".to_string(),
                    role: TeamMemberRole::Maintainer,
                    in_org: Some(false),
                },
                TeamMember {
                    login: "bob".to_string(),
                    role: TeamMemberRole::Member,
                    in_org: Some(true),
                },
            ],
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = &pages["owners/org-team-a.html"];

        assert!(
            detail_page.contains("Team Members"),
            "expected a Team Members section"
        );
        assert!(detail_page.contains("alice"), "expected maintainer login");
        assert!(detail_page.contains("bob"), "expected member login");
        assert!(
            detail_page.contains("Maintainer"),
            "expected role label Maintainer"
        );
        assert!(
            !detail_page.contains("this list may be incomplete"),
            "Complete status must not show the degraded-roster notice"
        );
        assert!(
            detail_page.contains("../report.html#add-a-team-member"),
            "expected the A3 add-a-member affordance to be reused, not duplicated"
        );
        assert!(
            detail_page.contains("No longer a member of this GitHub organisation."),
            "item9 Part B: departed member 'alice' (in_org=Some(false)) must show the warning tooltip"
        );
        let alice_idx = detail_page.find("alice").expect("alice login rendered");
        let bob_idx = detail_page.find("bob").expect("bob login rendered");
        let warn_idx = detail_page
            .find("No longer a member of this GitHub organisation.")
            .expect("warning tooltip rendered");
        assert!(
            alice_idx < warn_idx && warn_idx < bob_idx,
            "warning badge must render on alice's row (between alice's and bob's rows), not bob's — \
             alice={alice_idx} warn={warn_idx} bob={bob_idx}"
        );
    }

    /// item9 Part B test (b)/(c) render-level: a member confirmed present
    /// (`Some(true)`) shows no warning; a member with `in_org` unknown
    /// (`None`, degraded fetch) also shows no warning — never flag on
    /// missing data.
    #[test]
    fn render_owner_detail_html_no_departed_warning_when_present_or_degraded() {
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut evidence = evidence_with_owner_repos();
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![
                TeamMember {
                    login: "alice".to_string(),
                    role: TeamMemberRole::Maintainer,
                    in_org: Some(true),
                },
                TeamMember {
                    login: "bob".to_string(),
                    role: TeamMemberRole::Member,
                    in_org: None,
                },
            ],
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = &pages["owners/org-team-a.html"];

        assert!(
            !detail_page.contains("No longer a member of this GitHub organisation."),
            "present (Some(true)) and unknown (None) members must never show the departed warning"
        );
    }

    #[test]
    fn render_owner_detail_html_codeowners_meta_has_no_operations_link() {
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut evidence = evidence_with_owner_repos();
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Maintainer,
                in_org: None,
            }],
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = &pages["owners/org-team-a.html"];

        assert!(
            !detail_page.contains("OPERATIONS.html"),
            "CODEOWNERS meta line must no longer link to OPERATIONS.html"
        );
        assert!(
            detail_page.contains("../report.html#add-a-team-member"),
            "existing add-a-team-member link must still resolve unchanged"
        );
    }

    #[test]
    fn render_owner_detail_html_contains_repo_table() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(detail_page.contains("alpha-repo"));
        assert!(!detail_page.contains("[private repo]"));
        assert!(detail_page.contains("beta-repo"));
        assert!(detail_page.contains("status-dot"));
        assert!(detail_page.contains("status-pass"));
        assert!(detail_page.contains("status-fail"));
    }

    #[test]
    fn render_owner_detail_html_contains_control_name_labels() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(detail_page.contains("Security Policy"));
        assert!(detail_page.contains("Secret Scanning"));
        assert!(detail_page.contains("Dependabot"));
        assert!(detail_page.contains("Branch Protection"));
    }

    /// UF2-7(c): the owner-detail Secret Scanning card carries generalized
    /// descriptive copy (answering "do we scan for leaked secrets?") that
    /// states its population is public-only, scoped to that ONE card via
    /// `SummaryCard::key` (not the human `label`, which could reword).
    #[test]
    fn render_owner_detail_html_secret_scanning_card_has_population_tooltip() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("Enable it under Settings → Security → Advanced Security"),
            "expected the secret-scanning how-to-fix tooltip on the owner detail page"
        );
        assert!(
            detail_page.contains("public repositories only"),
            "expected the secret-scanning tooltip to state the public-only population"
        );
        assert!(
            !detail_page.to_lowercase().contains("mattilsynet"),
            "generalized copy must not hardcode the org name (UF2-A seam)"
        );
    }

    #[test]
    fn render_owner_detail_html_has_data_driven_table_headers() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(detail_page.contains(
            "<th class=\"text-center\" data-nosort>Security Policy <span class=\"tooltip-trigger tooltip-trigger-header\" tabindex=\"0\" data-tooltip=\""
        ));
        assert!(detail_page.contains(">Dependabot Status <span class=\"tooltip-trigger"));
    }

    #[test]
    fn format_date_prefix_full_iso_timestamp() {
        assert_eq!(
            super::format_date_prefix(Some("2026-04-09T12:00:00+00:00")),
            "2026-04-09"
        );
    }

    #[test]
    fn format_date_prefix_date_only() {
        assert_eq!(super::format_date_prefix(Some("2026-04-09")), "2026-04-09");
    }

    #[test]
    fn format_date_prefix_none_returns_em_dash() {
        assert_eq!(super::format_date_prefix(None), "\u{2014}");
    }

    #[test]
    fn format_date_prefix_short_string_returns_em_dash() {
        assert_eq!(super::format_date_prefix(Some("2026")), "\u{2014}");
    }

    #[test]
    fn format_date_prefix_cross_date_boundary_offset() {
        assert_eq!(
            super::format_date_prefix(Some("2026-04-09T23:30:00-05:00")),
            "2026-04-10"
        );
    }

    #[test]
    fn format_date_prefix_multibyte_no_panic() {
        assert_eq!(
            super::format_date_prefix(Some("日本語タイムスタンプ")),
            "\u{2014}"
        );
    }

    #[test]
    fn detail_vm_repo_row_metadata_defaults_when_no_data() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        let row = &vm.repo_rows[0];

        assert_eq!(row.description, "\u{2014}");
        assert_eq!(row.language, "\u{2014}");
        assert!(!row.is_fork);
        assert_eq!(row.license, "\u{2014}");
        assert_eq!(row.pushed_at, "\u{2014}");
        assert_eq!(row.created_at, "\u{2014}");
        assert_eq!(row.last_committer_login, "\u{2014}");
        assert!(row.last_committer_url.is_empty());
        assert_eq!(row.last_commit_date, "\u{2014}");
        assert!(
            !row.last_committer_unregistered,
            "item9 Part A: no-commit-data must render the neutral dash, not the unregistered-user warning"
        );
    }

    #[test]
    fn detail_vm_repo_row_metadata_populated_with_data() {
        use crate::domain::evidence::LastCommitInfo;

        let mut repo = test_fixtures::make_repository_evidence(
            "rich-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-rich"]),
            ),
        );
        {
            let r = &mut repo.repository;
            r.description = Some("A test repo".to_string());
            r.language = Some("Rust".to_string());
            r.fork = true;
            r.license_spdx = Some("MIT".to_string());
            r.pushed_at = Some("2026-04-08T10:30:00Z".to_string());
            r.created_at = Some("2025-01-15T08:00:00Z".to_string());
        }
        repo.last_commit = Some(LastCommitInfo {
            committer_login: Some("octocat".to_string()),
            committer_name: Some("The Octocat".to_string()),
            commit_date: Some("2026-04-08T10:30:00Z".to_string()),
        });

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        assert_eq!(detail_vms.len(), 1);
        let (_, vm) = &detail_vms[0];
        let row = &vm.repo_rows[0];

        assert_eq!(row.description, "A test repo");
        assert_eq!(row.language, "Rust");
        assert!(row.is_fork);
        assert_eq!(row.license, "MIT");
        assert_eq!(row.pushed_at, "2026-04-08");
        assert_eq!(row.created_at, "2025-01-15");
        assert_eq!(row.last_committer_login, "The Octocat");
        assert_eq!(
            row.last_committer_url,
            format!("{}/octocat", config::DEFAULT_GITHUB_WEB_BASE_URL),
        );
        assert_eq!(row.last_commit_date, "2026-04-08");
        assert!(
            !row.last_committer_unregistered,
            "item9 Part A: a matched committer_login must not show the unregistered-user warning"
        );
    }

    /// item9 Part A test (a): a committer name is present but GitHub could
    /// not match the commit to any GitHub account (`committer_login:
    /// None`) — this must render the unregistered-user warning, distinct
    /// from both "matched" (registered) and "no commit data at all"
    /// (neutral dash, covered by `detail_vm_repo_row_metadata_defaults_when_no_data`).
    #[test]
    fn detail_vm_unregistered_committer_flagged_when_name_present_but_no_login_matched() {
        use crate::domain::evidence::LastCommitInfo;

        let mut repo = test_fixtures::make_repository_evidence(
            "unmatched-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-unmatched"]),
            ),
        );
        repo.last_commit = Some(LastCommitInfo {
            committer_login: None,
            committer_name: Some("Jane Doe".to_string()),
            commit_date: Some("2026-04-08T10:30:00Z".to_string()),
        });

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        let row = &vm.repo_rows[0];

        assert_eq!(row.last_committer_login, "Jane Doe");
        assert!(row.last_committer_url.is_empty());
        assert!(
            row.last_committer_unregistered,
            "a committer name with no matched GitHub login must be flagged unregistered"
        );
    }

    #[test]
    fn detail_vm_last_committer_url_percent_encodes_login() {
        use crate::domain::evidence::LastCommitInfo;

        let mut repo = test_fixtures::make_repository_evidence(
            "enc-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-enc"]),
            ),
        );
        repo.last_commit = Some(LastCommitInfo {
            committer_login: Some("user name".to_string()),
            committer_name: None,
            commit_date: None,
        });

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        let row = &vm.repo_rows[0];

        assert_eq!(row.last_committer_login, "user name");
        assert_eq!(
            row.last_committer_url,
            format!("{}/user%20name", config::DEFAULT_GITHUB_WEB_BASE_URL),
        );
    }

    #[test]
    fn render_owner_detail_html_stale_repo_has_row_stale_class() {
        let mut repo = test_fixtures::make_repository_evidence(
            "ancient-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-stale"]),
            ),
        );
        repo.repository.updated_at = Some("2023-01-01T00:00:00Z".to_string());

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("row-stale"),
            "stale repo should have row-stale CSS class"
        );
        assert!(
            detail_page.contains("not been updated in over 2 years"),
            "detail page should contain stale footnote"
        );
        assert!(
            detail_page.contains("Stale: not updated in 2+ years"),
            "stale row should carry a per-row tooltip explaining the colour"
        );
        assert!(
            detail_page.contains("Rows highlighted in pink have not been updated in over 2 years and may be abandoned."),
            "footnote should reword the colour to pink"
        );
    }

    #[test]
    fn render_owner_detail_html_recent_repo_no_row_stale_class() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            !detail_page.contains("row-stale"),
            "repos without updated_at should not be flagged stale"
        );
        assert!(
            !detail_page.contains("not been updated in over 2 years"),
            "stale footnote should not appear when no repos are stale"
        );
        assert!(
            !detail_page.contains("Stale: not updated in 2+ years"),
            "stale row tooltip should not appear when no repos are stale"
        );
    }

    /// item9 Part A render-level test: an unregistered committer (name
    /// present, no matched GitHub login) shows the warning tooltip on the
    /// owner-detail Repositories table; a repo with no commit data at all
    /// (the `evidence_with_owner_repos` fixture) shows neither the warning
    /// nor a stray tooltip.
    #[test]
    fn render_owner_detail_html_unregistered_committer_shows_warning_badge() {
        use crate::domain::evidence::LastCommitInfo;

        let mut repo = test_fixtures::make_repository_evidence(
            "unmatched-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-unmatched"]),
            ),
        );
        repo.last_commit = Some(LastCommitInfo {
            committer_login: None,
            committer_name: Some("Jane Doe".to_string()),
            commit_date: Some("2026-04-08T10:30:00Z".to_string()),
        });

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(detail_page.contains("Jane Doe"));
        assert!(
            detail_page.contains("unregistered/unknown GitHub user")
                || detail_page.contains("could not be matched to a GitHub account"),
            "expected an unregistered-committer warning tooltip; got: {detail_page}"
        );
    }

    /// Empty repo (`size:0`, `is_empty` derived at the collector boundary)
    /// exercises the empty-repo pill in the owner detail table
    /// (adr-fmt-nvf8w).
    #[test]
    fn render_owner_detail_html_empty_repo_shows_pill_snapshot() {
        let mut repo = test_fixtures::make_repository_evidence(
            "empty-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-empty"]),
            ),
        );
        repo.repository.is_empty = true;

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains(r#"<span class="repo-badge-empty">empty</span>"#),
            "expected empty-repo pill markup; got: {detail_page}"
        );

        insta::assert_snapshot!("dashboard_owner_detail_empty_repo_badge", detail_page);
    }

    #[test]
    fn render_owner_detail_html_no_warning_badge_when_no_commit_data() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            !detail_page.contains("could not be matched to a GitHub account"),
            "no-commit-data (neutral dash) must not show the unregistered-committer warning"
        );
    }

    #[test]
    fn render_owner_detail_html_contains_metadata_headers() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(detail_page.contains(">Description <span class=\"tooltip-trigger"));
        assert!(detail_page.contains(">Language <span class=\"tooltip-trigger"));
        assert!(detail_page.contains(
            "<th class=\"text-center\" data-sort-type=\"text\">Fork <span class=\"tooltip-trigger tooltip-trigger-header\" tabindex=\"0\" data-tooltip=\"Yes if this repository is a fork of another repository.\">ⓘ</span></th>"
        ));
        assert!(detail_page.contains(">License <span class=\"tooltip-trigger"));
        assert!(detail_page.contains(">Last Push <span class=\"tooltip-trigger"));
        assert!(detail_page.contains(">Created <span class=\"tooltip-trigger"));
        assert!(detail_page.contains(">Last Committer <span class=\"tooltip-trigger"));
        assert!(detail_page.contains(">Last Commit <span class=\"tooltip-trigger"));
    }

    #[test]
    fn render_owner_detail_html_contains_visibility_header() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains(">Visibility <span class=\"tooltip-trigger"),
            "owner detail page should contain Visibility column header"
        );
        assert!(
            detail_page.contains("Private"),
            "owner detail page should show Private visibility"
        );
        assert!(
            detail_page.contains("Public"),
            "owner detail page should show Public visibility"
        );
    }

    #[test]
    fn detail_vm_repo_rows_have_visibility_field() {
        let evidence = evidence_with_owner_repos();
        let owner_repo_map = crate::domain::metrics::build_owner_repo_map(&evidence.repositories);
        let detail_vms = build_owner_detail_view_models(
            &evidence.metrics.owner_metrics,
            &owner_repo_map,
            &CoverageTiers::default(),
            &evidence.assessment_metadata.organization,
            &evidence.assessment_metadata.run_timestamp,
            &[],
            &[],
        );

        let (_, vm) = &detail_vms[0];
        assert_eq!(vm.repo_rows[0].visibility, "Private");
        assert_eq!(vm.repo_rows[1].visibility, "Public");
    }

    #[test]
    fn build_repo_display_internal_shows_real_name() {
        let repo = test_fixtures::make_repository_evidence(
            "internal-repo",
            Visibility::Internal,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_conforming(),
            ),
        );

        let org_encoded = utf8_percent_encode("TestOrg", PATH_SEGMENT).to_string();
        let name_encoded = utf8_percent_encode(&repo.repository.name, PATH_SEGMENT);
        let (name, url) = build_repo_display(&repo, &org_encoded, &name_encoded);

        assert_eq!(name, "internal-repo");
        assert_eq!(
            url,
            format!(
                "{}/TestOrg/internal-repo",
                config::DEFAULT_GITHUB_WEB_BASE_URL
            ),
        );
    }

    #[test]
    fn render_owner_detail_html_wraps_table_for_scroll() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("table-wrapper"),
            "table should be wrapped in a scrollable container"
        );
    }

    #[test]
    fn render_owner_detail_html_escapes_metadata_xss() {
        use crate::domain::evidence::LastCommitInfo;

        let mut repo = test_fixtures::make_repository_evidence(
            "xss-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-xss"]),
            ),
        );
        repo.repository.description = Some("<img onerror=alert(1)>".to_string());
        repo.last_commit = Some(LastCommitInfo {
            committer_login: Some("<b>bold</b>".to_string()),
            committer_name: Some("<i>italic</i>".to_string()),
            commit_date: Some("2026-04-08T10:00:00Z".to_string()),
        });

        let repos = vec![repo];
        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            !detail_page.contains("<img onerror=alert(1)>"),
            "raw img tag must be escaped in description"
        );
        assert!(
            !detail_page.contains("<i>italic</i>"),
            "raw italic tag must be escaped in committer name"
        );
        assert!(
            !detail_page.contains("<b>bold</b>"),
            "raw bold tag must be escaped in committer login"
        );
    }

    #[test]
    fn is_orphaned_absent_codeowners() {
        let repo = test_fixtures::make_repository_evidence(
            "no-codeowners",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        assert!(super::is_orphaned(&repo));
    }

    #[test]
    fn is_orphaned_unknown_codeowners_not_orphaned() {
        let repo = test_fixtures::make_repository_evidence(
            "unknown-codeowners",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_unknown(),
            ),
        );
        assert!(!super::is_orphaned(&repo));
    }

    #[test]
    fn is_orphaned_conforming_with_parsed_none_not_orphaned() {
        let repo = test_fixtures::all_passing_evidence("conforming-repo");
        assert!(!super::is_orphaned(&repo));
    }

    #[test]
    fn is_orphaned_conforming_with_empty_owners() {
        let repo = test_fixtures::make_repository_evidence(
            "empty-owners",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                CodeownersResult {
                    status: CodeownersStatus::Conforming,
                    path: Some(".github/CODEOWNERS".to_string()),
                    timestamp: test_fixtures::make_timestamp(),
                    parsed: Some(ParsedCodeowners {
                        entries: vec![],
                        unique_owners: vec![],
                        skipped_lines: 0,
                    }),
                    truncation: None,
                },
            ),
        );
        assert!(super::is_orphaned(&repo));
    }

    #[test]
    fn is_orphaned_conforming_with_owners_not_orphaned() {
        let repo = test_fixtures::make_repository_evidence(
            "has-owners",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/team-a"]),
            ),
        );
        assert!(!super::is_orphaned(&repo));
    }

    #[test]
    fn is_orphaned_non_conforming_with_empty_owners() {
        let repo = test_fixtures::make_repository_evidence(
            "non-conf-empty",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                CodeownersResult {
                    status: CodeownersStatus::NonConforming,
                    path: Some("CODEOWNERS".to_string()),
                    timestamp: test_fixtures::make_timestamp(),
                    parsed: Some(ParsedCodeowners {
                        entries: vec![],
                        unique_owners: vec![],
                        skipped_lines: 0,
                    }),
                    truncation: None,
                },
            ),
        );
        assert!(super::is_orphaned(&repo));
    }

    #[test]
    fn build_orphaned_vm_sorts_by_committer_then_name() {
        use crate::domain::evidence::LastCommitInfo;

        let mut repo_a = test_fixtures::make_repository_evidence(
            "zeta-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        repo_a.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: Some("Alice".to_string()),
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });

        let mut repo_b = test_fixtures::make_repository_evidence(
            "alpha-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        repo_b.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: Some("Alice".to_string()),
            commit_date: Some("2026-04-02T00:00:00Z".to_string()),
        });

        let mut repo_c = test_fixtures::make_repository_evidence(
            "beta-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        repo_c.last_commit = Some(LastCommitInfo {
            committer_login: Some("bob".to_string()),
            committer_name: None,
            commit_date: None,
        });

        let repos = vec![repo_a, repo_b, repo_c];
        let vm =
            super::build_orphaned_view_model(&repos, "TestOrg", "2026-04-09T12:00:00+00:00", &[]);

        assert_eq!(vm.rows.len(), 3);
        assert_eq!(vm.rows[0].repo_name, "alpha-repo");
        assert_eq!(vm.rows[0].last_committer_login, "Alice");
        assert_eq!(vm.rows[1].repo_name, "zeta-repo");
        assert_eq!(vm.rows[1].last_committer_login, "Alice");
        assert_eq!(vm.rows[2].repo_name, "beta-repo");
        assert_eq!(vm.rows[2].last_committer_login, "bob");
    }

    /// item9 Part A test (a), orphans view model: mirrors the owner-detail
    /// coverage above (`detail_vm_unregistered_committer_flagged_when_name_present_but_no_login_matched`)
    /// at the `build_orphaned_view_model` seam — a committer name present
    /// with no matched login is flagged; a matched login is not.
    #[test]
    fn build_orphaned_vm_flags_unregistered_committer_only_when_name_present_and_login_absent() {
        use crate::domain::evidence::LastCommitInfo;

        let mut unregistered = test_fixtures::make_repository_evidence(
            "unregistered-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        unregistered.last_commit = Some(LastCommitInfo {
            committer_login: None,
            committer_name: Some("Jane Doe".to_string()),
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });

        let mut registered = test_fixtures::make_repository_evidence(
            "registered-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        registered.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: Some("Alice".to_string()),
            commit_date: Some("2026-04-02T00:00:00Z".to_string()),
        });

        let repos = vec![unregistered, registered];
        let vm =
            super::build_orphaned_view_model(&repos, "TestOrg", "2026-04-09T12:00:00+00:00", &[]);

        let unregistered_row = vm
            .rows
            .iter()
            .find(|r| r.repo_name == "unregistered-repo")
            .expect("unregistered-repo row present");
        assert!(unregistered_row.last_committer_url.is_empty());
        assert!(
            unregistered_row.last_committer_unregistered,
            "committer name present with no matched login must be flagged unregistered"
        );

        let registered_row = vm
            .rows
            .iter()
            .find(|r| r.repo_name == "registered-repo")
            .expect("registered-repo row present");
        assert!(
            !registered_row.last_committer_unregistered,
            "a matched committer_login must not be flagged unregistered"
        );
    }

    /// B2: an orphan repo is attributed to the team whose roster lists its
    /// last committer, matched by the raw GitHub login (not the display
    /// name) so `TeamMember.login` comparisons are unaffected by
    /// `committer_name` formatting.
    #[test]
    fn build_orphaned_vm_attributes_team_via_last_committer_login() {
        use crate::domain::evidence::LastCommitInfo;
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut orphan = test_fixtures::make_repository_evidence(
            "orphan-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        orphan.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: Some("Alice Anderson".to_string()),
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });

        let mut unattributed = test_fixtures::make_repository_evidence(
            "unattributed-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        unattributed.last_commit = Some(LastCommitInfo {
            committer_login: Some("someone-else".to_string()),
            committer_name: None,
            commit_date: None,
        });

        let team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Maintainer,
                in_org: None,
            }],
        }];

        let vm = super::build_orphaned_view_model(
            &[orphan, unattributed],
            "TestOrg",
            "2026-04-09T12:00:00+00:00",
            &team_rosters,
        );

        let orphan_row = vm
            .rows
            .iter()
            .find(|r| r.repo_name == "orphan-repo")
            .expect("orphan-repo present");
        assert_eq!(
            orphan_row.attributed_team.as_deref(),
            Some("@org/team-a"),
            "matched via raw login 'alice', not display name 'Alice Anderson'"
        );

        let unattributed_row = vm
            .rows
            .iter()
            .find(|r| r.repo_name == "unattributed-repo")
            .expect("unattributed-repo present");
        assert_eq!(unattributed_row.attributed_team, None);

        assert_eq!(vm.by_team.len(), 1);
        assert_eq!(vm.by_team[0].team, "@org/team-a");
        assert_eq!(vm.by_team[0].rows.len(), 1);
        assert_eq!(vm.by_team[0].rows[0].repo_name, "orphan-repo");
    }

    /// B2: the "Orphans by Team" section renders in the HTML page.
    #[test]
    fn render_orphaned_html_contains_orphans_by_team_section() {
        use crate::domain::evidence::LastCommitInfo;
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut orphan = test_fixtures::make_repository_evidence(
            "orphan-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        orphan.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: None,
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });

        let mut evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            crate::aggregate::metrics::build_collection_statistics(&[orphan.clone()]),
            crate::aggregate::metrics::aggregate_metrics(&[orphan.clone()]),
            test_fixtures::make_observability(),
            vec![orphan],
        );
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Maintainer,
                in_org: None,
            }],
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let orphans_html = &pages["orphans.html"];

        assert!(
            orphans_html.contains("Orphans by Team"),
            "expected the B2 orphans-by-team section"
        );
        assert!(orphans_html.contains("owners/org-team-a.html"));
        assert!(orphans_html.contains("orphan-repo"));
    }

    /// item9 Part A render-level test, orphans.html: an unregistered
    /// committer (name present, no matched login) shows the warning
    /// tooltip on the top-level orphans table.
    #[test]
    fn render_orphaned_html_unregistered_committer_shows_warning_badge() {
        use crate::domain::evidence::LastCommitInfo;

        let mut orphan = test_fixtures::make_repository_evidence(
            "orphan-unmatched-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        orphan.last_commit = Some(LastCommitInfo {
            committer_login: None,
            committer_name: Some("Jane Doe".to_string()),
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });

        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            crate::aggregate::metrics::build_collection_statistics(&[orphan.clone()]),
            crate::aggregate::metrics::aggregate_metrics(&[orphan.clone()]),
            test_fixtures::make_observability(),
            vec![orphan],
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let orphans_html = &pages["orphans.html"];

        assert!(orphans_html.contains("Jane Doe"));
        assert!(
            orphans_html.contains("could not be matched to a GitHub account"),
            "expected an unregistered-committer warning tooltip on orphans.html; got: {orphans_html}"
        );
    }

    #[test]
    fn render_orphaned_html_stale_repo_has_stale_marker_and_footnote() {
        use crate::domain::evidence::LastCommitInfo;
        use crate::domain::metrics::{TeamMember, TeamMemberRole, TeamRoster, TeamRosterStatus};

        let mut orphan = test_fixtures::make_repository_evidence(
            "ancient-orphan-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        orphan.repository.updated_at = Some("2023-01-01T00:00:00Z".to_string());
        orphan.last_commit = Some(LastCommitInfo {
            committer_login: Some("alice".to_string()),
            committer_name: None,
            commit_date: Some("2026-04-01T00:00:00Z".to_string()),
        });

        let mut evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            crate::aggregate::metrics::build_collection_statistics(&[orphan.clone()]),
            crate::aggregate::metrics::aggregate_metrics(&[orphan.clone()]),
            test_fixtures::make_observability(),
            vec![orphan],
        );
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Complete,
            members: vec![TeamMember {
                login: "alice".to_string(),
                role: TeamMemberRole::Maintainer,
                in_org: None,
            }],
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let orphans_html = &pages["orphans.html"];

        assert!(
            orphans_html.contains("row-stale"),
            "stale orphan repo should have row-stale CSS class"
        );
        assert!(
            orphans_html
                .matches("Stale: not updated in 2+ years")
                .count()
                >= 2,
            "stale marker tooltip should render in both the main table and the by-team table"
        );
        assert!(
            orphans_html.contains(
                "Rows highlighted in pink have not been updated in over 2 years and may be abandoned."
            ),
            "footnote should reword the colour to pink and keep the 2-year explanation"
        );
        assert!(
            !orphans_html.contains("highlighted in orange"),
            "stale footnote must not reference the old, incorrect colour word"
        );
    }

    #[test]
    fn build_orphaned_vm_shows_private_repos() {
        let repo = test_fixtures::make_repository_evidence(
            "secret-repo",
            Visibility::Private,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );

        let vm =
            super::build_orphaned_view_model(&[repo], "TestOrg", "2026-04-09T12:00:00+00:00", &[]);

        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].repo_name, "secret-repo");
        assert_eq!(
            vm.rows[0].repo_url,
            format!(
                "{}/TestOrg/secret-repo",
                config::DEFAULT_GITHUB_WEB_BASE_URL
            ),
        );
        assert_eq!(vm.rows[0].visibility, "Private");
    }

    #[test]
    fn build_orphaned_vm_excludes_archived_repos() {
        let active = test_fixtures::make_repository_evidence(
            "active-orphan",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );
        let archived = test_fixtures::make_repository_evidence(
            "archived-orphan",
            Visibility::Public,
            true,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );

        let vm = super::build_orphaned_view_model(
            &[active, archived],
            "TestOrg",
            "2026-04-09T12:00:00+00:00",
            &[],
        );

        assert_eq!(vm.orphaned_count, 1);
        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].repo_name, "active-orphan");
    }

    #[test]
    fn build_orphaned_vm_empty_when_no_orphans() {
        let repo = test_fixtures::all_passing_evidence("owned-repo");
        let vm =
            super::build_orphaned_view_model(&[repo], "TestOrg", "2026-04-09T12:00:00+00:00", &[]);

        assert!(vm.rows.is_empty());
        assert_eq!(vm.orphaned_count, 0);
        assert!(!vm.has_stale_repos);
    }

    #[test]
    fn render_dashboard_orphaned_page_exists() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        assert!(pages.contains_key("orphans.html"));
        let orphaned = &pages["orphans.html"];
        assert!(orphaned.contains("Orphan Repositories"));
        assert!(orphaned.contains("No orphan repositories found"));
    }

    #[test]
    fn render_dashboard_orphaned_page_shows_absent_repos() {
        let repos = vec![
            test_fixtures::make_repository_evidence(
                "orphan-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_absent(),
                ),
            ),
            test_fixtures::all_passing_evidence("owned-repo"),
        ];

        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let orphaned = &pages["orphans.html"];

        assert!(orphaned.contains("orphan-repo"));
        assert!(!orphaned.contains("owned-repo"));
        assert!(orphaned.contains("Orphans (1)"));
    }

    #[test]
    fn render_dashboard_deleted_page_shows_pruned_deleted_repos() {
        let mut evidence = sample_evidence();
        evidence.deleted = vec![crate::projection::DeletedRepoRecord {
            repo_name: "deleted-repo".to_string(),
            detected_at: "2026-06-24T00:00:00Z".to_string(),
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let deleted = &pages["deleted.html"];
        assert!(deleted.contains("Deleted Repositories"));
        assert!(deleted.contains("deleted-repo"));
        assert!(deleted.contains("2026-06-24T00:00:00Z"));
        assert!(!deleted.contains("Security Policy"));
    }

    /// Falsifier for the canonical-vs-bare-slug join: `team_rosters` entries
    /// key their referencing-repos lookup by the full lowercased canonical
    /// owner (`@org/dead-team`), not the bare GitHub API `team_slug`
    /// (`dead-team`) that the roster itself also carries. Keying on the
    /// bare slug returns no match and this test fails.
    #[test]
    fn build_deleted_view_model_includes_deleted_team_with_referencing_repos() {
        let repos = vec![test_fixtures::make_repository_evidence(
            "codeowners-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/dead-team"]),
            ),
        )];
        let team_rosters = vec![TeamRoster {
            canonical_owner: "@org/dead-team".to_string(),
            team_slug: "dead-team".to_string(),
            status: TeamRosterStatus::Deleted,
            members: Vec::new(),
        }];

        let vm = build_deleted_view_model(&[], "TestOrg", &repos, &team_rosters);

        assert_eq!(
            vm.deleted_teams.len(),
            1,
            "expected exactly one deleted team"
        );
        let row = &vm.deleted_teams[0];
        assert_eq!(row.team_slug, "dead-team");
        assert!(
            row.referencing_repos
                .contains(&"codeowners-repo".to_string()),
            "expected dead-team's referencing repos to include codeowners-repo \
             (joined via canonical_owner, not bare team_slug); got {:?}",
            row.referencing_repos
        );
        assert!(
            row.team_url.contains("dead-team"),
            "expected team_url to reference the bare team slug; got {}",
            row.team_url
        );
    }

    #[test]
    fn build_deleted_view_model_omits_deleted_teams_when_none_are_deleted() {
        let repos = vec![test_fixtures::make_repository_evidence(
            "codeowners-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@org/live-team"]),
            ),
        )];
        let team_rosters = vec![TeamRoster {
            canonical_owner: "@org/live-team".to_string(),
            team_slug: "live-team".to_string(),
            status: TeamRosterStatus::Complete,
            members: Vec::new(),
        }];

        let vm = build_deleted_view_model(&[], "TestOrg", &repos, &team_rosters);

        assert!(vm.deleted_teams.is_empty());
    }

    #[test]
    fn render_dashboard_deleted_page_omits_deleted_teams_section_when_none() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let deleted = &pages["deleted.html"];

        assert!(
            !deleted.contains("Deleted Teams"),
            "expected the Deleted Teams section to be omitted entirely when \
             there are no deleted teams; got:\n{deleted}"
        );
    }

    #[test]
    fn render_dashboard_deleted_page_lists_deleted_team_with_referencing_repo() {
        let mut evidence = evidence_with_owner_repos();
        evidence.metrics.team_rosters = vec![TeamRoster {
            canonical_owner: "@org/team-a".to_string(),
            team_slug: "team-a".to_string(),
            status: TeamRosterStatus::Deleted,
            members: Vec::new(),
        }];

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let deleted = &pages["deleted.html"];

        assert!(
            deleted.contains("Deleted Teams"),
            "expected the Deleted Teams section to render; got:\n{deleted}"
        );
        assert!(deleted.contains("team-a"));
        assert!(deleted.contains("beta-repo"));
        assert!(deleted.contains("alpha-repo"));
        assert!(
            deleted.contains("https://github.com/orgs/TestOrg/teams/team-a"),
            "expected the team row to link to the GitHub team page; got:\n{deleted}"
        );
    }

    #[test]
    fn render_orphaned_html_contains_visibility_header() {
        let repos = vec![test_fixtures::make_repository_evidence(
            "orphan-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        )];

        let metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let orphaned = &pages["orphans.html"];

        assert!(
            orphaned.contains(">Visibility <span class=\"tooltip-trigger"),
            "orphaned page should contain Visibility column header"
        );
        assert!(
            orphaned.contains("Public"),
            "orphaned page should show Public visibility label"
        );
    }

    #[test]
    fn build_orphaned_vm_has_visibility_field() {
        let repo = test_fixtures::make_repository_evidence(
            "pub-orphan",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_absent(),
            ),
        );

        let vm =
            super::build_orphaned_view_model(&[repo], "TestOrg", "2026-04-09T12:00:00+00:00", &[]);

        assert_eq!(vm.rows.len(), 1);
        assert_eq!(vm.rows[0].visibility, "Public");
    }

    #[test]
    fn render_dashboard_nav_contains_orphaned_link() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let index = &pages["index.html"];
        assert!(
            index.contains("Orphans ("),
            "index.html should have orphans link"
        );

        let report = &pages["report.html"];
        assert!(
            report.contains("Orphans ("),
            "report.html should have orphans link"
        );

        let orphaned = &pages["orphans.html"];
        assert!(
            orphaned.contains("Orphans ("),
            "orphans.html should have orphans link"
        );
    }

    #[test]
    fn repo_score_all_passing() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        let (score, fmt, tier, wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(100.0));
        assert_eq!(fmt, "100.0%");
        assert_eq!(tier, CoverageTier::Pass);
        assert_eq!(wc, "w-100");
    }

    #[test]
    fn repo_score_all_failing() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Fail,
            SecretScanningStatus::Disabled,
            DependabotStatus::Disabled,
            BranchProtectionStatus::Fail,
        );
        let (score, _fmt, tier, _wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(20.0));
        assert_eq!(tier, CoverageTier::Fail);
    }

    #[test]
    fn repo_score_all_unknown_returns_na() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        let mut checks = checks;
        checks.codeowners.status = CodeownersStatus::Unknown;

        let (score, fmt, tier, wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, None);
        assert_eq!(fmt, "N/A");
        assert_eq!(tier, CoverageTier::Na);
        assert_eq!(wc, "w-0");
    }

    #[test]
    fn repo_score_mixed_with_unknowns_excluded() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Unknown,
            DependabotStatus::Disabled,
            BranchProtectionStatus::Unknown,
        );
        let (score, _fmt, tier, _wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        let s = score.unwrap();
        assert!((s - 66.7).abs() < 0.1, "expected ~66.7, got {s}");
        assert_eq!(tier, CoverageTier::Warn);
    }

    #[test]
    fn repo_score_paused_and_partial_count_as_fail() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Paused,
            BranchProtectionStatus::Partial,
        );
        let (score, _fmt, _tier, _wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(60.0));
    }

    #[test]
    fn repo_score_secret_scanning_permission_denied_excluded() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::PermissionDenied,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        let (score, fmt, tier, wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(100.0));
        assert_eq!(fmt, "100.0%");
        assert_eq!(tier, CoverageTier::Pass);
        assert_eq!(wc, "w-100");
    }

    #[test]
    fn repo_score_codeowners_non_conforming_counts_as_fail() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        checks.codeowners.status = CodeownersStatus::NonConforming;
        let (score, fmt, tier, _wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(80.0));
        assert_eq!(fmt, "80.0%");
        assert_eq!(tier, CoverageTier::Pass);
    }

    #[test]
    fn repo_score_codeowners_absent_counts_as_fail() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        checks.codeowners.status = CodeownersStatus::Absent;
        let (score, _fmt, _tier, _wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(80.0));
    }

    #[test]
    fn repo_score_all_controls_fail() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Fail,
            SecretScanningStatus::Disabled,
            DependabotStatus::Disabled,
            BranchProtectionStatus::Fail,
        );
        checks.codeowners.status = CodeownersStatus::Absent;
        let (score, fmt, tier, wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(0.0));
        assert_eq!(fmt, "0.0%");
        assert_eq!(tier, CoverageTier::Fail);
        assert_eq!(wc, "w-0");
    }

    #[test]
    fn repo_score_single_deterministic_control_pass() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        checks.codeowners.status = CodeownersStatus::Unknown;
        let (score, fmt, tier, wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(100.0));
        assert_eq!(fmt, "100.0%");
        assert_eq!(tier, CoverageTier::Pass);
        assert_eq!(wc, "w-100");
    }

    #[test]
    fn repo_score_single_deterministic_control_fail() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Paused,
            BranchProtectionStatus::Unknown,
        );
        checks.codeowners.status = CodeownersStatus::Unknown;
        let (score, fmt, tier, wc) = super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(0.0));
        assert_eq!(fmt, "0.0%");
        assert_eq!(tier, CoverageTier::Fail);
        assert_eq!(wc, "w-0");
    }

    #[test]
    fn repo_score_width_class_for_boundary_values() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Enabled,
            DependabotStatus::Paused,
            BranchProtectionStatus::Partial,
        );
        let (_score, _fmt, _tier, wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(wc, "w-60");

        let mut checks2 = make_checks_with_statuses(
            SecurityPolicyStatus::Fail,
            SecretScanningStatus::Disabled,
            DependabotStatus::Disabled,
            BranchProtectionStatus::Pass,
        );
        checks2.codeowners.status = CodeownersStatus::NonConforming;
        let (score2, _fmt2, _tier2, wc2) =
            super::compute_repo_score(&checks2, &CoverageTiers::default());
        assert_eq!(score2, Some(20.0));
        assert_eq!(wc2, "w-20");
    }

    #[test]
    fn repo_score_width_class_rounds_non_boundary() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Pass,
            SecretScanningStatus::Unknown,
            DependabotStatus::Disabled,
            BranchProtectionStatus::Unknown,
        );
        let (score, _fmt, _tier, wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        let s = score.unwrap();
        assert!((s - 66.7).abs() < 0.1, "expected ~66.7, got {s}");
        assert_eq!(wc, "w-65");
    }

    #[test]
    fn owner_sec_score_computed_in_overview() {
        let evidence = evidence_with_owner_repos();
        let owners_vm = super::build_owners_view_model(
            &evidence.metrics.owner_metrics,
            &CoverageTiers::default(),
        )
        .expect("should have owner metrics");

        assert!(!owners_vm.rows.is_empty());
        let row = &owners_vm.rows[0];

        assert!(row.sec_score.is_some(), "sec_score should be Some");
        assert!(row.sec_score_formatted.contains('%'));
        assert_ne!(row.sec_score_width_class, "w-0");
    }

    #[test]
    fn status_dots_pending_repos_render_pending() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        checks.secret_scanning.reason = Some("pending".to_string());
        checks.dependabot_security_updates.reason = Some("pending".to_string());
        checks.branch_protection.details.reason = Some("pending".to_string());

        let dots = build_status_dots(&checks);

        for dot in &dots {
            assert_eq!(
                dot.css_class, "status-pending",
                "dot '{}' should be status-pending, got {}",
                dot.label, dot.css_class
            );
            assert_eq!(
                dot.label, "Pending",
                "dot label should be 'Pending', got '{}'",
                dot.label
            );
        }
    }

    #[test]
    fn render_owner_detail_html_contains_repo_posture_header() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("Repo Posture"),
            "owner detail page should contain 'Repo Posture' header, not bare 'Score'"
        );
        assert!(
            !detail_page.contains("Repo Score"),
            "the old 'Repo Score' label must be fully replaced (item6-04)"
        );
        assert!(
            !detail_page.contains("<th class=\"text-center\">Score</th>"),
            "owner detail page should not have bare 'Score' column header"
        );
    }

    #[test]
    fn render_owner_detail_html_repo_posture_tooltip_states_formula_and_exclusion_rule() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("Arithmetic mean: 100 × passing / measured controls"),
            "Repo Posture tooltip must state its exact formula; detail page:\n{detail_page}"
        );
        assert!(
            detail_page.contains(
                "Security Policy, Secret Scanning, Dependabot, Branch Protection, CODEOWNERS"
            ),
            "Repo Posture tooltip must state its five-control set"
        );
        assert!(
            detail_page.contains("excluded from the denominator"),
            "Repo Posture tooltip must state the exclusion rule"
        );
        assert!(
            detail_page.contains("Unlike the owner-level Team Health score"),
            "Repo Posture tooltip must disambiguate from Team Health using the new owner score name"
        );
        assert!(
            !detail_page.contains("Sec Score"),
            "the old 'Sec Score' name must be fully replaced (item6-04)"
        );
    }

    #[test]
    fn render_owner_detail_html_stale_repos_card_disambiguates_freshness_from_archival_coverage() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let detail_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/"))
            .expect("expected an owner detail page")
            .1;

        assert!(
            detail_page.contains("Stale Repos"),
            "Stale Repos card label must be unchanged (no value/label change for this card)"
        );
        assert!(
            detail_page.contains("(total - stale) / total"),
            "Stale Repos card tooltip must state the Freshness control's exact formula; detail page:\n{detail_page}"
        );
        assert!(
            detail_page.contains("Freshness"),
            "Stale Repos card tooltip must name the Freshness control it disambiguates from"
        );
        assert!(
            detail_page.contains("Distinct from the org-wide Archival Coverage"),
            "Stale Repos card tooltip must explicitly disambiguate from the org-level Archival Coverage metric"
        );
    }

    #[test]
    fn status_dots_not_applicable_renders_na() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::NotApplicable,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        checks.security_policy.evidence = SecurityPolicyEvidence::NotApplicable;

        let dots = build_status_dots(&checks);

        assert_eq!(dots[0].css_class, "status-na");
        assert_eq!(dots[0].label, "N/A");
        assert_eq!(dots[1].css_class, "status-pass");
    }

    #[test]
    fn repo_score_not_applicable_excluded_from_denominator() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::NotApplicable,
            SecretScanningStatus::Enabled,
            DependabotStatus::Enabled,
            BranchProtectionStatus::Pass,
        );
        checks.security_policy.evidence = SecurityPolicyEvidence::NotApplicable;
        let (score, _fmt, tier, _wc) =
            super::compute_repo_score(&checks, &CoverageTiers::default());
        assert_eq!(score, Some(100.0));
        assert_eq!(tier, CoverageTier::Pass);
    }

    #[test]
    fn is_pending_repo_positive() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        checks.secret_scanning.reason = Some("pending".to_string());
        assert!(super::is_pending_repo(&checks));
    }

    #[test]
    fn is_pending_repo_negative_collection_error() {
        let mut checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        checks.secret_scanning.reason = Some("collection_error".to_string());
        assert!(!super::is_pending_repo(&checks));
    }

    #[test]
    fn is_pending_repo_negative_none() {
        let checks = make_checks_with_statuses(
            SecurityPolicyStatus::Unknown,
            SecretScanningStatus::Unknown,
            DependabotStatus::Unknown,
            BranchProtectionStatus::Unknown,
        );
        assert!(!super::is_pending_repo(&checks));
    }

    #[test]
    fn control_display_name_non_stale() {
        assert_eq!(super::control_display_name("non_stale"), "Freshness");
    }

    #[test]
    fn control_display_name_alert_free() {
        assert_eq!(super::control_display_name("alert_free"), "Alert-Free");
    }

    #[test]
    fn control_display_name_unknown_key() {
        assert_eq!(super::control_display_name("bogus"), "Unknown");
    }

    /// Helper to build evidence with specific owners and types.
    fn evidence_with_mixed_owner_types() -> Evidence {
        let repos = vec![
            test_fixtures::make_repository_evidence(
                "team-repo-1",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/security-team"]),
                ),
            ),
            test_fixtures::make_repository_evidence(
                "user-repo-1",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@alice"]),
                ),
            ),
            test_fixtures::make_repository_evidence(
                "team-repo-2",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/infra-team"]),
                ),
            ),
            test_fixtures::make_repository_evidence(
                "team-repo-3",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_fail(),
                    test_fixtures::secret_disabled(),
                    test_fixtures::dependabot_disabled(),
                    test_fixtures::branch_fail(),
                    test_fixtures::codeowners_with_owners(&["@org/dev-team"]),
                ),
            ),
        ];

        let mut metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        crate::aggregate::metrics::enrich_owner_metrics_with_lifecycle(
            &mut metrics.owner_metrics,
            &repos,
            &test_fixtures::make_timestamp(),
        );
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);

        test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        )
    }

    #[test]
    fn podium_excludes_user_owners() {
        let evidence = evidence_with_mixed_owner_types();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            !index.contains("alice"),
            "podium should not contain user owner @alice"
        );
        assert!(
            index.contains("security-team") || index.contains("infra-team"),
            "podium should contain at least one team owner"
        );
    }

    /// item9 Part B render-level test: a departed (`in_org=Some(false)`)
    /// individual-user CODEOWNERS owner shows the warning tooltip on
    /// their own owner-detail H1 (the sole GitHub-profile-link site for
    /// user-type owners — `owners.html`'s overview table links only to
    /// this internal detail page, never directly to GitHub).
    #[test]
    fn render_owner_detail_html_departed_individual_user_owner_shows_warning_badge() {
        use crate::domain::metrics::OwnerType;

        let mut evidence = evidence_with_mixed_owner_types();
        for owner in &mut evidence.metrics.owner_metrics {
            if owner.owner_type == OwnerType::User {
                owner.in_org = Some(false);
            }
        }

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let alice_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/") && k.contains("alice"))
            .expect("expected an owner detail page for @alice")
            .1;

        assert!(
            alice_page.contains("No longer a member of this GitHub organisation."),
            "departed individual-user owner must show the warning tooltip on their H1; got: {alice_page}"
        );
    }

    /// item9 Part B render-level test: an individual-user owner confirmed
    /// present (`Some(true)`) shows no warning.
    #[test]
    fn render_owner_detail_html_present_individual_user_owner_shows_no_warning_badge() {
        use crate::domain::metrics::OwnerType;

        let mut evidence = evidence_with_mixed_owner_types();
        for owner in &mut evidence.metrics.owner_metrics {
            if owner.owner_type == OwnerType::User {
                owner.in_org = Some(true);
            }
        }

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let alice_page = pages
            .iter()
            .find(|(k, _)| k.starts_with("owners/") && k.contains("alice"))
            .expect("expected an owner detail page for @alice")
            .1;

        assert!(
            !alice_page.contains("No longer a member of this GitHub organisation."),
            "present individual-user owner must not show the departed warning"
        );
    }

    #[test]
    fn podium_gold_in_center_position() {
        let evidence = evidence_with_mixed_owner_types();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("rank-gold"),
            "podium should contain rank-gold class"
        );

        if let (Some(silver_pos), Some(gold_pos)) =
            (index.find("rank-silver"), index.find("rank-gold"))
        {
            assert!(
                silver_pos < gold_pos,
                "Silver should appear before Gold in HTML (visual order: Silver, Gold, Bronze)"
            );
        }

        if let (Some(gold_pos), Some(bronze_pos)) =
            (index.find("rank-gold"), index.find("rank-bronze"))
        {
            assert!(
                gold_pos < bronze_pos,
                "Gold should appear before Bronze in HTML"
            );
        }
    }

    #[test]
    fn podium_zero_teams_produces_empty() {
        let repos = vec![test_fixtures::make_repository_evidence(
            "user-only-repo",
            Visibility::Public,
            false,
            test_fixtures::make_checks(
                test_fixtures::policy_pass_setting(),
                test_fixtures::secret_enabled_observable(false),
                test_fixtures::dependabot_enabled(),
                test_fixtures::branch_pass(),
                test_fixtures::codeowners_with_owners(&["@alice"]),
            ),
        )];
        let mut metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        crate::aggregate::metrics::enrich_owner_metrics_with_lifecycle(
            &mut metrics.owner_metrics,
            &repos,
            &test_fixtures::make_timestamp(),
        );
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            !index.contains("rank-gold"),
            "podium should be empty when no team owners exist"
        );
    }

    #[test]
    fn podium_one_team_shows_only_gold() {
        let repos = vec![
            test_fixtures::make_repository_evidence(
                "team-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@org/solo-team"]),
                ),
            ),
            test_fixtures::make_repository_evidence(
                "user-repo",
                Visibility::Public,
                false,
                test_fixtures::make_checks(
                    test_fixtures::policy_pass_setting(),
                    test_fixtures::secret_enabled_observable(false),
                    test_fixtures::dependabot_enabled(),
                    test_fixtures::branch_pass(),
                    test_fixtures::codeowners_with_owners(&["@alice"]),
                ),
            ),
        ];
        let mut metrics = crate::aggregate::metrics::aggregate_metrics(&repos);
        crate::aggregate::metrics::enrich_owner_metrics_with_lifecycle(
            &mut metrics.owner_metrics,
            &repos,
            &test_fixtures::make_timestamp(),
        );
        let stats = crate::aggregate::metrics::build_collection_statistics(&repos);
        let evidence = test_fixtures::make_full_evidence(
            test_fixtures::make_metadata(),
            stats,
            metrics,
            test_fixtures::make_observability(),
            repos,
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(index.contains("rank-gold"), "should have gold");
        assert!(
            !index.contains("rank-silver"),
            "should not have silver with only 1 team"
        );
        assert!(
            !index.contains("rank-bronze"),
            "should not have bronze with only 1 team"
        );
    }

    #[test]
    fn warm_start_badge_visible_when_warm_start_true() {
        let mut metadata = test_fixtures::make_metadata();
        metadata.warm_start = true;

        let evidence = test_fixtures::make_full_evidence(
            metadata,
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            test_fixtures::make_minimal_metrics(),
            test_fixtures::make_observability(),
            vec![test_fixtures::all_passing_evidence("repo-1")],
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("warm-start-badge"),
            "index should contain warm-start-badge when warm_start is true"
        );
    }

    #[test]
    fn warm_start_badge_hidden_when_warm_start_false() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            !index.contains("warm-start-badge"),
            "index should not contain warm-start-badge when warm_start is false"
        );
    }

    #[test]
    fn warm_start_meta_refresh_present_when_warm_start_true() {
        let mut metadata = test_fixtures::make_metadata();
        metadata.warm_start = true;

        let evidence = test_fixtures::make_full_evidence(
            metadata,
            test_fixtures::make_collection_statistics(1, 1, 0, 0),
            test_fixtures::make_minimal_metrics(),
            test_fixtures::make_observability(),
            vec![test_fixtures::all_passing_evidence("repo-1")],
        );

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let meta_tag = r#"<meta http-equiv="refresh" content="5">"#;
        for (name, html) in &pages {
            if is_non_html_asset(name) {
                continue;
            }
            assert!(
                html.contains(meta_tag),
                "{name} should contain meta-refresh tag when warm_start is true"
            );
        }
    }

    #[test]
    fn warm_start_meta_refresh_present_in_owner_pages() {
        let mut evidence = evidence_with_owner_repos();
        evidence.assessment_metadata.warm_start = true;

        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        let meta_tag = r#"<meta http-equiv="refresh" content="5">"#;
        assert!(
            pages.contains_key("owners.html"),
            "owner evidence should produce owners.html"
        );
        for (name, html) in &pages {
            if is_non_html_asset(name) {
                continue;
            }
            assert!(
                html.contains(meta_tag),
                "{name} should contain meta-refresh tag when warm_start is true"
            );
        }
    }

    #[test]
    fn warm_start_meta_refresh_absent_when_warm_start_false() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        for (name, html) in &pages {
            if is_non_html_asset(name) {
                continue;
            }
            assert!(
                !html.contains("http-equiv=\"refresh\""),
                "{name} should not contain meta-refresh tag when warm_start is false"
            );
        }
    }

    #[test]
    fn warm_start_meta_refresh_absent_in_owner_pages() {
        let evidence = evidence_with_owner_repos();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();

        for (name, html) in &pages {
            if is_non_html_asset(name) {
                continue;
            }
            assert!(
                !html.contains("http-equiv=\"refresh\""),
                "{name} should not contain meta-refresh tag when warm_start is false"
            );
        }
    }

    #[test]
    fn header_uses_page_header_class() {
        let evidence = sample_evidence();
        let pages = render_dashboard(&evidence, &DashboardConfig::default()).unwrap();
        let index = &pages["index.html"];

        assert!(
            index.contains("page-header"),
            "index should use page-header class for the header layout"
        );
    }

    #[test]
    fn owner_sec_score_includes_lifecycle_controls() {
        let evidence = evidence_with_mixed_owner_types();
        let owners_vm = super::build_owners_view_model(
            &evidence.metrics.owner_metrics,
            &CoverageTiers::default(),
        )
        .expect("should have owner metrics");

        let security_team = owners_vm
            .rows
            .iter()
            .find(|r| r.owner.contains("security-team"))
            .expect("should find security-team");

        assert!(
            security_team.sec_score.is_some(),
            "sec_score should be computed"
        );
        assert_eq!(
            security_team.sec_score_formatted, "100.0%",
            "all-passing team with fresh repo and no alerts should score 100%"
        );
    }
}
