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
//! The companion `validate_ws_origin(headers, policy)` lives in
//! [`super::super::projection::handlers`] alongside `ws_handler`.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
