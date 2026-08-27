//! SEC-0012 WebSocket Origin validation policy carriers.
//!
//! Realises **SEC-0012** as two library-owned value types threaded
//! through [`super::super::projection::build_projection_router`]
//! between `limits` and `extra_routes` per the CHE-0049 Amendment
//! 2026-06-10 (SEC-0012) grammar:
//!
//! - [`WebSocketOriginPolicy`] — typed policy enum. `Strict` (default)
//!   rejects absent `Origin` at WS upgrade with `403 FORBIDDEN`.
//!   `AllowAbsent` is the documented escape hatch for non-browser
//!   clients; consumers electing it accept CWE-346 / CWE-1385 risk
//!   per SEC-0012:R3.
//! - [`WsAuthLimits`] — sibling value type to [`super::LayerLimits`],
//!   carrying `origin_policy` today; future authentication knobs land
//!   as new fields per SEC-0012:R4 + CHE-0062:R6.
//!
//! Both types carry `#[non_exhaustive]` per COM-0021:R1 for additive,
//! semver-minor evolution.
//!
//! The companion [`validate_ws_origin`] and its authority-normalisation
//! helper live in this module — the single home for WS origin
//! validation across both the static-serve runtime and the projection
//! adapter (CHE-0086:R8).

use axum::http::{HeaderMap, header};

/// Maximum inbound WebSocket message size (bytes). Client messages are
/// discarded, so 4 KB is sufficient for Pong frames and future commands.
pub(crate) const WS_MAX_MESSAGE_SIZE: usize = 4096;

/// Policy controlling how the projection WebSocket upgrade validates
/// the inbound `Origin` header against the `Host` header (SEC-0012).
///
/// The default ([`WebSocketOriginPolicy::Strict`]) closes the
/// CWE-346 / CWE-1385 Cross-Site WebSocket Hijacking (CSWSH) hole at
/// the trust boundary: a browser-context attacker cannot open a WS
/// to the target carrying victim cookies because the absent or
/// mismatched `Origin` is rejected before the handshake completes.
///
/// Future variants land additively per SEC-0012:R4; the
/// `#[non_exhaustive]` attribute reserves the surface for them
/// (e.g. an `AllowMatching` allowlist variant) without breaking
/// downstream `match` arms.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebSocketOriginPolicy {
    /// Reject WS upgrades whose `Origin` header is absent, malformed,
    /// or does not match the `Host` header per the
    /// donor-derived authority-normalisation semantics. Closes
    /// CWE-346 / CWE-1385 CSWSH at the WS trust boundary. **This is
    /// the default** (SEC-0012:R2).
    Strict,

    /// Permit WS upgrades that arrive without an `Origin` header.
    /// Documented escape hatch for non-browser clients (CLI tools,
    /// server-to-server bots, native mobile apps) per SEC-0012:R3.
    /// Consumers electing this variant accept CWE-346 / CWE-1385
    /// risk explicitly. Mismatched and malformed `Origin` headers
    /// remain rejected — the variant only loosens the *absent*
    /// branch.
    AllowAbsent,
}

impl Default for WebSocketOriginPolicy {
    /// Safety-by-default per SEC-0012:R2 — every consumer that does
    /// not explicitly elect [`WebSocketOriginPolicy::AllowAbsent`]
    /// gets CSWSH-closed behaviour.
    fn default() -> Self {
        Self::Strict
    }
}

/// Per-router WebSocket authentication knobs attached by the
/// projection adapter at construction (SEC-0012:R1, R4).
///
/// Sibling to [`super::LayerLimits`] (CHE-0062 availability sizing).
/// Where `LayerLimits` carries SEC-0003 R1/R3 availability sizing
/// (`usize` numbers), `WsAuthLimits` carries SEC-0005 authenticity
/// policy (typed enums). Splitting the two carriers keeps CISQ
/// primaries MECE per COM-0028 (authenticity vs availability).
///
/// Construct via [`Default::default`] (= safety-by-default per
/// SEC-0012:R2) or [`WsAuthLimits::permissive_for_tests`]. The
/// `#[non_exhaustive]` attribute (COM-0021:R1) blocks the
/// struct-literal idiom outside the crate, matching `LayerLimits`;
/// consumers cannot accidentally rely on a field set that future
/// versions will extend per CHE-0062:R6.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WsAuthLimits {
    /// Policy applied at WS upgrade to the inbound `Origin` header.
    /// See [`WebSocketOriginPolicy`].
    pub origin_policy: WebSocketOriginPolicy,
}

