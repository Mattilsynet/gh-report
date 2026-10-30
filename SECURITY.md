# Security Policy

## Reporting a Vulnerability

If you discover a security issue in any crate in this workspace (notably
`gh-report`, which handles GitHub credentials and webhook signatures, and
the `cherry-pit-*` family it builds on), please report it privately rather
than filing a public issue.

Use GitHub private vulnerability reporting on this repository first
(**Security** tab → **Report a vulnerability**). It opens a private advisory
visible only to maintainers and keeps the report out of public issue
tracking. It is enabled on this repository as of 2026-10-30.

Otherwise, email <24.7@mattilsynet.no> (Mattilsynet's published security
contact, per <https://www.mattilsynet.no/.well-known/security.txt>). The
repository is owned by the `@Mattilsynet/stabsec` team (see
`.github/CODEOWNERS`). Include:

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
