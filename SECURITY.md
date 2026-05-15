# Security Policy

## Reporting a Vulnerability

If you discover a security issue in any crate in this workspace (notably
`gh-report`, which handles GitHub credentials and webhook signatures, and
the `cherry-pit-*` family it builds on), please report it privately rather
than filing a public issue.

Send a report to the repository owner via GitHub:
<https://github.com/acje>. Include:

- a description of the issue and the affected crate(s),
- reproduction steps or a minimal proof of concept,
- any known mitigations.

We will acknowledge receipt within a reasonable window, investigate, and
coordinate disclosure once a fix is available.

## Scope

In scope:

- token handling, secret zeroization, and auth flows in `gh-report` and
  `cherry-pit-gateway` (see `docs/adr/security/`),
- HMAC verification and replay protection on webhook endpoints,
- supply-chain concerns flagged by `cargo audit` / `cargo deny`
  (workspace policy in `deny.toml`).

Out of scope:

- issues in third-party dependencies for which an upstream advisory
  already exists — please file upstream first,
- the `adr-fmt` binary is read-only at runtime and has
  `#![forbid(unsafe_code)]`; report findings nonetheless if you see
  something unexpected.
