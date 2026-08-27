//! SEC-0012 WebSocket policy carrier and Origin validation.
//!
//! Realises **SEC-0012** for every cherry-pit-web surface that mounts a
//! WS upgrade — the projection adapter and the generic serve runtime:
//!
//! - [`WebSocketOriginPolicy`] — typed policy enum. `Strict` (default)
//!   rejects absent `Origin` at WS upgrade with `403 FORBIDDEN`.
//!   `AllowAbsent` is the documented escape hatch for non-browser
//!   clients; consumers electing it accept CWE-346 / CWE-1385 risk
//!   per SEC-0012:R3.
//! - [`WsPolicy`] — the carrier, holding `max_connections` alongside
//!   `origin_policy` (SEC-0012:R1).
//!
//! The two knobs are fused deliberately. They were once split across
//! `LayerLimits` (availability) and `WsAuthLimits` (authenticity) to
//! keep CISQ primaries MECE; that split let `serve::build_router` take
//! the WS cap and never take a policy, so no origin-strict WebSocket
//! ran in production for the eight weeks SEC-0012:R2 declared `Strict`
//! the default. A carrier now groups the knobs one capability needs to
//! be safe, so the omission is a compile error rather than an absence
//! nobody can see. See SEC-0012's Consequences for the full reversal.
//!
//! Both types carry `#[non_exhaustive]` per COM-0021:R1 for additive,
//! semver-minor evolution.
//!
//! [`validate_ws_origin`] and its [`Authority`] parser live here — the
//! single home for WS origin validation across both the static-serve
//! runtime and the projection adapter (CHE-0086:R8).

use axum::http::{HeaderMap, header};

/// Maximum inbound WebSocket message size (bytes). Client messages are
/// discarded, so 4 KB is sufficient for Pong frames and future commands.
pub(crate) const WS_MAX_MESSAGE_SIZE: usize = 4096;

/// Policy controlling how a WebSocket upgrade validates the inbound
/// `Origin` header against the `Host` header (SEC-0012).
///
/// The default ([`WebSocketOriginPolicy::Strict`]) closes the
/// CWE-346 / CWE-1385 Cross-Site WebSocket Hijacking (CSWSH) hole at
/// the trust boundary: an attacker-origin page cannot open a WS to the
/// target, because the absent or mismatched `Origin` is rejected
/// before the handshake completes.
///
/// Ambient credentials are the textbook CSWSH vector — the browser
/// attaches the victim's cookies to a cross-origin upgrade — but they
/// are not the only one, and a service that sets no cookies is not
/// therefore safe. The socket pushes data *after* the handshake, so a
/// successful cross-origin upgrade is a read primitive over whatever
/// the surface publishes regardless of how the request was
/// authenticated.
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

/// Per-surface WebSocket policy, attached at router construction by
/// every cherry-pit-web surface that mounts a `/ws` upgrade
/// (SEC-0012:R1, R4).
///
/// Carries the two knobs a WS upgrade needs to be safe:
/// `max_connections` (the SEC-0003:R3 connection cap, unconditional
/// per CHE-0062:R4) and `origin_policy` (the SEC-0005 authenticity
/// election, consumer-discretionary per SEC-0012:R5). Fusing an
/// electable knob with an unconditional one makes neither
/// electable-by-omission — a consumer must name both.
///
/// Construct via [`WsPolicy::new`], which yields `Strict` without the
/// caller having to name it, or [`WsPolicy::permissive_for_tests`].
/// The `#[non_exhaustive]` attribute (COM-0021:R1) blocks the
/// struct-literal idiom outside the crate, matching
/// [`super::LayerLimits`]; consumers cannot accidentally rely on a
/// field set that future versions will extend per SEC-0012:R4.
///
/// [`Default`] is deliberately absent: `max_connections` has no
/// defensible default, and a defaulted availability cap is the same
/// SEC-0003 footgun [`super::LayerLimits`] avoids.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct WsPolicy {
    /// Maximum concurrent WebSocket connections accepted by the
    /// surface's `/ws` upgrade. Upgrades arriving when this many
    /// sessions are already attached are rejected with `503` before
    /// the handshake completes. SEC-0003:R3 route-scoped.
    pub max_connections: usize,

    /// Policy applied at WS upgrade to the inbound `Origin` header.
    /// See [`WebSocketOriginPolicy`].
    pub origin_policy: WebSocketOriginPolicy,
}

