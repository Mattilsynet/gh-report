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
    BranchProtectionRegime, BranchProtectionStatus, CodeownersStatus, DependabotStatus,
    ScoreCategory, SecretScanningStatus, SecurityPolicyStatus,
};
use crate::domain::evidence::{Evidence, RepositoryEvidence};
use crate::domain::metrics::{
    CollectionHealthCheckKind, OwnerType, ScoreExclusionCount, TeamRoster, TeamRosterStatus,
};
use crate::domain::time::{is_repo_stale, parse_iso8601};
use crate::error::ReportError;
use crate::report::view_model::{
    BprBandGroup, BprRepoRow, BranchProtectionRegimeViewModel, ControlCell, ControlColumn,
    CoverageTier, DeletedRepoRow, DeletedTeamRow, DeletedViewModel, OrphanedRepoRow,
    OrphanedTeamGroup, OrphanedViewModel, OwnerDetailViewModel, OwnerOverviewRow, OwnerRepoRow,
    OwnersViewModel, ReportViewModel, RosterSection, StatusDot, SummaryCard, TeamMemberRow,
    TeamRosterViewModel, TopNav, TopSecurityTeam, bpr_band_metadata, compute_health_score,
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

/// Askama template for the Branch Protection Regime drill-down page.
#[derive(Template)]
#[template(path = "branch_protection.html")]
struct BranchProtectionTemplate {
    vm: BranchProtectionRegimeViewModel,
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

    let bpr_vm = build_bpr_view_model(
        &evidence.repositories,
        &evidence.assessment_metadata.organization,
    );
    let bpr_html = render_template(&BranchProtectionTemplate {
        vm: bpr_vm,
        nav,
        warm_start,
    })?;
    sink("branch_protection.html".to_string(), bpr_html);

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
            let sec_score_table_formatted = match sec_score {
                Some(s) => format!("{s:.0}%"),
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
                sec_score_table_formatted,
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
    let table_formatted = rate_metric.map_or_else(
        || "N/A".to_string(),
        crate::domain::metrics::RateMetric::to_table_string,
    );
    let exclusion = control_key_to_check_kind(key)
        .map(|check_kind| format_exclusion(check_kind, score_exclusion_counts));
    let (excluded_total, excluded_formatted) = match exclusion {
        Some(e) => (e.total, e.formatted),
        None => (0, "0 unmeasured".to_string()),
    };
    ControlCell {
        rate_formatted: formatted,
        rate_table_formatted: table_formatted,
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

    let (is_complete, status_label, degraded_notice) = match roster.status {
        TeamRosterStatus::Complete => (true, "Complete", None),
        TeamRosterStatus::Deleted => (
            false,
            "Deleted",
            Some(
                "This team no longer exists on GitHub — CODEOWNERS references a team \
                 GitHub has deleted.",
            ),
        ),
        TeamRosterStatus::PermissionDenied => (
            false,
            "Permission denied",
            Some("Roster fetch: Permission denied — this list may be incomplete."),
        ),
        TeamRosterStatus::TransientError => (
            false,
            "Temporarily unavailable",
            Some("Roster fetch: Temporarily unavailable — this list may be incomplete."),
        ),
    };

    TeamRosterViewModel {
        is_complete,
        status_label,
        degraded_notice,
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
        OwnerType::AmbiguousTeamShaped => None,
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

            let roster_entry = team_rosters.iter().find(|r| r.canonical_owner == m.owner);
            let roster = match m.owner_type {
                OwnerType::User => RosterSection::NotApplicable,
                OwnerType::Team | OwnerType::AmbiguousTeamShaped => match roster_entry {
                    Some(r) => RosterSection::Team(build_team_roster_view_model(r)),
                    None => RosterSection::Unresolved,
                },
            };

            let orphan_repo_rows: Vec<OrphanedRepoRow> = orphaned_by_team
                .iter()
                .find(|group| group.team == m.owner)
                .map(|group| group.rows.clone())
                .unwrap_or_default();
            let orphan_unresolved = matches!(
                m.owner_type,
                OwnerType::Team | OwnerType::AmbiguousTeamShaped
            ) && roster_entry.is_none();

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
                orphan_unresolved,
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

/// Map an `Option<bool>` signal to a status dot: `Some(true)` pass,
/// `Some(false)` fail, `None` unknown.
fn option_bool_dot(value: Option<bool>) -> StatusDot {
    match value {
        Some(true) => StatusDot {
            css_class: "status-pass",
            label: "yes",
        },
        Some(false) => StatusDot {
            css_class: "status-fail",
            label: "no",
        },
        None => StatusDot {
            css_class: "status-unknown",
            label: "unknown",
        },
    }
}

/// Build the Branch Protection Regime drill-down view model (BPR0..BPR5).
///
/// Report-side only (CHE-0083:R7): [`BranchProtectionRegime`] is computed
/// fresh from each repo's [`crate::domain::checks::BranchProtectionResult::regime`]
/// on every call, never read from a persisted field. Page-local view-model
/// (COM-0027:R3/R4) — no `domain::metrics`/`aggregate::metrics` sibling.
fn build_bpr_view_model(
    repositories: &[RepositoryEvidence],
    organization: &str,
) -> BranchProtectionRegimeViewModel {
    let org_encoded = utf8_percent_encode(organization, PATH_SEGMENT).to_string();

    let mut by_regime: HashMap<BranchProtectionRegime, Vec<BprRepoRow>> = HashMap::new();
    for repo in repositories.iter().filter(|r| !r.repository.archived) {
        let name_encoded = utf8_percent_encode(&repo.repository.name, PATH_SEGMENT);
        let (repo_name, repo_url) = build_repo_display(repo, &org_encoded, &name_encoded);
        let details = &repo.checks.branch_protection.details;
        let regime = repo.checks.branch_protection.regime();

        let row = BprRepoRow {
            repo_name,
            repo_url,
            visibility: repo.repository.visibility.to_string(),
            has_pr: option_bool_dot(details.has_pr),
            required_reviewers_formatted: details
                .required_reviewers
                .map_or_else(|| EM_DASH.to_string(), |count| count.to_string()),
            has_status_checks: option_bool_dot(details.has_status_checks),
            admin_equivalent: option_bool_dot(details.admin_equivalent),
            has_broad_bypass: option_bool_dot(details.has_broad_bypass),
            force_push_blocked: option_bool_dot(details.force_push_blocked),
            deletion_blocked: option_bool_dot(details.deletion_blocked),
        };

        by_regime.entry(regime).or_default().push(row);
    }

    let all_regimes = [
        BranchProtectionRegime::Unmeasured,
        BranchProtectionRegime::Unprotected,
        BranchProtectionRegime::IntegrityOnly,
        BranchProtectionRegime::ReviewedWithBypass,
        BranchProtectionRegime::ReviewedGated,
        BranchProtectionRegime::Hardened,
    ];

    let mut total_repos: u32 = 0;
    let bands: Vec<BprBandGroup> = all_regimes
        .into_iter()
        .map(|regime| {
            let (id, name, description, css_class) = bpr_band_metadata(regime);
            let mut repos = by_regime.remove(&regime).unwrap_or_default();
            repos.sort_by_key(|r| r.repo_name.to_lowercase());
            total_repos += u32::try_from(repos.len()).unwrap_or(u32::MAX);
            BprBandGroup {
                id,
                name,
                description,
                css_class,
                repos,
            }
        })
        .collect();

    BranchProtectionRegimeViewModel {
        organization: organization.to_string(),
        bands,
        total_repos,
    }
}

#[cfg(test)]
#[path = "html/tests.rs"]
mod tests;
