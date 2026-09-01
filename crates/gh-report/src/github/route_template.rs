//! Path-to-route-template normalisation for outbound GitHub API logging.
//!
//! [`route_template`] is the *only* representation of an outbound request
//! target this crate is permitted to log. It returns a `&'static str` drawn
//! from a closed, compile-time set of literals, so no byte of a runtime
//! request target — caller-supplied, or taken from a server-supplied
//! pagination `Link` header — can reach an emitted field. A `&'static str`
//! literal cannot carry a runtime secret, which makes the credential-leak
//! class unrepresentable rather than sanitised (SEC-0007:R1).
//!
//! The runtime target is consumed only to *select* which literal to return;
//! it is never borrowed into the result. There is deliberately no accessor
//! yielding a runtime-derived target string.

/// Template used when no known GitHub route shape matches.
pub(crate) const UNMATCHED: &str = "/{unmatched}";

/// The closed set of route templates [`route_template`] can return.
///
/// Test-only: the `&'static str` return type is the production guarantee.
/// This enumeration exists so tests can assert set membership of every
/// emitted `route` field.
#[cfg(test)]
pub(crate) const TEMPLATES: [&str; 14] = [
    "/user",
    "/rate_limit",
    "/app/installations/{installation_id}/access_tokens",
    "/orgs/{org}/repos",
    "/orgs/{org}/members",
    "/orgs/{org}/teams/{team_slug}/members",
    "/orgs/{org}/secret-scanning/alerts",
    "/repos/{owner}/{repo}",
    "/repos/{owner}/{repo}/rulesets",
    "/repos/{owner}/{repo}/commits",
    "/repos/{owner}/{repo}/secret-scanning/alerts",
    "/repos/{owner}/{repo}/branches/{branch}/protection",
    "/repos/{owner}/{repo}/contents/{path}",
    UNMATCHED,
];

/// Reduce a request path or absolute request URL to a bounded route template.
///
/// The return type is `&'static str`: the result is always one of
/// [`TEMPLATES`], never a substring of `target`.
pub(crate) fn route_template(target: &str) -> &'static str {
    route_of(path_for_matching(target))
}

const MAX_TRACKED_SEGMENTS: usize = 8;

const CONTENTS: &str = "/repos/{owner}/{repo}/contents/{path}";

fn route_of(path: &str) -> &'static str {
    let mut tracked = [""; MAX_TRACKED_SEGMENTS];
    let mut count = 0usize;
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if count < MAX_TRACKED_SEGMENTS {
            tracked[count] = segment;
        }
        count = count.saturating_add(1);
    }

    if count > MAX_TRACKED_SEGMENTS {
        return match tracked {
            ["repos", _, _, "contents", ..] => CONTENTS,
            _ => UNMATCHED,
        };
    }

    match &tracked[..count] {
        ["user"] => "/user",
        ["rate_limit"] => "/rate_limit",
        ["app", "installations", _, "access_tokens"] => {
            "/app/installations/{installation_id}/access_tokens"
        }
        ["orgs", _, "repos"] => "/orgs/{org}/repos",
        ["orgs", _, "members"] => "/orgs/{org}/members",
        ["orgs", _, "teams", _, "members"] => "/orgs/{org}/teams/{team_slug}/members",
        ["orgs", _, "secret-scanning", "alerts"] => "/orgs/{org}/secret-scanning/alerts",
        ["repos", _, _] => "/repos/{owner}/{repo}",
        ["repos", _, _, "rulesets"] => "/repos/{owner}/{repo}/rulesets",
        ["repos", _, _, "commits"] => "/repos/{owner}/{repo}/commits",
        ["repos", _, _, "secret-scanning", "alerts"] => {
            "/repos/{owner}/{repo}/secret-scanning/alerts"
        }
        ["repos", _, _, "branches", _, "protection"] => {
            "/repos/{owner}/{repo}/branches/{branch}/protection"
        }
        ["repos", _, _, "contents", ..] => CONTENTS,
        _ => UNMATCHED,
    }
}