impl WsPolicy {
    /// Construct a [`WsPolicy`] capped at `max_connections` with the
    /// safety-by-default [`WebSocketOriginPolicy::Strict`] election
    /// (SEC-0012:R2).
    ///
    /// This is the production constructor. `Strict` is reached by not
    /// asking for anything else, which is the property that makes
    /// SEC-0012:R2 hold by construction rather than by review.
    #[must_use]
    pub fn new(max_connections: usize) -> Self {
        Self {
            max_connections,
            origin_policy: WebSocketOriginPolicy::Strict,
        }
    }

    /// Construct a [`WsPolicy`] electing the permissive
    /// [`WebSocketOriginPolicy::AllowAbsent`] variant with an
    /// effectively unbounded connection cap. Intended **only** for
    /// tests whose harness does not synthesise an `Origin` header.
    ///
    /// The name is deliberately pejorative: production code that
    /// calls this is wrong unless the consumer has documented
    /// acceptance of CWE-346 / CWE-1385 risk per SEC-0012:R3.
    #[must_use]
    pub fn permissive_for_tests() -> Self {
        Self {
            max_connections: 1024,
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
/// - **Present `Origin`**: both `Origin` and `Host` are parsed into
///   [`Authority`] and the two validated values are compared. An
///   authority that is malformed, ambiguous, or carries userinfo never
///   becomes a comparison operand (SEC-0012:R6), so mismatched,
///   malformed, and non-HTTP(S) `Origin` are rejected regardless of
///   `policy`.
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
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "https" => 443,
        "http" => 80,
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
    let (Some(origin), Some(host)) = (
        Authority::parse(origin_authority, default_port),
        Authority::parse(host_str, default_port),
    ) else {
        return false;
    };
    origin == host
}

/// A validated, normalised HTTP authority (SEC-0012:R6).
///
/// [`Authority::parse`] is the only constructor, so a malformed,
/// ambiguous, or userinfo-bearing authority is unrepresentable as a
/// comparison operand rather than being screened by a runtime guard
/// over raw strings (SEC-0002:R3 parse-don't-validate).
///
/// Normalisation makes equality the whole comparison: `host` is
/// ASCII-lowercased per RFC 3986 §3.2.2, and a port equal to the
/// scheme default becomes `None` so `example.com` and
/// `example.com:443` compare equal under `https`.
#[derive(Debug, PartialEq, Eq)]
struct Authority {
    host: String,
    port: Option<u16>,
}

impl Authority {
    /// Parse `raw` as an authority, normalising against `default_port`.
    ///
    /// Returns `None` when `raw` is empty, carries userinfo (`@`), has
    /// an empty or out-of-`u16`-range port, is unbracketed IPv6, or
    /// carries a bracket outside well-formed IPv6 notation. Each of
    /// those is an authority whose intended host is ambiguous, and an
    /// ambiguous authority must not participate in an origin decision.
    fn parse(raw: &str, default_port: u16) -> Option<Self> {
        if raw.is_empty() || raw.contains('@') {
            return None;
        }

        let (host, port_str) = if let Some(rest) = raw.strip_prefix('[') {
            let (inner, after) = rest.split_once(']')?;
            if inner.is_empty() || inner.contains('[') {
                return None;
            }
            let port = if after.is_empty() {
                None
            } else {
                Some(after.strip_prefix(':')?)
            };
            (inner, port)
        } else {
            if raw.contains('[') || raw.contains(']') {
                return None;
            }
            let (host, port) = match raw.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (raw, None),
            };
            if host.is_empty() || host.contains(':') {
                return None;
            }
            (host, port)
        };

        let port = match port_str {
            Some(port) => {
                let parsed: u16 = port.parse().ok()?;
                (parsed != default_port).then_some(parsed)
            }
            None => None,
        };

        Some(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
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
            WsPolicy::new(8).origin_policy,
            WebSocketOriginPolicy::Strict
        ));
    }

    #[test]
    fn new_carries_the_requested_cap() {
        assert_eq!(WsPolicy::new(7).max_connections, 7);
    }

