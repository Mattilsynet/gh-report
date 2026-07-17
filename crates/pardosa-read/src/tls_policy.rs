/// Encrypted-vs-plaintext decision for a `nats_url`, loopback-aware
/// (O3 orientation, mission `adr-fmt-1a060`): a plaintext scheme to a
/// loopback host is a local-dev pattern, not a production risk, so it
/// is allowed without the `--allow-plaintext` escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsPolicy {
    Allow,
    AllowWithWarning,
    Deny(DenyReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    NonLoopbackPlaintext,
    UnknownScheme,
    Unparseable,
}

impl std::fmt::Display for DenyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            DenyReason::NonLoopbackPlaintext => {
                "plaintext NATS URL targets a non-loopback host; pass --allow-plaintext to override"
            }
            DenyReason::UnknownScheme => "NATS URL has an unknown or missing scheme",
            DenyReason::Unparseable => "NATS URL host could not be parsed",
        };
        f.write_str(msg)
    }
}

/// Loopback recognition: `127.0.0.0/8`, `::1`, and the hostname
/// `localhost` (case-insensitive, trailing-dot tolerant). The
/// hostname match is name-based, not a resolved-address check — a
/// deliberate scope limit documented here rather than in a `//`
/// comment, per house style.
fn is_loopback_host(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if host == "::1" {
        return true;
    }
    let lower = host.to_ascii_lowercase();
    let lower = lower.strip_suffix('.').unwrap_or(&lower);
    if lower == "localhost" {
        return true;
    }
    if let Ok(addr) = host.parse::<std::net::Ipv4Addr>() {
        return addr.octets()[0] == 127;
    }
    false
}

fn extract_host(without_scheme: &str) -> Option<&str> {
    let host_and_rest = without_scheme.split('/').next().unwrap_or("");
    if host_and_rest.is_empty() {
        return None;
    }
    if let Some(bracket_end) = host_and_rest.find(']')
        && host_and_rest.starts_with('[')
    {
        return Some(&host_and_rest[..=bracket_end]);
    }
    let host = host_and_rest.split(':').next().unwrap_or(host_and_rest);
    if host.is_empty() { None } else { Some(host) }
}

/// Evaluate whether `nats_url` may be used, per the O3 loopback-aware
/// gate (mission `adr-fmt-1a060`, SEC-0007 confidentiality intent).
#[must_use]
pub fn evaluate_tls_policy(nats_url: &str, allow_plaintext: bool) -> TlsPolicy {
    let Some((scheme, rest)) = nats_url.split_once("://") else {
        return TlsPolicy::Deny(DenyReason::UnknownScheme);
    };

    let encrypted = match scheme {
        "tls" | "wss" => true,
        "nats" | "ws" => false,
        _ => return TlsPolicy::Deny(DenyReason::UnknownScheme),
    };

    if encrypted {
        return TlsPolicy::Allow;
    }

    let Some(host) = extract_host(rest) else {
        return TlsPolicy::Deny(DenyReason::Unparseable);
    };

    if is_loopback_host(host) {
        return TlsPolicy::Allow;
    }

    if allow_plaintext {
        TlsPolicy::AllowWithWarning
    } else {
        TlsPolicy::Deny(DenyReason::NonLoopbackPlaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_scheme_always_allowed() {
        assert_eq!(
            evaluate_tls_policy("tls://prod:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn wss_scheme_always_allowed() {
        assert_eq!(
            evaluate_tls_policy("wss://prod:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_loopback_ipv4_allowed_no_flag() {
        assert_eq!(
            evaluate_tls_policy("nats://127.0.0.1:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_loopback_ipv4_whole_slash8_allowed() {
        assert_eq!(
            evaluate_tls_policy("nats://127.0.0.2:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_loopback_ipv6_bracketed_allowed() {
        assert_eq!(
            evaluate_tls_policy("nats://[::1]:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_localhost_allowed() {
        assert_eq!(
            evaluate_tls_policy("nats://localhost:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_localhost_case_insensitive_allowed() {
        assert_eq!(
            evaluate_tls_policy("nats://LOCALHOST:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_localhost_trailing_dot_allowed() {
        assert_eq!(
            evaluate_tls_policy("nats://localhost.:4222", false),
            TlsPolicy::Allow
        );
    }

    #[test]
    fn nats_non_loopback_denied_without_flag() {
        assert_eq!(
            evaluate_tls_policy("nats://prod.example:4222", false),
            TlsPolicy::Deny(DenyReason::NonLoopbackPlaintext)
        );
    }

    #[test]
    fn nats_non_loopback_allowed_with_warning_when_flag_set() {
        assert_eq!(
            evaluate_tls_policy("nats://prod.example:4222", true),
            TlsPolicy::AllowWithWarning
        );
    }

    #[test]
    fn ws_non_loopback_denied_without_flag() {
        assert_eq!(
            evaluate_tls_policy("ws://prod.example:4222", false),
            TlsPolicy::Deny(DenyReason::NonLoopbackPlaintext)
        );
    }

    #[test]
    fn bare_host_no_scheme_denied_unknown_scheme() {
        assert_eq!(
            evaluate_tls_policy("prod.example:4222", false),
            TlsPolicy::Deny(DenyReason::UnknownScheme)
        );
    }

    #[test]
    fn empty_garbage_denied_unparseable() {
        assert_eq!(
            evaluate_tls_policy("", false),
            TlsPolicy::Deny(DenyReason::UnknownScheme)
        );
        assert_eq!(
            evaluate_tls_policy("nats://", false),
            TlsPolicy::Deny(DenyReason::Unparseable)
        );
    }
}