fn path_for_matching(target: &str) -> &str {
    let rest = target.split('#').next().unwrap_or(target);
    let rest = rest.split('?').next().unwrap_or(rest);
    let rest = match rest.find("://") {
        Some(scheme_end) => authority_tail(&rest[scheme_end.saturating_add(3)..]),
        None => rest,
    };

    match rest.strip_prefix("//") {
        Some(protocol_relative) => authority_tail(protocol_relative),
        None => rest,
    }
}

fn authority_tail(authority_onwards: &str) -> &str {
    authority_onwards
        .find('/')
        .map_or("", |path_start| &authority_onwards[path_start..])
}

#[cfg(test)]
mod tests {
    use super::{TEMPLATES, UNMATCHED, route_template};

    const PLANTED: &str = "ghp_plantedsecret1234567890abcdefghij";

    fn assert_closed_and_secret_free(target: &str) {
        let template = route_template(target);
        assert!(
            TEMPLATES.contains(&template),
            "template {template} is outside the closed set for target {target}"
        );
        assert!(
            !template.contains(PLANTED),
            "template {template} leaked the planted secret from {target}"
        );
    }

    #[test]
    fn every_adversarial_target_shape_yields_a_closed_secret_free_template() {
        let vectors = [
            format!("/orgs/mattilsynet/repos?per_page=100&access_token={PLANTED}"),
            format!("https://x-access-token:{PLANTED}@api.github.com/orgs/m/repos"),
            format!("//x-access-token:{PLANTED}@api.github.com/orgs/m/repos"),
            format!("/orgs/mattilsynet/repos#{PLANTED}"),
            format!("/repos/a/b/%23{PLANTED}"),
            format!("/repos/a/b/%3Faccess_token%3D{PLANTED}"),
            format!("/repos/a/{PLANTED}"),
            format!("/repos/{PLANTED}/b/commits"),
            format!("https:\\x-access-token:{PLANTED}@api.github.com\\orgs\\m\\repos"),
            format!("x-access-token:{PLANTED}@api.github.com/orgs/m/repos"),
            format!("https://api.github.com/https://u:{PLANTED}@other/orgs/m/repos"),
            format!("HTTPS://x-access-token:{PLANTED}@api.github.com/orgs/m/repos"),
            format!("/repos/a/b/contents/dir/{PLANTED}/file.txt"),
            format!("https://api.github.com/orgs/m/repos?page=2&token={PLANTED}"),
            format!("https://x-access-token:{PLANTED}@api.github.com"),
        ];
        for target in &vectors {
            assert_closed_and_secret_free(target);
        }
    }

    #[test]
    fn arbitrary_input_never_escapes_the_closed_template_set() {
        let targets = [
            "",
            "/",
            "///",
            "not-a-path",
            "?only=query",
            "#only-fragment",
            "\\\\\\",
            "/repos/a/b/c/d/e/f/g/h/i/j/k",
            "/repos/a/b/contents/a/b/c/d/e/f/g/h/i",
        ];
        for target in targets {
            let template = route_template(target);
            assert!(
                TEMPLATES.contains(&template),
                "template {template} is outside the closed set for target {target}"
            );
        }
    }

    #[test]
    fn org_repository_listing_is_templated() {
        assert_eq!(
            route_template("/orgs/mattilsynet/repos"),
            "/orgs/{org}/repos"
        );
    }

    #[test]
    fn query_string_is_stripped_before_matching() {
        assert_eq!(
            route_template("/orgs/mattilsynet/repos?type=all&per_page=100"),
            "/orgs/{org}/repos"
        );
    }

    #[test]
    fn absolute_url_is_reduced_to_its_path() {
        assert_eq!(
            route_template("https://api.github.com/orgs/mattilsynet/repos?page=2"),
            "/orgs/{org}/repos"
        );
    }

    #[test]
    fn branch_protection_replaces_owner_repo_and_branch() {
        assert_eq!(
            route_template("/repos/mattilsynet/gh-report/branches/main/protection"),
            "/repos/{owner}/{repo}/branches/{branch}/protection"
        );
    }