impl WsAuthLimits {
    /// Construct a [`WsAuthLimits`] electing the permissive
    /// [`WebSocketOriginPolicy::AllowAbsent`] variant. Intended
    /// **only** for tests whose harness does not synthesise an
    /// `Origin` header.
    ///
    /// The name is deliberately pejorative: production code that
    /// calls this is wrong unless the consumer has documented
    /// acceptance of CWE-346 / CWE-1385 risk per SEC-0012:R3.
    /// Production code constructs via [`Default::default`] (= Strict)
    /// or explicit named-variant election.
    #[must_use]
    pub fn permissive_for_tests() -> Self {
        Self {
            origin_policy: WebSocketOriginPolicy::AllowAbsent,
        }
    }
}

/// Validate the inbound `Origin` against `Host` for CSWSH defence,
/// gated by the consumer-elected [`WebSocketOriginPolicy`] per
/// SEC-0012.
///
/// Behaviour split by `policy`:
///
/// - **Absent `Origin`**: depends on `policy`.
///   [`WebSocketOriginPolicy::Strict`] → reject (SEC-0012:R2);
///   [`WebSocketOriginPolicy::AllowAbsent`] → permit (SEC-0012:R3,
///   non-browser-client escape hatch).
/// - **Present `Origin`**: parsed and compared to `Host` with
///   authority normalisation (default-port stripping, IPv6 bracket
///   handling per [`normalize_authority`]). Mismatched, malformed, or
///   non-HTTP(S) `Origin` is rejected regardless of `policy`.
pub(crate) fn validate_ws_origin(headers: &HeaderMap, policy: &WebSocketOriginPolicy) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return matches!(policy, WebSocketOriginPolicy::AllowAbsent);
    };
    let Ok(origin_str) = origin.to_str() else {
        return false;
    };
    let Some((scheme, after_scheme)) = origin_str.split_once("://") else {
        return false;
    };
    let default_port = match scheme {
        "https" => "443",
        "http" => "80",
        _ => return false,
    };
    let origin_authority = after_scheme
        .split_once('/')
        .map_or(after_scheme, |(head, _)| head);
    let Some(host_hdr) = headers.get(header::HOST) else {
        return false;
    };
    let Ok(host_str) = host_hdr.to_str() else {
        return false;
    };
    if origin_authority == host_str {
        return true;
    }
    normalize_authority(origin_authority, default_port)
        == normalize_authority(host_str, default_port)
}