    #[test]
    fn permissive_for_tests_elects_allow_absent() {
        assert!(matches!(
            WsPolicy::permissive_for_tests().origin_policy,
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
    /// Under SEC-0012:R6 a stray bracket is not merely parsed
    /// differently, it is rejected: the authority never becomes a
    /// comparison operand at all.
    #[test]
    fn validate_ws_origin_rejects_stray_bracket_authority_confusion() {
        let h = make_headers(&[("origin", "http://foo]bar"), ("host", "foo]baz")]);
        assert!(
            !validate_ws_origin(&h, &WebSocketOriginPolicy::Strict),
            "stray-bracket authorities must not normalise to a common prefix"
        );

        assert_eq!(
            Authority::parse("foo]bar:8080", 80),
            None,
            "a bare ']' without a leading '[' is ambiguous and must not parse"
        );
        assert_eq!(
            Authority::parse("foo]baz", 80),
            None,
            "the confusable counterpart must be equally unrepresentable"
        );
    }

    #[test]
    fn validate_ws_origin_matches_host_case_insensitively() {
        let h = make_headers(&[("origin", "https://EXAMPLE.com"), ("host", "example.com")]);
        assert!(
            validate_ws_origin(&h, &WebSocketOriginPolicy::Strict),
            "RFC 3986 authority host comparison is case-insensitive"
        );
    }

    #[test]
    fn validate_ws_origin_matches_scheme_case_insensitively() {
        let h = make_headers(&[
            ("origin", "HTTPS://example.com:443"),
            ("host", "example.com"),
        ]);
        assert!(
            validate_ws_origin(&h, &WebSocketOriginPolicy::Strict),
            "RFC 3986 §3.1 schemes are case-insensitive; an uppercase scheme must still \
             resolve its default port rather than falsely rejecting a same-origin client"
        );
    }

    #[test]
    fn validate_ws_origin_rejects_userinfo_in_host() {
        let h = make_headers(&[
            ("origin", "https://example.com"),
            ("host", "evil.com@example.com"),
        ]);
        assert!(
            !validate_ws_origin(&h, &WebSocketOriginPolicy::Strict),
            "userinfo is illegal in Host; it must not normalise away to a matching host"
        );
    }

    #[test]
    fn validate_ws_origin_rejects_out_of_range_port() {
        let h = make_headers(&[
            ("origin", "https://example.com:99999"),
            ("host", "example.com"),
        ]);
        assert!(
            !validate_ws_origin(&h, &WebSocketOriginPolicy::Strict),
            "an unparseable port must not silently degrade to the scheme default"
        );
    }

    fn authority(host: &str, port: Option<u16>) -> Authority {
        Authority {
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn authority_ipv6_loopback_no_port() {
        assert_eq!(Authority::parse("[::1]", 443), Some(authority("::1", None)));
    }

    #[test]
    fn authority_ipv6_with_non_default_port() {
        assert_eq!(
            Authority::parse("[::1]:8080", 443),
            Some(authority("::1", Some(8080)))
        );
    }

    #[test]
    fn authority_ipv6_with_default_port_stripped() {
        assert_eq!(
            Authority::parse("[::1]:443", 443),
            Some(authority("::1", None))
        );
    }

    #[test]
    fn authority_ipv6_full_address() {
        assert_eq!(
            Authority::parse("[2001:db8::1]:9090", 443),
            Some(authority("2001:db8::1", Some(9090)))
        );
    }

    #[test]
    fn authority_rejects_unbracketed_ipv6_as_ambiguous() {
        assert_eq!(
            Authority::parse("2001:db8::1", 443),
            None,
            "unbracketed IPv6 cannot be split into host and port unambiguously"
        );
    }

    #[test]
    fn authority_rejects_empty_port() {
        assert_eq!(Authority::parse("example.com:", 443), None);
    }

    #[test]
    fn origin_validation_cross_origin_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://evil.com"));
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_differing_non_default_ports_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com:9999"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com:8080"));
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_file_scheme_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("file://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_ftp_scheme_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("ftp://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_http_default_port_stripped() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://example.com:80"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_http_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://localhost:8080"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8080"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_no_host_header_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_no_origin_header_allowed() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_no_scheme_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static("example.com"));
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_origin_has_default_port_host_omits() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com:443"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_origin_with_trailing_path() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com/path"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_same_origin_matches() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_same_origin_with_port() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://example.com:8080"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com:8080"));
        assert!(validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }

    #[test]
    fn origin_validation_subdomain_mismatch_rejected() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://sub.example.com"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));
        assert!(!validate_ws_origin(
            &headers,
            &WebSocketOriginPolicy::AllowAbsent
        ));
    }
}