    #[test]
    fn an_over_long_branch_protection_lookalike_does_not_match_by_truncation() {
        assert_eq!(
            route_template("/repos/a/b/branches/main/protection/extra/more/still/more"),
            UNMATCHED
        );
    }

    #[test]
    fn contents_collapses_a_variable_length_file_path() {
        assert_eq!(
            route_template("/repos/mattilsynet/gh-report/contents/.github/CODEOWNERS"),
            "/repos/{owner}/{repo}/contents/{path}"
        );
        assert_eq!(
            route_template("/repos/a/b/contents/a/b/c/d/e/f/g/h/i/j"),
            "/repos/{owner}/{repo}/contents/{path}"
        );
    }

    #[test]
    fn team_membership_replaces_org_and_team_slug() {
        assert_eq!(
            route_template("/orgs/mattilsynet/teams/platform/members?role=member"),
            "/orgs/{org}/teams/{team_slug}/members"
        );
    }

    #[test]
    fn org_and_repository_secret_scanning_are_distinct_templates() {
        assert_eq!(
            route_template("/orgs/mattilsynet/secret-scanning/alerts?state=open"),
            "/orgs/{org}/secret-scanning/alerts"
        );
        assert_eq!(
            route_template("/repos/mattilsynet/gh-report/secret-scanning/alerts?state=open"),
            "/repos/{owner}/{repo}/secret-scanning/alerts"
        );
    }

    #[test]
    fn single_repository_rulesets_and_commits_are_templated() {
        assert_eq!(
            route_template("/repos/mattilsynet/gh-report"),
            "/repos/{owner}/{repo}"
        );
        assert_eq!(
            route_template("/repos/mattilsynet/gh-report/rulesets"),
            "/repos/{owner}/{repo}/rulesets"
        );
        assert_eq!(
            route_template("/repos/mattilsynet/gh-report/commits?sha=main&per_page=1"),
            "/repos/{owner}/{repo}/commits"
        );
    }

    #[test]
    fn installation_token_exchange_replaces_the_installation_id() {
        assert_eq!(
            route_template("/app/installations/12345/access_tokens"),
            "/app/installations/{installation_id}/access_tokens"
        );
    }

    #[test]
    fn fixed_routes_pass_through_unchanged() {
        assert_eq!(route_template("/user"), "/user");
        assert_eq!(route_template("/rate_limit"), "/rate_limit");
    }

    #[test]
    fn unknown_shapes_collapse_to_a_single_unmatched_template() {
        assert_eq!(route_template("/test/path"), UNMATCHED);
        assert_eq!(route_template("/repos/a/b/c/d/e/f"), UNMATCHED);
        assert_eq!(route_template(""), UNMATCHED);
    }

    #[test]
    fn no_organisation_or_repository_name_survives_normalisation() {
        let paths = [
            "/orgs/mattilsynet/repos",
            "/orgs/mattilsynet/members",
            "/orgs/mattilsynet/teams/platform/members",
            "/orgs/mattilsynet/secret-scanning/alerts",
            "/repos/mattilsynet/gh-report",
            "/repos/mattilsynet/gh-report/rulesets",
            "/repos/mattilsynet/gh-report/commits",
            "/repos/mattilsynet/gh-report/secret-scanning/alerts",
            "/repos/mattilsynet/gh-report/branches/main/protection",
            "/repos/mattilsynet/gh-report/contents/.github/CODEOWNERS",
        ];
        for path in paths {
            let template = route_template(path);
            assert!(
                !template.contains("mattilsynet") && !template.contains("gh-report"),
                "template {template} leaked an identifier from {path}"
            );
        }
    }

    #[test]
    fn template_set_stays_bounded_under_identifier_variation() {
        let mut templates: Vec<&'static str> = (0..500)
            .map(|i| {
                let path = format!("/repos/org{i}/repo{i}/branches/branch{i}/protection");
                route_template(&path)
            })
            .collect();
        templates.sort_unstable();
        templates.dedup();
        assert_eq!(
            templates,
            vec!["/repos/{owner}/{repo}/branches/{branch}/protection"]
        );
    }
}