/// Strip the default port from an authority string for comparison.
///
/// Handles IPv6 bracket notation: `[::1]:8080` splits into hostname
/// `[::1]` and port `8080`. Plain IPv4/hostname uses `rsplit_once(':')`.
///
/// `"example.com:443"` with `default_port = "443"` → `("example.com", "")`.
/// `"example.com:8080"` with `default_port = "443"` → `("example.com", "8080")`.
/// `"example.com"` → `("example.com", "")`.
/// `"[::1]:8080"` → `("[::1]", "8080")`.
///
/// The bracket branch is gated on a leading `[`, so a bare `]` inside an
/// otherwise plain authority (`foo]bar:8080`) falls through to the
/// `rsplit_once` branch and keeps its port, rather than being truncated
/// at the stray bracket.
fn normalize_authority<'a>(authority: &'a str, default_port: &str) -> (&'a str, &'a str) {
    if authority.starts_with('[')
        && let Some(bracket_end) = authority.find(']')
    {
        let after_bracket = &authority[bracket_end + 1..];
        if let Some(port) = after_bracket.strip_prefix(':') {
            let hostname = &authority[..=bracket_end];
            if port == default_port {
                return (hostname, "");
            }
            return (hostname, port);
        }
        return (authority, "");
    }

    match authority.rsplit_once(':') {
        Some((hostname, port)) if port == default_port => (hostname, ""),
        Some((hostname, port)) => (hostname, port),
        None => (authority, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn default_websocket_origin_policy_is_strict() {
        assert_eq!(
            WebSocketOriginPolicy::default(),
            WebSocketOriginPolicy::Strict
        );
        assert!(matches!(
            WsAuthLimits::default().origin_policy,
            WebSocketOriginPolicy::Strict
        ));
    }

    #[test]
    fn permissive_for_tests_elects_allow_absent() {
        assert!(matches!(
            WsAuthLimits::permissive_for_tests().origin_policy,
            WebSocketOriginPolicy::AllowAbsent
        ));
    }

    fn make_headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn validate_ws_origin_allow_absent_policy_allows_absent() {
        assert!(validate_ws_origin(
            &HeaderMap::new(),
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn validate_ws_origin_strict_rejects_absent() {
        assert!(!validate_ws_origin(
            &HeaderMap::new(),
            &WebSocketOriginPolicy::Strict
        ));
    }

    #[test]
    fn validate_ws_origin_allows_exact_match() {
        let h = make_headers(&[("origin", "https://example.com"), ("host", "example.com")]);
        assert!(validate_ws_origin(&h, &WebSocketOriginPolicy::Strict));
    }

    #[test]
    fn validate_ws_origin_allows_default_port_normalisation() {
        let h = make_headers(&[
            ("origin", "https://example.com:443"),
            ("host", "example.com"),
        ]);
        assert!(validate_ws_origin(&h, &WebSocketOriginPolicy::Strict));
    }

    #[test]
    fn validate_ws_origin_rejects_mismatched_host() {
        let h = make_headers(&[("origin", "https://evil.com"), ("host", "example.com")]);
        assert!(!validate_ws_origin(&h, &WebSocketOriginPolicy::Strict));
    }

    #[test]
    fn validate_ws_origin_rejects_non_http_scheme() {
        let h = make_headers(&[("origin", "file://local"), ("host", "example.com")]);
        assert!(!validate_ws_origin(&h, &WebSocketOriginPolicy::Strict));
    }

    /// Regression guard for the authority-parser unification (CHE-0086:R8).
    ///
    /// The superseded projection-side parser matched on `find(']')`
    /// without requiring a leading `[`, so `foo]bar:8080` truncated to
    /// `("foo]", "")` and `foo]baz` likewise — making a cross-origin
    /// `Origin`/`Host` pair compare equal and pass validation. That is a
    /// CWE-346 origin-validation bypass in the SEC-0012-governed copy.
    /// The retained parser gates the bracket branch on `starts_with('[')`,
    /// so this pair must be REJECTED.
    #[test]
    fn validate_ws_origin_rejects_stray_bracket_authority_confusion() {
        let h = make_headers(&[("origin", "http://foo]bar"), ("host", "foo]baz")]);
        assert!(
            !validate_ws_origin(&h, &WebSocketOriginPolicy::Strict),
            "stray-bracket authorities must not normalise to a common prefix"
        );

        assert_eq!(
            normalize_authority("foo]bar:8080", "80"),
            ("foo]bar", "8080"),
            "a bare ']' without a leading '[' must not trigger the IPv6 branch"
        );
        assert_ne!(
            normalize_authority("foo]bar", "80"),
            normalize_authority("foo]baz", "80"),
            "distinct stray-bracket authorities must stay distinct"
        );
    }

    #[test]
    fn normalize_authority_ipv6_loopback_no_port() {
        assert_eq!(normalize_authority("[::1]", "443"), ("[::1]", ""));
    }

    #[test]
    fn normalize_authority_ipv6_with_non_default_port() {
        assert_eq!(normalize_authority("[::1]:8080", "443"), ("[::1]", "8080"));
    }

    #[test]
    fn normalize_authority_ipv6_with_default_port_stripped() {
        assert_eq!(normalize_authority("[::1]:443", "443"), ("[::1]", ""));
    }

    #[test]
    fn normalize_authority_ipv6_full_address() {
        assert_eq!(
            normalize_authority("[2001:db8::1]:9090", "443"),
            ("[2001:db8::1]", "9090")
        );
    }
}
