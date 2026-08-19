# SEC-0013. Supply Chain Advisory Ignore Lifecycle

Date: 2026-08-19
Last-reviewed: 2026-08-19
Tier: B
Status: Accepted

## Related

References: SEC-0009, RST-0004

## Context

SEC-0009 decides "enforce automated dependency auditing in CI" — a
control-existence decision. Accepting one specific advisory as standing
risk via a `deny.toml` `ignore` entry is a different decision with its
own procedure: entry format, expiry, owner, and advisory-class
eligibility. SEC-0009 states none of these; its only exception language
is a Consequences-section prediction that exceptions "may" be required.
Today's `deny.toml` comment reads "Reviewed per SEC-0009" though SEC-0009
states no such rule — the citation is decorative, not binding (oracle
survey ghr-ixbd7, gaps G1/G2/G3/G6). Bolting five procedural rules onto
an Accepted 2026-04-28 ADR would dilute its control-existence decision
and muddy its Last-reviewed semantics, so this is a new ADR, not an
amendment. SEC-0013 builds on RST-0004:R3 (`cargo audit` fails the build
on vulnerabilities) by making vulnerability-class advisories ineligible
for ignoring — strengthening R3, not carving an exception into it.

## Decision

An accepted-risk advisory ignore in `deny.toml` is eligible only for
unmaintained/notice-class advisories, uses a machine-checked table-form
entry with an expiry, and is enforced by a named executable check.

R1 [5]: Only `unmaintained`- or `notice`-class RustSec advisories MAY be
  accepted as risk via a `deny.toml` ignore entry; a vulnerability-class
  advisory MUST NOT be ignored — it is fixed, or the dependency is
  dropped (builds on RST-0004:R3, does not weaken it)
R2 [5]: The sanctioned pattern is check-level `unmaintained = "all"`
  (strictest) narrowed by explicit per-id ignores; a blanket downgrade of
  the check level MUST NOT be used to silence one advisory, and
  `unused-ignored-advisory = "warn"` (or stricter) MUST remain set so a
  no-longer-matching ignore surfaces
R3 [5]: Every ignore entry MUST use cargo-deny's table form
  `{ id = "...", reason = "..." }`; the `reason` string MUST begin with
  the fixed-order prefix grammar
  `expires=YYYY-MM-DD owner=<handle> class=unmaintained|notice -- `
  followed by free-form prose (justification and exit trigger) — this
  exact grammar is what `tools/tripwires.sh deny-ignore-lifecycle`
  parses
R4 [5]: `expires` MUST be no more than 180 days after the entry's last
  review and MUST NOT be in the past; a past-due entry MUST fail CI.
  Renewal moves the date and records the outcome in this ADR's
  Last-reviewed field, not in a config comment (COM-0034:R4)
R5 [5]: A `deny.toml` ignore entry's `reason` MUST cite a rule id that
  actually states the invariant relied on; SEC-0013's rules are that id
  — closing the gap left by the current undischargeable
  "Reviewed per SEC-0009" citation
R6 [5]: `tools/tripwires.sh deny-ignore-lifecycle` is the mechanism
  enforcing R2/R3/R4; a rule without this named, dispatchable check is a
  NEEDS WORK reject per AGENTS.md's anti-dead-letter doctrine

## Consequences

`deny.toml`'s two existing ignore entries (RUSTSEC-2024-0436 paste,
RUSTSEC-2026-0173 proc-macro-error2 — see ghr-kmggf) remain bare strings
today and are NOT yet R3-compliant; bringing them to table form is
deliberately out of scope for this change and deferred to a separate,
byte-identical-safe edit (tracked: ghr-y4hkd). `deny-ignore-lifecycle`
is therefore staged in `tools/tripwires.sh` as dispatchable-by-name but
excluded from the `all`/CI-gate set until that follow-up lands — see
ghr-y4hkd for the activation trigger. Until then, citing SEC-0013 in a
future-compliant entry is sanctioned; the two live entries continue to
cite SEC-0009 informally and are tracked as a known gap, not silently
hidden. A future entry with `class=unmaintained` past its 180-day window
forces an explicit renewal-or-removal decision rather than silent
indefinite acceptance.
