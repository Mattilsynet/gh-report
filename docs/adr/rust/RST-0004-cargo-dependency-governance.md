# RST-0004. Cargo Dependency Governance

Date: 2026-04-27
Last-reviewed: 2026-04-28
Tier: B
Status: Accepted

## Related

References: RST-0001, COM-0016

## Context

COM-0016 (Dependencies as Managed Liabilities) establishes that
every external dependency is a complexity cost requiring
justification. Cargo's dependency model creates specific risks:
feature flag proliferation adds unneeded code to compile and audit,
transitive depth grows silently via `cargo add`, known
vulnerabilities persist without automated scanning, and mixed
licenses can create incompatible distribution obligations. Tools
like `cargo-audit` (vulnerability scanning) and `cargo-deny`
(license, duplicate, and advisory enforcement) provide automated
governance.

## Decision

Manage Cargo dependencies through workspace-level declarations,
minimal feature flags, and automated CI checks for vulnerabilities,
license compliance, and duplicate crates.

R1 [5]: All dependencies are declared in
  `[workspace.dependencies]`; crates reference via
  `dep = { workspace = true }` for single-inventory control
R2 [5]: New dependencies use `default-features = false` with only
  required features explicitly enabled
R3 [5]: `cargo audit` runs in CI and fails the build on known
  vulnerabilities from the RustSec advisory database
R4 [5]: `cargo deny check` runs in CI enforcing license
  allowlist, duplicate bans, and unknown registry denial
R5 [6]: Adding a new workspace dependency requires PR
  justification: what it provides, why not std/in-house, and
  transitive depth via `cargo tree -p <crate>`
R6 [6]: Dependencies are periodically reviewed for continued
  necessity using `cargo-udeps` or `cargo machete`

## Consequences

Known vulnerabilities are caught before production. License
compliance is automated — a GPL transitive dependency is caught at
CI time, not during later legal review. Default-features-false
reduces compile times, binary size, and attack surface. The
dependency approval process adds intentional friction aligned with
COM-0016. `deny.toml` evolves as the license posture changes;
the initial allowlist covers common permissive licenses.
