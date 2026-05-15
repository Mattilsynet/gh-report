//! Integration tests for the adr-fmt binary.
//!
//! Each test creates a self-contained tempdir with the necessary file
//! structure (adr-fmt.toml, domain directories, ADR files) and runs the
//! binary against it.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ── helpers ─────────────────────────────────────────────────────────

/// Minimal adr-fmt.toml for a single-domain test corpus (override-only format).
const MINIMAL_CONFIG: &str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "TST"
name = "Test Domain"
directory = "test"
description = "Integration test domain."
crates = ["test-core"]

[[rules]]
id = "T015"
params = { min_words = 10 }
"#;

/// Multi-domain config with foundation domain.
const MULTI_DOMAIN_CONFIG: &str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "COM"
name = "Common"
directory = "common"
description = "Cross-cutting principles."
crates = []
foundation = true

[[domains]]
prefix = "TST"
name = "Test Domain"
directory = "test"
description = "Integration test domain."
crates = ["test-core"]

[[rules]]
id = "T015"
params = { min_words = 10 }
"#;

/// A valid ADR file that passes all rules (root ADR).
const VALID_ADR: &str = "\
# TST-0001. Valid Test ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0001

## Context

This ADR documents a valid test case for the integration test suite to verify.

## Decision

R1 [5]: We decided to create a minimal but complete ADR that satisfies all template rules.

## Consequences

The integration test can verify that a clean corpus produces zero diagnostics.
";

/// A second ADR that references TST-0001.
const REFERENCING_ADR: &str = "\
# TST-0002. Referencing ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-0001

## Context

This ADR references TST-0001 to test inbound-reference resolution in --refs mode.

## Decision

R1 [5]: We reference another ADR to verify --refs surfaces the inbound link.

## Consequences

`--refs TST-0001` should list TST-0002 as an inbound reference.
";

/// An ADR with a dangling link to trigger L001.
const DANGLING_LINK_ADR: &str = "\
# TST-0003. Dangling Link ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-9999

## Context

This ADR has a dangling link that should trigger the L001 validation rule.

## Decision

R1 [5]: We reference a non-existent ADR to verify that dangling link detection works.

## Consequences

The linter should report a dangling link warning for TST-9999 in the output.
";

/// ADR using a legacy relationship verb (triggers L006 per AFM-0009).
const LEGACY_VERB_ADR: &str = "\
# TST-0005. Legacy Verb ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Depends on: TST-0001

## Context

This ADR uses the legacy `Depends on` verb which is deprecated by AFM-0009.

## Decision

R1 [5]: Trigger L006 by using a legacy relationship verb.

## Consequences

The linter should report L006 with migration guidance to use References.
";

/// ADR without tagged rules (triggers T016).
const NO_TAGGED_RULES_ADR: &str = "\
# TST-0004. No Tagged Rules

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0004

## Context

This ADR has prose in the Decision section but no tagged rules.

## Decision

We decided to use plain prose without the required tagged rule format.

## Consequences

The linter should report a T016 warning for missing tagged rules.
";

/// Draft ADR without tagged rules (no longer exempt from T016).
const DRAFT_ADR: &str = "\
# TST-0005. Draft ADR

Date: 2026-04-27
Tier: B
Status: Draft

## Related

Root: TST-0005

## Context

This is a draft ADR that is no longer exempt from the tagged rules requirement.

## Decision

We are still drafting this decision and have not formalized rules yet.

## Consequences

Draft status no longer exempts this ADR from T016 checks.
";

/// ADR with per-ADR Crates field.
const ADR_WITH_CRATES: &str = "\
# TST-0006. ADR With Crates

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Crates: test-core, test-api
Status: Accepted

## Related

Root: TST-0006

## Context

This ADR specifies crate applicability via the Crates metadata field.

## Decision

R1 [5]: Crate-specific decisions are scoped to test-core and test-api.

## Consequences

Context mode should only include this ADR when querying test-core or test-api.
";

/// Foundation domain ADR (COM).
const FOUNDATION_ADR: &str = "\
# COM-0001. Foundation Principle

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: S
Status: Accepted

## Related

Root: COM-0001

## Context

This is a foundation domain ADR that applies to all crates in the workspace.

## Decision

R1 [5]: Foundation rules apply universally across all domains and crates.

## Consequences

Context mode must always include foundation domain ADRs.
";

/// Stale ADR (superseded, in stale directory). Stub form per AFM-0022.
/// Carries a `Supersedes:` edge to TST-0001 to exercise the
/// stale-referrer-exclusion path in `--refs`. Retirement narrative
/// is sized comfortably above tier-B min-words (with `min_words=10`
/// from `MINIMAL_CONFIG` × tier-B factor 1.0 = 10) to keep S004 quiet.
const STALE_ADR: &str = "\
# TST-0010. Stale ADR

Date: 2026-01-01
Last-reviewed: 2026-01-01
Tier: B
Status: Superseded by TST-0001

## Related

Supersedes: TST-0001

## Retirement

Superseded by TST-0001 on 2026-04-27. The newer ADR provides better
guidance, replacing every rule and constraint of this retired
decision with an updated normative statement.
";

/// ADR with non-sequential tagged rule IDs (gap: R1, R3).
const GAP_RULES_ADR: &str = "\
# TST-0007. Gap Rules ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0007

## Context

This ADR has tagged rules with a gap in numbering.

## Decision

R1 [5]: First rule is present.
R3 [5]: Third rule skips R2.

## Consequences

The linter should report a T016 warning for non-sequential rule IDs.
";

/// ADR using legacy `## Status` section format (triggers T005c).
const LEGACY_STATUS_ADR: &str = "\
# TST-0011. Legacy Status Format

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B

## Status

Accepted

## Related

Root: TST-0011

## Context

This ADR uses the legacy section format for status instead of the preamble field.

## Decision

R1 [5]: Legacy status section format should produce a T005c migration warning.

## Consequences

The linter should report a T005c warning suggesting migration to preamble format.
";

/// ADR missing the Date field (triggers T002).
const MISSING_DATE_ADR: &str = "\
# TST-0013. Missing Date ADR

Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0013

## Context

This ADR intentionally omits the Date field to trigger T002.

## Decision

R1 [5]: Omit the Date field to verify T002 fires.

## Consequences

The linter should report a T002 warning for the missing Date field.
";

/// Create test corpus in a tempdir with optional multi-domain support.
///
/// Layout:
///   <tempdir>/adr-fmt.toml          (marker — discovery walks up to here)
///   <tempdir>/docs/adr/<domain>/... (corpus content)
///   <tempdir>/docs/adr/stale/...    (optional stale ADRs)
///
/// `domains` is a slice of (`domain_directory`, &[(filename, content)]) tuples.
/// `stale_adrs` is an optional slice of (filename, content) for the stale directory.
fn setup_multi_corpus(
    config: &str,
    domains: &[(&str, &[(&str, &str)])],
    stale_adrs: &[(&str, &str)],
) -> TempDir {
    let dir = TempDir::new().expect("create tempdir");
    let adr_root = dir.path().join("docs/adr");

    fs::create_dir_all(&adr_root).expect("create adr root");
    // Marker file lives at the workspace root (tempdir root), not at adr_root.
    fs::write(dir.path().join("adr-fmt.toml"), config).expect("write config");

    for (domain_dir_name, adrs) in domains {
        let domain_dir = adr_root.join(domain_dir_name);
        fs::create_dir_all(&domain_dir).expect("create domain dir");
        for (filename, content) in *adrs {
            fs::write(domain_dir.join(filename), content).expect("write ADR");
        }
    }

    if !stale_adrs.is_empty() {
        let stale_dir = adr_root.join("stale");
        fs::create_dir_all(&stale_dir).expect("create stale dir");
        for (filename, content) in stale_adrs {
            fs::write(stale_dir.join(filename), content).expect("write stale ADR");
        }
    }

    dir
}

/// Create simple single-domain corpus.
fn setup_corpus(config: &str, adrs: &[(&str, &str)]) -> TempDir {
    setup_multi_corpus(config, &[("test", adrs)], &[])
}

fn adr_fmt() -> Command {
    Command::cargo_bin("adr-fmt").expect("binary exists")
}

/// Build a Command rooted at the marker directory so discovery walks up
/// from there. Replaces the legacy positional `<ADR_DIR>` argument.
fn adr_fmt_in(dir: &TempDir) -> Command {
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path());
    cmd
}

// ── default mode (guidelines) ──────────────────────────────────────

#[test]
fn default_mode_with_config_shows_governance() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir).assert().success().stdout(
        predicate::str::contains("ADR Governance Reference")
            .and(predicate::str::contains("MODES"))
            .and(predicate::str::contains("TAGGED RULES")),
    );
}

#[test]
fn default_mode_without_config_shows_setup_guide() {
    // No adr-fmt.toml anywhere up the chain → setup guide is printed.
    let dir = TempDir::new().expect("create tempdir");

    adr_fmt_in(&dir)
        .assert()
        .success()
        .stdout(predicate::str::contains("adr-fmt").and(predicate::str::contains("QUICK START")));
}

// ── lint mode ──────────────────────────────────────────────────────

#[test]
fn valid_corpus_clean_output() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 warning(s)"));
}

// ── legacy `[[rules]]` deprecation surface ─────────────────────────
//
// AFM-0003: the advisory channel must remain credible. The legacy
// rule-declaration format (with `category` + `description` populated
// inline rather than relying on hardcoded definitions) is deprecated;
// users still on it must see exactly one stderr `warning:` per run
// against their *selected* marker. Walk-up traversal uses `load_quiet`
// so skipped markers do not pollute stderr; the warning fires once in
// `main.rs` after marker selection. This test pins both halves.

#[test]
fn legacy_rule_declaration_emits_deprecation_warning() {
    // Config carries a legacy [[rules]] block with category+description
    // populated, mimicking pre-AFM-0009 configs that haven't been
    // migrated to the override-only format.
    let legacy_config = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "TST"
name = "Test Domain"
directory = "test"
description = "Integration test domain."
crates = ["test-core"]

[[rules]]
id = "T015"
category = "template"
description = "Section word count limits."
params = { min_words = 10 }
"#;

    let dir = setup_corpus(legacy_config, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir).args(["--lint"]).assert().success().stderr(
        predicate::str::contains("warning:")
            .and(predicate::str::contains("legacy rule declaration")),
    );
}

#[test]
fn dangling_link_produces_l001() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0003-dangling-link-adr.md", DANGLING_LINK_ADR),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("L001"));
}

#[test]
fn legacy_verb_produces_l006() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0005-legacy-verb-adr.md", LEGACY_VERB_ADR),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[L006]"))
        .stdout(predicate::str::contains("Depends on"))
        .stdout(predicate::str::contains("AFM-0009"));
}

#[test]
fn empty_domain_directory_graceful() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("0 ADR(s)"));
}

#[test]
fn lint_output_on_stdout() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    // Verify diagnostics go to stdout
    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Diagnostics"));
}

// ── T016 tagged rules ──────────────────────────────────────────────

#[test]
fn t016_missing_tagged_rules() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[("TST-0004-no-tagged-rules.md", NO_TAGGED_RULES_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T016"));
}

#[test]
fn t016_draft_not_exempt() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0005-draft-adr.md", DRAFT_ADR)]);

    // Draft ADRs are no longer exempt from T016 — should appear in lint output
    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T016"));
}

#[test]
fn t016_gap_in_rule_ids() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[("TST-0007-gap-rules-adr.md", GAP_RULES_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T016"));
}

#[test]
fn t016_tagged_rules_present_no_warning() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    // VALID_ADR has tagged rules — no T016
    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T016").not());
}

// ── T005c legacy status section ────────────────────────────────────

#[test]
fn t005c_legacy_status_section() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[("TST-0011-legacy-status-format.md", LEGACY_STATUS_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T005c"));
}

#[test]
fn t005c_preamble_status_field_no_warning() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    // VALID_ADR uses `Status: Accepted` preamble field — no T005c
    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T005c").not());
}

// ── T002 missing Date field ─────────────────────────────────────────

#[test]
fn t002_missing_date_field() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[("TST-0013-missing-date.md", MISSING_DATE_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("T002"));
}

// ── T016 layer warning → lint exit(0), advisory per AFM-0003 ───────

/// ADR with invalid layer (0) — triggers warning-severity T016.
const INVALID_LAYER_ADR: &str = "\
# TST-0012. Invalid Layer ADR

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: TST-0001

## Context

ADR with an invalid Meadows layer annotation to test warning emission.

## Decision

R1 [0]: This rule has an invalid layer zero which must trigger a warning.

## Consequences

Lint completes successfully (exit 0) and emits a T016 warning per AFM-0003
advisory-only semantics. CI wrappers parse warning counts for enforcement.
";

#[test]
fn lint_warns_on_invalid_layer() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0012-invalid-layer.md", INVALID_LAYER_ADR),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[T016]"))
        .stdout(predicate::str::contains("layer 0"));
}

/// ADR with two invalid layer annotations — produces two T016 warnings.
const TWO_INVALID_LAYERS_ADR: &str = "\
# TST-0013. Two Invalid Layers ADR

Date: 2026-04-29
Last-reviewed: 2026-04-29
Tier: B
Status: Accepted

## Related

References: TST-0001

## Context

ADR with two invalid Meadows layer annotations to test multi-warning exit-zero
behavior under AFM-0003 advisory-only semantics.

## Decision

R1 [0]: First rule has an invalid layer zero which must trigger a warning.
R2 [13]: Second rule has an invalid layer thirteen which must also warn.

## Consequences

Two T016 warnings emitted; lint exits 0 per AFM-0003 R1.
";

/// AFM-0003 R1/R3 contract test: multiple warnings, exit 0, exact header
/// format `## Diagnostics: N warning(s)`, no `error(s)` substring.
#[test]
fn lint_multiple_warnings_exits_zero() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0013-two-invalid-layers.md", TWO_INVALID_LAYERS_ADR),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[T016]"))
        .stdout(predicate::str::contains("layer 0"))
        .stdout(predicate::str::contains("layer 13"))
        .stdout(predicate::str::contains("## Diagnostics:"))
        .stdout(predicate::str::contains("warning(s)"))
        .stdout(predicate::str::contains("error(s)").not());
}

// ── refs mode ──────────────────────────────────────────────────────

#[test]
fn refs_returns_inbound_refs() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0002-referencing-adr.md", REFERENCING_ADR),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--refs", "TST-0001"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("◆ REFS: TST-0001")
                .and(predicate::str::contains("- TST-0002 [References]"))
                .and(predicate::str::contains("Tier: B"))
                .and(predicate::str::contains("Status: Accepted")),
        );
}

#[test]
fn refs_empty_for_isolated_target() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    // Isolated ADR: header rendered, then "No references found."
    adr_fmt_in(&dir)
        .args(["--refs", "TST-0001"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("◆ REFS: TST-0001")
                .and(predicate::str::contains("No references found.")),
        );
}

#[test]
fn refs_invalid_id_exits_nonzero() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--refs", "INVALID"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a valid ADR ID"));
}

#[test]
fn refs_unknown_adr_exits_nonzero() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--refs", "TST-9999"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn refs_excludes_stale_referrers() {
    let dir = setup_multi_corpus(
        MINIMAL_CONFIG,
        &[("test", &[("TST-0001-valid-test-adr.md", VALID_ADR)])],
        &[("TST-0010-stale-adr.md", STALE_ADR)],
    );

    // TST-0001 is referenced by stale TST-0010 (Supersedes edge);
    // stale referrers must be filtered out — empty refs list expected.
    adr_fmt_in(&dir)
        .args(["--refs", "TST-0001"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("◆ REFS: TST-0001")
                .and(predicate::str::contains("No references found."))
                .and(predicate::str::contains("TST-0010").not()),
        );
}

/// AFM-0022 / S007 regression pin: a stub-form stale ADR (preamble,
/// optional `## Related` with `Supersedes:` only, `## Retirement`)
/// must produce zero diagnostics from the stub-aware rule set
/// (S007, T007, T008, T009, T010, T016) under `--lint`.
///
/// Catches future rule additions or guard regressions silently
/// breaking the stub policy. T015 is also asserted because
/// Retirement-section word-count violations route through it.
#[test]
fn lint_stale_stub_emits_no_stub_aware_warnings() {
    let dir = setup_multi_corpus(
        MINIMAL_CONFIG,
        &[("test", &[("TST-0001-valid-test-adr.md", VALID_ADR)])],
        &[("TST-0010-stale-adr.md", STALE_ADR)],
    );

    let assert = adr_fmt_in(&dir).args(["--lint"]).assert().success();
    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Confirm the stale stub was actually processed (guards against a
    // vacuous pass if the path filter misses every diagnostic).
    assert!(
        stdout.contains("188 ADR(s)") || stdout.contains("ADR(s)"),
        "lint output should report scanned-ADR count:\n{stdout}"
    );

    // Tolerate POSIX and Windows path separators when filtering.
    let stale_lines: Vec<&str> = stdout
        .lines()
        .filter(|l| l.contains("stale/TST-0010") || l.contains("stale\\TST-0010"))
        .collect();

    for rule in ["S007", "T007", "T008", "T009", "T010", "T015", "T016"] {
        let bracket = format!("[{rule}]");
        for line in &stale_lines {
            assert!(
                !line.contains(&bracket),
                "rule {rule} fired on stale stub (per AFM-0022 must not):\n{line}\n\nfull stale-line set:\n{}",
                stale_lines.join("\n")
            );
        }
    }
}

#[test]
fn refs_stale_target_returns_live_referrers() {
    // AFM-0021 R3: querying a stale ADR succeeds and lists live
    // referrers. The target itself is in stale/, but referrers
    // outside stale/ must still be surfaced.
    let stale_target: &str = "\
# TST-0050. Stale Target

Date: 2026-01-01
Last-reviewed: 2026-01-01
Tier: B
Status: Superseded by TST-0001

## Related

References: TST-0001

## Context

Stale ADR used as a --refs target to verify live referrers still surface.

## Decision

R1 [5]: This decision was superseded but stays queryable for archaeology.

## Consequences

`--refs TST-0050` succeeds and lists live referrers (TST-0051).

## Retirement

Superseded by TST-0001 on 2026-04-27. Kept queryable for tests.
";
    let live_referrer: &str = "\
# TST-0051. Live Referrer

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-0050

## Context

Live ADR pointing at a stale target to verify --refs still finds it.

## Decision

R1 [5]: We reference a stale target to test stale-target query semantics.

## Consequences

`--refs TST-0050` must list TST-0051 even though target is in stale/.
";
    let dir = setup_multi_corpus(
        MINIMAL_CONFIG,
        &[("test", &[("TST-0051-live-referrer.md", live_referrer)])],
        &[("TST-0050-stale-target.md", stale_target)],
    );

    adr_fmt_in(&dir)
        .args(["--refs", "TST-0050"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("◆ REFS: TST-0050")
                .and(predicate::str::contains("- TST-0051 [References]"))
                .and(predicate::str::contains("No references found.").not()),
        );
}

#[test]
fn refs_renders_sort_order_across_tiers() {
    // Sort key: tier rank (S < A < B) → prefix → number → verb.
    // S-tier referrer must appear before A-tier; A-tier before B-tier.
    let s_tier: &str = "\
# TST-0030. S-tier Referrer

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: S
Status: Accepted

## Related

References: TST-0001

## Context

S-tier referrer for sort-order test in --refs output rendering.

## Decision

R1 [5]: S-tier rule referencing the test target for sort verification.

## Consequences

Must appear first in --refs TST-0001 output.
";
    let a_tier: &str = "\
# TST-0031. A-tier Referrer

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: A
Status: Accepted

## Related

References: TST-0001

## Context

A-tier referrer for sort-order test in --refs output rendering.

## Decision

R1 [5]: A-tier rule referencing the test target for sort verification.

## Consequences

Must appear after S-tier and before B-tier in --refs TST-0001 output.
";
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0030-s-tier.md", s_tier),
            ("TST-0031-a-tier.md", a_tier),
            ("TST-0002-referencing-adr.md", REFERENCING_ADR),
        ],
    );

    let out = adr_fmt_in(&dir)
        .args(["--refs", "TST-0001"])
        .output()
        .expect("binary runs");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    let s_pos = stdout.find("TST-0030").expect("S-tier referrer missing");
    let a_pos = stdout.find("TST-0031").expect("A-tier referrer missing");
    let b_pos = stdout.find("TST-0002").expect("B-tier referrer missing");
    assert!(
        s_pos < a_pos && a_pos < b_pos,
        "expected order S(0030) < A(0031) < B(0002), got positions {s_pos}/{a_pos}/{b_pos}\noutput:\n{stdout}"
    );
}

#[test]
fn refs_includes_cross_domain_referrers() {
    // --refs has no domain filter: a TST referrer to COM-0001 must
    // surface in --refs COM-0001 output.
    let cross_referrer: &str = "\
# TST-0040. Cross-domain Referrer

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted
Parent-cross-domain: COM-0001 — boundary ADR

## Related

References: COM-0001

## Context

TST-domain ADR referencing a COM-domain target to test cross-domain refs.

## Decision

R1 [5]: We reference a foundation principle from a domain ADR for the test.

## Consequences

`--refs COM-0001` must list TST-0040 (no domain filtering applied).
";
    let dir = setup_multi_corpus(
        MULTI_DOMAIN_CONFIG,
        &[
            (
                "common",
                &[("COM-0001-foundation-principle.md", FOUNDATION_ADR)],
            ),
            (
                "test",
                &[("TST-0040-cross-domain-referrer.md", cross_referrer)],
            ),
        ],
        &[],
    );

    adr_fmt_in(&dir)
        .args(["--refs", "COM-0001"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("◆ REFS: COM-0001")
                .and(predicate::str::contains("- TST-0040 [References]")),
        );
}

#[test]
fn refs_renders_duplicate_verbs_same_source() {
    // A single source ADR with both `References:` and `Supersedes:`
    // edges to the same target must appear twice in --refs output:
    // once under [References], once under [Supersedes]. References
    // sorts before Supersedes (same source/tier/number).
    let dual_edge: &str = "\
# TST-0060. Dual-edge Referrer

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-0001
Supersedes: TST-0001

## Context

Source with both References and Supersedes to the same target — verifies
that --refs renders both edges as distinct rows.

## Decision

R1 [5]: We reference and supersede the same target to test verb dedup.

## Consequences

`--refs TST-0001` must render TST-0060 twice (References then Supersedes).
";
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0060-dual-edge.md", dual_edge),
        ],
    );

    let out = adr_fmt_in(&dir)
        .args(["--refs", "TST-0001"])
        .output()
        .expect("binary runs");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);

    let refs_pos = stdout
        .find("- TST-0060 [References]")
        .expect("References row missing");
    let supersedes_pos = stdout
        .find("- TST-0060 [Supersedes]")
        .expect("Supersedes row missing");
    assert!(
        refs_pos < supersedes_pos,
        "References row must precede Supersedes row, got {refs_pos}/{supersedes_pos}\noutput:\n{stdout}"
    );
}

// ── context mode ───────────────────────────────────────────────────

#[test]
fn context_shows_crate_rules() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--context", "test-core"])
        .assert()
        .success()
        .stdout(predicate::str::contains("test-core").and(predicate::str::contains("TST-0001")));
}

#[test]
fn context_unknown_crate_exits_nonzero() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--context", "unknown-crate"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

#[test]
fn context_includes_foundation() {
    let dir = setup_multi_corpus(
        MULTI_DOMAIN_CONFIG,
        &[
            (
                "common",
                &[("COM-0001-foundation-principle.md", FOUNDATION_ADR)],
            ),
            ("test", &[("TST-0001-valid-test-adr.md", VALID_ADR)]),
        ],
        &[],
    );

    // Foundation domain ADRs should always be included
    adr_fmt_in(&dir)
        .args(["--context", "test-core"])
        .assert()
        .success()
        .stdout(predicate::str::contains("COM-0001").and(predicate::str::contains("TST-0001")));
}

#[test]
fn context_per_adr_crates_filtering() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0006-adr-with-crates.md", ADR_WITH_CRATES),
        ],
    );

    // TST-0006 has Crates: test-core, test-api — should be included
    adr_fmt_in(&dir)
        .args(["--context", "test-core"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TST-0006"));
}

// ── context output format (end-to-end) ─────────────────────────────

/// ADR with multi-line tagged rules for end-to-end context output test.
const MULTILINE_RULES_ADR: &str = "\
# TST-0008. Multi-line Rules ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0008

## Context

This ADR tests multi-line tagged rule extraction through context mode output.

## Decision

R1 [5]: Use explicit versioning on every event payload
  so that consumers can deserialize historical events
  without schema ambiguity.
R2 [5]: Single-line rule stays on one line.

## Consequences

Multi-line rules should be joined and rendered correctly in context output.
";

/// Draft ADR with tagged rules — must NOT appear in context output.
const DRAFT_WITH_RULES_ADR: &str = "\
# TST-0009. Draft With Rules

Date: 2026-04-27
Tier: B
Status: Draft

## Related

Root: TST-0009

## Context

Draft ADR with tagged rules that should be excluded from context output.

## Decision

R1 [5]: This rule must not leak into context output.

## Consequences

Draft exclusion verified.
";

#[test]
fn context_end_to_end_output_format() {
    // Setup: foundation S-tier + domain B-tier (multi-line rules) + draft (excluded)
    let dir = setup_multi_corpus(
        MULTI_DOMAIN_CONFIG,
        &[
            (
                "common",
                &[("COM-0001-foundation-principle.md", FOUNDATION_ADR)],
            ),
            (
                "test",
                &[
                    ("TST-0008-multiline-rules.md", MULTILINE_RULES_ADR),
                    ("TST-0009-draft-with-rules.md", DRAFT_WITH_RULES_ADR),
                ],
            ),
        ],
        &[],
    );

    let output = adr_fmt_in(&dir)
        .args(["--context", "test-core"])
        .output()
        .expect("run adr-fmt");

    assert!(output.status.success(), "adr-fmt should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // ── Preamble ──
    assert!(
        stdout.contains("# Architecture Rules"),
        "missing preamble title:\n{stdout}"
    );
    assert!(
        stdout.contains("crate `test-core`"),
        "missing crate name in preamble:\n{stdout}"
    );
    assert!(
        stdout.contains("Follow every rule without exception"),
        "missing mandate in preamble:\n{stdout}"
    );

    // ── Root-grouped headers in correct order (foundation first) ──
    let com_pos = stdout
        .find("### COM-0001. Foundation Principle")
        .expect("COM-0001 root header missing");
    let tst_pos = stdout
        .find("### TST-0008. Multi-line Rules ADR")
        .expect("TST-0008 root header missing");
    assert!(
        com_pos < tst_pos,
        "Foundation root ({com_pos}) must appear before domain root ({tst_pos})"
    );

    // ── Foundation rule with ID and layer at end ──
    assert!(
        stdout.contains("[COM-0001:R1:L5]"),
        "foundation rule should have ID:layer at end:\n{stdout}"
    );

    // ── Multi-line rule text joined on single line with ID ──
    let r1_line = stdout
        .lines()
        .find(|l| l.contains("[TST-0008:R1:L5]"))
        .expect("R1 line with ID missing");
    assert!(
        r1_line.contains("Use explicit versioning on every event payload"),
        "multi-line R1 start text missing on rule line:\n{r1_line}"
    );
    assert!(
        r1_line.contains("without schema ambiguity."),
        "multi-line R1 continuation text must be on same line as ID:\n{r1_line}"
    );

    // ── Single-line rule ──
    assert!(
        stdout.contains("- Single-line rule stays on one line. [TST-0008:R2:L5]"),
        "single-line R2 format wrong:\n{stdout}"
    );

    // ── Draft exclusion ──
    assert!(
        !stdout.contains("TST-0009"),
        "draft ADR ID must not appear in context output:\n{stdout}"
    );
    assert!(
        !stdout.contains("must not leak"),
        "draft ADR rule text must not appear in context output:\n{stdout}"
    );

    // ── No old metadata noise ──
    assert!(
        !stdout.contains("| Status:"),
        "old status metadata should not appear:\n{stdout}"
    );
    assert!(
        !stdout.contains("| Domain:"),
        "old domain metadata should not appear:\n{stdout}"
    );
}

// ── tree mode ──────────────────────────────────────────────────────

#[test]
fn tree_produces_output() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    // Use -- to separate --tree (no domain filter) from positional ADR_DIR
    adr_fmt_in(&dir)
        .args(["--tree", "--"])
        .assert()
        .success()
        .stdout(predicate::str::contains("TST-0001"));
}

#[test]
fn tree_filtered_by_domain() {
    let dir = setup_multi_corpus(
        MULTI_DOMAIN_CONFIG,
        &[
            (
                "common",
                &[("COM-0001-foundation-principle.md", FOUNDATION_ADR)],
            ),
            ("test", &[("TST-0001-valid-test-adr.md", VALID_ADR)]),
        ],
        &[],
    );

    // Filter to TST domain only
    adr_fmt_in(&dir)
        .args(["--tree", "TST"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("TST-0001").and(predicate::str::contains("COM-0001").not()),
        );
}

#[test]
fn tree_unknown_domain_graceful() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--tree", "NONEXISTENT"])
        .assert()
        .success()
        .stdout(predicate::str::contains("No domain found"));
}

#[test]
fn tree_renders_cross_domain_parented_adrs_with_arrow() {
    // HR1 regression: when an ADR's first References target is in a
    // different domain and `Parent-cross-domain:` declares it, the
    // ADR must render as a top-level entry in its own domain with
    // `↑ <PARENT-ID>` annotation, NOT in the orphans section. Before
    // the fix, --tree silently dropped the cross-domain edge and
    // marked the ADR as orphaned, while --lint stayed quiet (HR2).
    let cross_domain_child = "\
# TST-0002. Cross-domain child
Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted
Parent-cross-domain: COM-0001 — boundary ADR

## Related

References: COM-0001

## Context
Test fixture for cross-domain parent rendering.

## Decision
R1 [5]: This is a cross-domain-parented ADR for tree-render testing.

## Consequences
Renders under TST with ↑ COM-0001 annotation.
";
    let dir = setup_multi_corpus(
        MULTI_DOMAIN_CONFIG,
        &[
            (
                "common",
                &[("COM-0001-foundation-principle.md", FOUNDATION_ADR)],
            ),
            (
                "test",
                &[("TST-0002-cross-domain-child.md", cross_domain_child)],
            ),
        ],
        &[],
    );

    let out = adr_fmt_in(&dir)
        .args(["--tree", "--"])
        .output()
        .expect("binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("TST-0002") && stdout.contains("↑ COM-0001"),
        "TST-0002 must render with ↑ COM-0001 annotation, got:\n{stdout}",
    );
    assert!(
        !stdout.contains("orphans"),
        "cross-domain-parented ADR must not appear in orphans section, got:\n{stdout}",
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "regression test sets up a multi-domain fixture and asserts on the rendered tree byte-for-byte; splitting would either duplicate the fixture or hide the assertion sequence"
)]
fn tree_cross_domain_root_renders_descendants_with_correct_indent() {
    // HR1 regression (rigormortis follow-up): cross-domain forest
    // roots must render their same-domain descendants with the same
    // indent/connector pattern as native-Root subtrees. This test
    // exercises a cross-domain root with a child and a grandchild,
    // and checks that grandchild glyphs match the depth-2 expectation
    // (`     │  └─ ` or `     │  ├─ ` for ongoing siblings, etc.).
    let cross_root = "\
# TST-0002. Cross-domain root
Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted
Parent-cross-domain: COM-0001 — boundary ADR

## Related

References: COM-0001

## Context
Test fixture: cross-domain forest root with descendants.

## Decision
R1 [5]: Cross-domain root with same-domain descendants for indent tests.

## Consequences
Children render at depth 1, grandchild at depth 2.
";
    let child_a = "\
# TST-0003. Child A
Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-0002

## Context
First child of TST-0002.

## Decision
R1 [5]: First child of cross-domain root TST-0002 for indent test.

## Consequences
None.
";
    let child_b = "\
# TST-0004. Child B
Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-0002

## Context
Second child of TST-0002.

## Decision
R1 [5]: Second child of cross-domain root TST-0002 for indent test.

## Consequences
None.
";
    let grandchild = "\
# TST-0005. Grandchild
Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

References: TST-0003

## Context
Grandchild via TST-0003.

## Decision
R1 [5]: Grandchild of cross-domain root via TST-0003 for indent test.

## Consequences
None.
";
    let dir = setup_multi_corpus(
        MULTI_DOMAIN_CONFIG,
        &[
            (
                "common",
                &[("COM-0001-foundation-principle.md", FOUNDATION_ADR)],
            ),
            (
                "test",
                &[
                    ("TST-0002-cross-domain-root.md", cross_root),
                    ("TST-0003-child-a.md", child_a),
                    ("TST-0004-child-b.md", child_b),
                    ("TST-0005-grandchild.md", grandchild),
                ],
            ),
        ],
        &[],
    );

    let out = adr_fmt_in(&dir)
        .args(["--tree", "TST"])
        .output()
        .expect("binary runs");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // The cross-domain root renders at column 2 with no connector.
    assert!(
        stdout.contains("  TST-0002"),
        "cross-domain root must render at column 2 without connector, got:\n{stdout}",
    );
    // Children render with the same depth-1 indent as native-root children.
    // TST-0003 is the first child (not last) → ├─, TST-0004 last → └─.
    assert!(
        stdout.contains("     ├─ TST-0003"),
        "first child must render with ├─ at depth 1, got:\n{stdout}",
    );
    assert!(
        stdout.contains("     └─ TST-0004"),
        "last child must render with └─ at depth 1, got:\n{stdout}",
    );
    // Grandchild under TST-0003: depth 2. Since TST-0003 has more
    // siblings (TST-0004 follows), col-5 must be `│`. Grandchild is
    // last (and only) child so it gets `└─` at col 8.
    assert!(
        stdout.contains("     │  └─ TST-0005"),
        "grandchild must render with │ continuation at col 5 and └─ at col 8, got:\n{stdout}",
    );
    // Orphans section must not contain any of the test ADRs.
    let orphans_section = stdout.split("orphans").nth(1).unwrap_or("");
    assert!(
        !orphans_section.contains("TST-0002")
            && !orphans_section.contains("TST-0003")
            && !orphans_section.contains("TST-0004")
            && !orphans_section.contains("TST-0005"),
        "no test ADR may appear in orphans section, got:\n{stdout}",
    );
}

// ── mutual exclusion ───────────────────────────────────────────────

#[test]
fn refs_and_context_mutually_exclusive() {
    adr_fmt()
        .args(["--refs", "TST-0001", "--context", "test-core"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn lint_and_refs_mutually_exclusive() {
    adr_fmt()
        .args(["--lint", "--refs", "TST-0001"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn refs_and_tree_mutually_exclusive() {
    adr_fmt()
        .args(["--refs", "TST-0001", "--tree"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

// ── infrastructure errors ──────────────────────────────────────────

#[test]
fn help_flag_shows_usage() {
    adr_fmt()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("adr-fmt"));
}

#[test]
fn version_flag_shows_version() {
    adr_fmt()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("adr-fmt"));
}

// ── read-only verification ─────────────────────────────────────────

#[test]
fn no_files_modified_after_lint() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    // Snapshot directory contents before
    let adr_dir = dir.path().join("docs/adr");
    let before: Vec<_> = walkdir(&adr_dir);

    adr_fmt_in(&dir).args(["--lint"]).assert().success();

    // Verify no new files or modifications
    let after: Vec<_> = walkdir(&adr_dir);
    assert_eq!(before, after, "lint mode should not create or modify files");
}

#[test]
fn no_files_modified_after_refs() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    let adr_dir = dir.path().join("docs/adr");
    let before: Vec<_> = walkdir(&adr_dir);

    adr_fmt_in(&dir)
        .args(["--refs", "TST-0001"])
        .assert()
        .success();

    let after: Vec<_> = walkdir(&adr_dir);
    assert_eq!(before, after, "refs mode should not create or modify files");
}

/// Recursively list all files under a directory (sorted, relative paths).
fn walkdir(root: &std::path::Path) -> Vec<String> {
    let mut entries = Vec::new();
    walk_recursive(root, root, &mut entries);
    entries.sort();
    entries
}

fn walk_recursive(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_recursive(base, &path, out);
            } else {
                let rel = path.strip_prefix(base).unwrap().display().to_string();
                out.push(rel);
            }
        }
    }
}

// ── path containment ───────────────────────────────────────────────

/// Config with an absolute domain directory escapes the ADR root.
const ABSOLUTE_DOMAIN_CONFIG: &str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "TST"
name = "Test"
directory = "/etc"
description = "Malicious absolute path."
crates = []
"#;

/// Config with a parent-traversal domain directory escapes the ADR root.
const TRAVERSAL_DOMAIN_CONFIG: &str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "TST"
name = "Test"
directory = "../../../etc"
description = "Malicious parent traversal."
crates = []
"#;

/// Config with a parent-traversal stale directory.
const TRAVERSAL_STALE_CONFIG: &str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "../escape"

[[domains]]
prefix = "TST"
name = "Test"
directory = "test"
description = "Valid domain."
crates = []
"#;

#[test]
fn containment_rejects_absolute_domain_directory() {
    let dir = setup_corpus(
        ABSOLUTE_DOMAIN_CONFIG,
        &[("TST-0001-valid-test-adr.md", VALID_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("domain 'TST'"))
        .stderr(predicate::str::contains("absolute"));
}

/// Lexical absolute-path rejection must fire before any filesystem
/// canonicalization is attempted. Pointing at a definitely-non-existent
/// absolute path proves the lexical check runs first: if it didn't,
/// the error would be `CanonicalizeFailed` ("No such file or directory")
/// instead of `Absolute`.
const NONEXISTENT_ABSOLUTE_CONFIG: &str = r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "TST"
name = "Test"
directory = "/this/path/should/never/exist/on/any/system"
description = "Lexical-check-first guarantee."
crates = []
"#;

#[test]
fn containment_lexical_check_fires_before_canonicalize() {
    let dir = setup_corpus(
        NONEXISTENT_ABSOLUTE_CONFIG,
        &[("TST-0001-valid-test-adr.md", VALID_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("is absolute"))
        .stderr(predicate::str::contains("cannot canonicalize").not());
}

#[test]
fn containment_rejects_parent_traversal_domain_directory() {
    let dir = setup_corpus(
        TRAVERSAL_DOMAIN_CONFIG,
        &[("TST-0001-valid-test-adr.md", VALID_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("domain 'TST'"))
        .stderr(predicate::str::contains("parent-traversal"));
}

#[test]
fn containment_rejects_parent_traversal_stale_directory() {
    let dir = setup_corpus(
        TRAVERSAL_STALE_CONFIG,
        &[("TST-0001-valid-test-adr.md", VALID_ADR)],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("stale directory"))
        .stderr(predicate::str::contains("parent-traversal"));
}

#[cfg(unix)]
#[test]
fn containment_rejects_symlink_escape_in_domain_directory() {
    use std::os::unix::fs::symlink;

    // Build a corpus with a domain directory that is a symlink pointing
    // outside the ADR root. The lint must abort with an EscapesRoot error.
    let dir = TempDir::new().expect("create tempdir");
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).expect("create outside");
    let adr_root_path = dir.path().join("docs/adr");
    fs::create_dir_all(&adr_root_path).expect("create adr root");
    fs::write(
        dir.path().join("adr-fmt.toml"),
        r#"
[corpus]
root = "docs/adr"

[stale]
directory = "stale"

[[domains]]
prefix = "TST"
name = "Test"
directory = "test"
description = "Symlink-escape domain."
crates = []
"#,
    )
    .expect("write config");
    symlink(&outside, adr_root_path.join("test")).expect("create symlink");

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("domain 'TST'"))
        .stderr(predicate::str::contains("escapes the ADR root"));
}

/// Symlinks whose canonical target stays inside the ADR root are
/// allowed — strict containment must not be over-strict. This locks
/// the positive case so future hardening doesn't accidentally reject
/// in-tree symlink farms.
#[cfg(unix)]
#[test]
fn containment_accepts_symlink_inside_root() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create tempdir");
    let adr_root_path = dir.path().join("docs/adr");
    fs::create_dir_all(&adr_root_path).expect("create adr root");
    fs::write(dir.path().join("adr-fmt.toml"), MINIMAL_CONFIG).expect("write config");

    // Real `test/` directory at adr_root/real-test, then symlink test → real-test.
    let real = adr_root_path.join("real-test");
    fs::create_dir(&real).expect("create real test dir");
    fs::write(real.join("TST-0001-valid-test-adr.md"), VALID_ADR).expect("write ADR");
    symlink(&real, adr_root_path.join("test")).expect("create symlink");

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning(s)"));
}

// ── Parser-stage diagnostics (AFM-0017) ─────────────────────────────

/// A file matching the prefix filename pattern but missing its H1
/// title must surface as a `P002` warning instead of being silently
/// dropped from the corpus. Lint exits 0 (advisory-only per
/// AFM-0003 R1).
#[test]
fn parser_p002_missing_title_emits_warning() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            (
                "TST-0002-no-h1-title.md",
                "Some prose without an H1 header at all.\n",
            ),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[P002]"))
        .stdout(predicate::str::contains("missing or malformed"))
        .stdout(predicate::str::contains("TST-0002-no-h1-title.md"));
}

/// An empty file matching the filename pattern must surface as
/// `P002` (empty file variant) rather than being silently dropped.
#[test]
fn parser_p002_empty_file_emits_warning() {
    let dir = setup_corpus(
        MINIMAL_CONFIG,
        &[
            ("TST-0001-valid-test-adr.md", VALID_ADR),
            ("TST-0003-empty-file.md", ""),
        ],
    );

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[P002]"))
        .stdout(predicate::str::contains("empty"))
        .stdout(predicate::str::contains("TST-0003-empty-file.md"));
}

/// A directory whose name matches the ADR filename pattern (e.g.
/// `TST-0004-actually-a-dir.md/`) causes `fs::read_to_string` to
/// fail with EISDIR. The parser surfaces this as `P001` rather than
/// silently dropping the entry.
#[cfg(unix)]
#[test]
fn parser_p001_unreadable_file_emits_warning() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);
    let test_subdir = dir
        .path()
        .join("docs/adr/test")
        .join("TST-0004-actually-a-dir.md");
    fs::create_dir(&test_subdir).expect("create masquerading dir");

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[P001]"))
        .stdout(predicate::str::contains("cannot read ADR file"))
        .stdout(predicate::str::contains("TST-0004-actually-a-dir.md"));
}

/// Valid corpus must not emit any P-codes — proves the parser
/// stays quiet when nothing is wrong. Asserts on the `warning[P`
/// substring so future P003+ codes are also caught.
#[test]
fn parser_no_p_codes_for_valid_corpus() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-valid-test-adr.md", VALID_ADR)]);

    adr_fmt_in(&dir)
        .args(["--lint"])
        .assert()
        .success()
        .stdout(predicate::str::contains("warning[P").not());
}

// ── real-corpus pin tests ─────────────────────────────────────────
//
// These run against the live `docs/adr/` corpus to pin the
// parent-edge tree model. They protect against silent regressions
// where a refactor to nav/output/links would, e.g., reintroduce
// orphans, fire structural diagnostics on a known-clean corpus,
// or drop ADRs from the tree view.

/// Locate the workspace root directory (containing `adr-fmt.toml`)
/// from this crate's manifest dir. Workspace layout:
/// `<root>/crates/adr-fmt/Cargo.toml`.
fn workspace_root() -> std::path::PathBuf {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent() // crates/
        .and_then(|p| p.parent()) // workspace root
        .expect("workspace root exists")
        .to_owned()
}

/// Build a Command with cwd set to the real workspace root, so
/// adr-fmt's discovery walks up to the workspace `adr-fmt.toml`.
fn adr_fmt_at_workspace() -> Command {
    let mut cmd = adr_fmt();
    cmd.current_dir(workspace_root());
    cmd
}

/// The real corpus must produce ZERO structural-defect diagnostics
/// (L010/L011/L012/L013/L014/L017). L015 (root-first heuristic) and
/// L016 (lower-tier parent) are migration-pending and may fire.
///
/// Failure mode: a parent-edge or cycle-detection regression makes
/// previously-clean ADRs trip a rule that does not reflect a real
/// authoring defect.
#[test]
fn real_corpus_clean_of_structural_defects() {
    let output = adr_fmt_at_workspace()
        .args(["--lint"])
        .output()
        .expect("binary runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for rule in ["L010", "L011", "L012", "L013", "L014", "L017"] {
        let bracket = format!("warning[{rule}]");
        assert!(
            !stdout.contains(&bracket),
            "{rule} fired against the real corpus, which should be \
             structurally clean. Output excerpt:\n{}\n\nIf this is a \
             genuine new defect, fix the corpus; if it is a model \
             regression, fix the rule.",
            stdout
                .lines()
                .filter(|l| l.contains(&bracket))
                .take(5)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

/// `--tree` against the real corpus must render every non-stale
/// ADR somewhere — either as a node in a parent-edge tree OR in the
/// per-domain orphan section (categorized as "no References", "chain
/// ends at non-root", or "cycle"). No ADR may silently disappear.
///
/// Pin: count of `<PREFIX>-NNNN` ID occurrences in `--tree` stdout
/// must be ≥ count of non-stale ADRs in the corpus. (≥ because
/// `[also: …]` annotations and per-record orphan listings can
/// repeat IDs; the pin is a lower bound.)
///
/// Counting strategy: walk `docs/adr/` directly (excluding `stale/`)
/// rather than parsing the lint summary, which is format-coupled.
#[test]
fn real_corpus_tree_covers_every_adr() {
    let root = workspace_root()
        .join("docs/adr")
        .to_string_lossy()
        .into_owned();

    // Count non-stale ADR files: any `.md` file matching the ADR ID
    // pattern in `docs/adr/<domain>/`, excluding `stale/` archive.
    let non_stale_count = count_non_stale_adrs(&root);
    assert!(
        non_stale_count > 0,
        "corpus has zero non-stale ADRs — test setup broken"
    );

    let tree_out = adr_fmt_at_workspace()
        .args(["--tree", "--"])
        .output()
        .expect("binary runs");
    let tree_stdout = String::from_utf8_lossy(&tree_out.stdout);

    let ids = extract_adr_ids(&tree_stdout);

    assert!(
        ids.len() >= non_stale_count,
        "tree output covered {} distinct ADR IDs but corpus has {} non-stale ADRs — \
         some ADRs are silently missing from --tree output",
        ids.len(),
        non_stale_count,
    );
}

/// Walk `<root>/<domain>/*.md` and count files whose basename matches
/// the ADR-ID pattern (`<PREFIX>-NNNN-…md`). Skips the `stale/`
/// archive subdirectory and any non-domain top-level entries.
fn count_non_stale_adrs(root: &str) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Skip the stale archive
        if path.file_name().is_some_and(|n| n == "stale") {
            continue;
        }
        let Ok(domain_entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for adr in domain_entries.flatten() {
            let adr_path = adr.path();
            if !adr_path.is_file() {
                continue;
            }
            let Some(name) = adr_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // ADR filenames are lowercase by convention (ledger L6), but
            // use Path::extension for the canonical .md check.
            if std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
                && parse_adr_id_prefix(name).is_some()
            {
                count += 1;
            }
        }
    }
    count
}

/// Extract all ADR IDs (`<PREFIX>-NNNN` where prefix is ≥ 2 uppercase
/// ASCII letters and number is ≥ 4 ASCII digits) from arbitrary text.
/// Used to count IDs in `--tree` output without coupling to format.
fn extract_adr_ids(text: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for word in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
        if let Some(id) = parse_adr_id_prefix(word) {
            out.insert(id);
        }
    }
    out
}

/// Parse `<PREFIX>-NNNN` from the start of `word`. Prefix is ≥ 2
/// uppercase ASCII letters; number is ≥ 4 ASCII digits. Returns the
/// matched prefix as `Some(<PREFIX>-NNNN)` (excluding any trailing
/// chars from the input).
fn parse_adr_id_prefix(word: &str) -> Option<String> {
    let bytes = word.as_bytes();
    let dash = bytes.iter().position(|&b| b == b'-')?;
    if dash < 2 {
        return None;
    }
    if !bytes[..dash].iter().all(u8::is_ascii_uppercase) {
        return None;
    }
    let digits_start = dash + 1;
    let digits_end = bytes[digits_start..]
        .iter()
        .position(|b| !b.is_ascii_digit())
        .map_or(bytes.len(), |p| digits_start + p);
    let digit_count = digits_end - digits_start;
    if digit_count < 4 {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[..digits_end]).into_owned())
}

#[cfg(test)]
mod adr_id_extraction_tests {
    use super::{extract_adr_ids, parse_adr_id_prefix};

    #[test]
    fn three_letter_prefix() {
        assert_eq!(parse_adr_id_prefix("CHE-0001"), Some("CHE-0001".to_owned()));
    }

    #[test]
    fn four_letter_prefix() {
        assert_eq!(
            parse_adr_id_prefix("PARD-0042"),
            Some("PARD-0042".to_owned())
        );
    }

    #[test]
    fn two_letter_prefix() {
        assert_eq!(parse_adr_id_prefix("XY-0001"), Some("XY-0001".to_owned()));
    }

    #[test]
    fn one_letter_prefix_rejected() {
        assert_eq!(parse_adr_id_prefix("X-0001"), None);
    }

    #[test]
    fn three_digit_number_rejected() {
        assert_eq!(parse_adr_id_prefix("CHE-001"), None);
    }

    #[test]
    fn lowercase_prefix_rejected() {
        assert_eq!(parse_adr_id_prefix("che-0001"), None);
    }

    #[test]
    fn id_in_filename_truncated_at_extension() {
        assert_eq!(
            parse_adr_id_prefix("CHE-0001-foo.md"),
            Some("CHE-0001".to_owned()),
        );
    }

    #[test]
    fn extracts_from_multiline_text() {
        let text = "── CHE-0001\n│   └── CHE-0002 [also: COM-0003]\n";
        let ids = extract_adr_ids(text);
        assert_eq!(ids.len(), 3);
        assert!(ids.contains("CHE-0001"));
        assert!(ids.contains("CHE-0002"));
        assert!(ids.contains("COM-0003"));
    }
}

// ─── marker discovery (walk-up) ──────────────────────────────────────
//
// These tests pin the discovery contract introduced when the corpus
// path moved from a positional CLI argument to walk-up search for an
// `adr-fmt.toml` with a valid `[corpus]` table. The CLI no longer
// accepts an ADR-directory argument; discovery is the SSOT.

/// Marker discovery walks up from a deeply-nested CWD inside the
/// corpus and finds the toml at the workspace root.
#[test]
fn walks_up_to_find_marker_from_nested_cwd() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);
    // CWD = <tmp>/docs/adr/test/  (3 levels below marker)
    let nested = dir.path().join("docs/adr/test");
    let mut cmd = adr_fmt();
    cmd.current_dir(&nested).arg("--lint").assert().success();
}

/// Marker discovery from a sibling subdirectory still finds the
/// workspace marker (walk-up traverses through unrelated dirs).
#[test]
fn walks_up_from_crate_subdirectory() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);
    // Simulate a crate sibling: <tmp>/crates/foo/src/
    let crate_src = dir.path().join("crates/foo/src");
    fs::create_dir_all(&crate_src).expect("create crate src");
    let mut cmd = adr_fmt();
    cmd.current_dir(&crate_src).arg("--lint").assert().success();
}

/// Walk-up terminates at the filesystem root with no marker found.
/// Default mode prints the setup guide; explicit modes exit 1 with
/// the discovery error.
#[test]
fn walk_up_terminates_at_filesystem_root_default_mode() {
    let dir = TempDir::new().expect("create tempdir");
    // Empty tempdir, no toml anywhere on the way up.
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("QUICK START"));
}

#[test]
fn walk_up_terminates_at_filesystem_root_lint_mode() {
    let dir = TempDir::new().expect("create tempdir");
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path())
        .arg("--lint")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no adr-fmt.toml with a valid [corpus] table",
        ));
}

/// A toml with no `[corpus]` table is a parse failure (the
/// `[corpus]` field is required). Walk-up emits a `note: skipping`
/// trace AND, finding no parent marker, terminates with the
/// discovery error. Both must fire.
#[test]
fn corpus_table_missing_emits_clear_error() {
    let dir = TempDir::new().expect("create tempdir");
    fs::write(
        dir.path().join("adr-fmt.toml"),
        // No [corpus], no [[domains]] — required field missing.
        "# stray toml\n[stale]\ndirectory = \"x\"\n",
    )
    .expect("write stray toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path())
        .arg("--lint")
        .assert()
        .failure()
        // Both the per-marker skip note AND the terminal discovery
        // error must fire — they are distinct UX signals.
        .stderr(
            predicate::str::contains("skipping").and(predicate::str::contains(
                "no adr-fmt.toml with a valid [corpus] table",
            )),
        );
}

/// `[corpus] root` with an absolute path is rejected (containment).
#[test]
fn corpus_root_containment_rejects_absolute_path() {
    let dir = TempDir::new().expect("create tempdir");
    let absolute = if cfg!(windows) {
        "C:\\elsewhere"
    } else {
        "/tmp/elsewhere"
    };
    let toml = format!(
        "[corpus]\nroot = \"{absolute}\"\n\n[[domains]]\nprefix = \"TST\"\nname = \"x\"\ndirectory = \"test\"\ndescription = \"x\"\ncrates = []\n"
    );
    fs::write(dir.path().join("adr-fmt.toml"), toml).expect("write toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path()).arg("--lint").assert().failure();
}

/// `[corpus] root` with `..` traversal is rejected.
#[test]
fn corpus_root_containment_rejects_dot_dot() {
    let dir = TempDir::new().expect("create tempdir");
    fs::write(
        dir.path().join("adr-fmt.toml"),
        "[corpus]\nroot = \"../escape\"\n\n[[domains]]\nprefix = \"TST\"\nname = \"x\"\ndirectory = \"test\"\ndescription = \"x\"\ncrates = []\n",
    )
    .expect("write toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path()).arg("--lint").assert().failure();
}

/// Symlink in `[corpus] root` that escapes the marker tree is
/// rejected by containment canonicalization.
#[cfg(unix)]
#[test]
fn corpus_root_symlink_escape_rejected() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new().expect("create tempdir");
    let outside = TempDir::new().expect("create outside tempdir");
    fs::create_dir_all(outside.path().join("docs/adr/test")).expect("create outside corpus");
    // Create a symlink `escape -> <outside>` inside the marker dir.
    symlink(outside.path(), dir.path().join("escape")).expect("create symlink");
    fs::write(
        dir.path().join("adr-fmt.toml"),
        "[corpus]\nroot = \"escape/docs/adr\"\n\n[[domains]]\nprefix = \"TST\"\nname = \"x\"\ndirectory = \"test\"\ndescription = \"x\"\ncrates = []\n",
    )
    .expect("write toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path()).arg("--lint").assert().failure();
}

/// `[corpus] root` pointing at a regular file (not a dir) is rejected.
#[test]
fn corpus_root_is_regular_file_rejected() {
    let dir = TempDir::new().expect("create tempdir");
    fs::write(dir.path().join("not-a-dir"), "x").expect("write file");
    fs::write(
        dir.path().join("adr-fmt.toml"),
        "[corpus]\nroot = \"not-a-dir\"\n\n[[domains]]\nprefix = \"TST\"\nname = \"x\"\ndirectory = \"test\"\ndescription = \"x\"\ncrates = []\n",
    )
    .expect("write toml");
    // Marker is structurally valid but corpus root resolves to a file:
    // try_marker rejects it (not a dir) → discovery walks past → no
    // marker found at filesystem root → discovery error in --lint.
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path()).arg("--lint").assert().failure();
}

/// `[corpus] root` pointing at a non-existent directory: `try_marker`
/// rejects (`corpus_root.is_dir()` == false), walk-up continues, and
/// without a parent marker discovery fails.
#[test]
fn corpus_root_nonexistent_dir() {
    let dir = TempDir::new().expect("create tempdir");
    fs::write(
        dir.path().join("adr-fmt.toml"),
        "[corpus]\nroot = \"does/not/exist\"\n\n[[domains]]\nprefix = \"TST\"\nname = \"x\"\ndirectory = \"test\"\ndescription = \"x\"\ncrates = []\n",
    )
    .expect("write toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path()).arg("--lint").assert().failure();
}

/// Walk-up skips a malformed toml (parse error) with a `note:` and
/// continues to find a valid marker in a parent directory.
#[test]
fn walk_up_skips_malformed_toml_and_finds_parent_marker() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);
    // Place a malformed adr-fmt.toml deeper in the tree.
    let nested = dir.path().join("docs/adr/test");
    fs::write(nested.join("adr-fmt.toml"), "this is not valid toml ===")
        .expect("write malformed toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(&nested)
        .arg("--lint")
        .assert()
        .success() // parent marker found
        .stderr(predicate::str::contains("skipping"));
}

/// Walk-up skips a toml that parses but lacks `[corpus]` and finds
/// the valid parent marker.
#[test]
fn walk_up_skips_toml_without_corpus_and_finds_parent_marker() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);
    let nested = dir.path().join("docs/adr/test");
    fs::write(
        nested.join("adr-fmt.toml"),
        "[stale]\ndirectory = \"x\"\n", // valid toml, no [corpus]
    )
    .expect("write incomplete toml");
    let mut cmd = adr_fmt();
    cmd.current_dir(&nested).arg("--lint").assert().success();
}

/// The CLI no longer accepts a positional ADR-directory argument.
/// Passing one must produce a clap error (exit 2).
#[test]
fn cli_no_longer_accepts_adr_directory_arg() {
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);
    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path())
        .args(["--lint", dir.path().to_str().unwrap()])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("unexpected argument").or(predicate::str::contains("error:")),
        );
}

// ─── walk-up edge cases (rigormortis-driven) ─────────────────────────

/// **Pinned footgun.** A stray `adr-fmt.toml` deeper in the tree
/// whose corpus root resolves AND whose only domain has a violating
/// directory MASKS a valid parent marker. The masking is intentional:
/// claiming the marker surfaces the violation as an infrastructure
/// error rather than walking past it. This test pins the behavior so
/// future readers see the contract is deliberate.
#[test]
fn stray_marker_with_violating_domain_masks_parent() {
    // Set up a valid parent corpus.
    let dir = setup_corpus(MINIMAL_CONFIG, &[]);

    // Plant a stray marker inside the corpus tree whose corpus root
    // resolves (it points at the parent's docs/adr) but whose only
    // configured domain has a containment violation. The user clearly
    // intended this stray to be the marker, so we claim it and let
    // downstream surface the violation.
    let stray_dir = dir.path().join("docs/adr/test");
    let stray_toml = "[corpus]\nroot = \".\"\n\n[stale]\ndirectory = \"stale\"\n\n[[domains]]\nprefix = \"BAD\"\nname = \"Bad\"\ndirectory = \"../escape\"\ndescription = \"Violating domain.\"\ncrates = []\n";
    fs::write(stray_dir.join("adr-fmt.toml"), stray_toml).expect("write stray");

    let mut cmd = adr_fmt();
    cmd.current_dir(&stray_dir)
        .arg("--lint")
        .assert()
        .failure()
        // Containment error from the stray (NOT a generic "no marker"
        // error) — proves the stray was claimed and parent was masked.
        .stderr(predicate::str::contains("domain 'BAD'"));
}

/// CWD reached via a symlink (e.g. macOS `/var → /private/var`)
/// must walk up through the resolved path. This pins that the
/// canonicalization step in `discover_marker` is in place.
#[cfg(unix)]
#[test]
fn walk_up_canonicalizes_symlinked_cwd() {
    use std::os::unix::fs::symlink;

    let dir = setup_corpus(MINIMAL_CONFIG, &[]);

    // Create a symlink alias at a sibling path that points at the
    // tempdir. CWD = <symlink>/docs/adr/test/ — lexical walk-up
    // would traverse <symlink-parent>, NOT the resolved tempdir
    // ancestors. Canonical walk-up resolves <symlink> first.
    let alias_parent = TempDir::new().expect("create alias tempdir");
    let alias = alias_parent.path().join("workspace-alias");
    symlink(dir.path(), &alias).expect("create symlink alias");

    let nested = alias.join("docs/adr/test");
    let mut cmd = adr_fmt();
    cmd.current_dir(&nested).arg("--lint").assert().success();
}

/// When two valid markers exist in the chain (deeper + shallower),
/// the *nearer* (deeper) marker is selected.
#[test]
fn walk_up_selects_nearer_marker() {
    // Outer corpus with a unique-named ADR.
    let outer_adr = "\
# TST-0001. Outer ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0001

## Context

Outer corpus marker.

## Decision

R1 [5]: outer rule

## Consequences

Outer.
";
    let outer = setup_corpus(MINIMAL_CONFIG, &[("TST-0001-outer.md", outer_adr)]);

    // Inner corpus nested under outer/sub/ with its own marker AND
    // a uniquely-named ADR (TST-0002 not present in outer).
    let inner_root = outer.path().join("sub");
    fs::create_dir_all(inner_root.join("docs/adr/test")).expect("create inner corpus");
    fs::write(inner_root.join("adr-fmt.toml"), MINIMAL_CONFIG).expect("write inner toml");
    let inner_adr = "\
# TST-0002. Inner ADR

Date: 2026-04-27
Last-reviewed: 2026-04-27
Tier: B
Status: Accepted

## Related

Root: TST-0002
References: TST-9999

## Context

Inner corpus marker. References a deliberately dangling ADR so
the diagnostic uniquely identifies this file in lint output.

## Decision

R1 [5]: inner rule

## Consequences

Inner.
";
    fs::write(
        inner_root.join("docs/adr/test/TST-0002-inner.md"),
        inner_adr,
    )
    .expect("write inner ADR");

    // Run from inside the inner corpus. Discovery must stop at the
    // inner marker, not walk through it to the outer.
    let nested = inner_root.join("docs/adr/test");
    let mut cmd = adr_fmt();
    let output = cmd
        .current_dir(&nested)
        .arg("--lint")
        .output()
        .expect("binary runs");
    assert!(output.status.success(), "lint should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Pin: stderr must NOT contain `skipping` (inner claimed first).
    assert!(
        !stderr.contains("skipping"),
        "inner marker should be claimed immediately, no skips expected; stderr:\n{stderr}"
    );
    // Pin: the diagnostic for TST-9999 (a dangling reference unique
    // to the inner ADR) must fire AND its file path must include
    // the `sub/` segment proving the inner corpus was scanned.
    assert!(
        stdout.contains("TST-9999"),
        "inner ADR's L001 diagnostic missing — outer corpus may have won; stdout:\n{stdout}"
    );
    let sub_segment = if cfg!(windows) {
        "sub\\docs\\adr"
    } else {
        "sub/docs/adr"
    };
    assert!(
        stdout.contains(sub_segment),
        "diagnostic file path should reference the inner corpus ({sub_segment}); stdout:\n{stdout}"
    );
}

/// A marker whose corpus root resolves to an existing but EMPTY
/// directory (no domains exist on disk yet) is treated as a stray
/// — walk-up continues. In default mode with no parent marker the
/// setup guide is printed.
#[test]
fn marker_with_empty_corpus_falls_back_to_setup_guide() {
    let dir = TempDir::new().expect("create tempdir");
    // Marker exists, corpus root exists, but no domain dirs created.
    fs::create_dir_all(dir.path().join("docs/adr")).expect("create empty corpus");
    fs::write(dir.path().join("adr-fmt.toml"), MINIMAL_CONFIG).expect("write toml");

    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path())
        .assert()
        .success()
        // No domain → marker not claimed → walk-up continues → root
        // → setup guide.
        .stdout(predicate::str::contains("QUICK START"));
}

/// A marker file that exists but cannot be read (permission denied)
/// is a hard error during discovery — never silently skipped, since
/// the user clearly intended it as the marker.
#[cfg(unix)]
#[test]
fn unreadable_marker_aborts_discovery() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("create tempdir");
    let marker = dir.path().join("adr-fmt.toml");
    fs::write(&marker, MINIMAL_CONFIG).expect("write toml");
    // chmod 000 — no permissions for anyone.
    let mut perms = fs::metadata(&marker).expect("stat").permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&marker, perms).expect("chmod 000");

    // chmod 000 has no effect when running as root (CI-on-root,
    // container builds). Probe by attempting to read the marker
    // ourselves; if that succeeds, we're root and the test cannot
    // exercise the unreadable path. Skip rather than fail.
    if fs::read_to_string(&marker).is_ok() {
        let _ = fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644));
        eprintln!("skipping unreadable_marker_aborts_discovery: chmod 000 ineffective (root?)");
        return;
    }

    let mut cmd = adr_fmt();
    let result = cmd.current_dir(dir.path()).arg("--lint").output();

    // Best-effort restore so TempDir cleanup works even on assertion
    // failure.
    let _ = fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644));

    let output = result.expect("binary runs");
    assert!(
        !output.status.success(),
        "unreadable marker must fail discovery"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot read") || stderr.contains("Permission denied"),
        "stderr should describe the IO error; got:\n{stderr}"
    );
}

/// Same hard-error behavior when the unreadable marker is a *parent*
/// during walk-up (CWD has no marker, walk-up encounters one it
/// cannot read). Symmetric pin to the parse-skip-on-parent tests.
#[cfg(unix)]
#[test]
fn unreadable_parent_marker_aborts_discovery() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("create tempdir");
    let marker = dir.path().join("adr-fmt.toml");
    fs::write(&marker, MINIMAL_CONFIG).expect("write toml");
    // Create a sub directory to run from (no marker here).
    let nested = dir.path().join("subdir");
    fs::create_dir_all(&nested).expect("create subdir");

    let mut perms = fs::metadata(&marker).expect("stat").permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&marker, perms).expect("chmod 000");

    // Same root-skip probe as in unreadable_marker_aborts_discovery.
    if fs::read_to_string(&marker).is_ok() {
        let _ = fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644));
        eprintln!(
            "skipping unreadable_parent_marker_aborts_discovery: chmod 000 ineffective (root?)"
        );
        return;
    }

    let mut cmd = adr_fmt();
    let result = cmd.current_dir(&nested).arg("--lint").output();

    let _ = fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644));

    let output = result.expect("binary runs");
    assert!(
        !output.status.success(),
        "unreadable parent marker must fail discovery"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot read") || stderr.contains("Permission denied"),
        "stderr should describe the IO error; got:\n{stderr}"
    );
}

/// A marker file that is itself a *symlink* to a real config file
/// is accepted: `is_file()` follows symlinks, `read_to_string` follows
/// symlinks, and the `marker_dir` used for resolution is the symlink's
/// parent (not the target's parent). Pins the deliberate decision to
/// not invent a special case for symlinked markers.
#[cfg(unix)]
#[test]
fn symlinked_marker_file_is_accepted() {
    use std::os::unix::fs::symlink;

    // Real config lives in `real-config/adr-fmt.toml` plus a corpus
    // tree at `real-config/docs/adr/test/`. The marker symlink lives
    // at the workspace root and points at the real file.
    let dir = TempDir::new().expect("create tempdir");
    let real_dir = dir.path().join("real-config");
    fs::create_dir_all(real_dir.join("docs/adr/test")).expect("create real corpus");
    let real_marker = real_dir.join("adr-fmt.toml");
    fs::write(&real_marker, MINIMAL_CONFIG).expect("write real toml");

    // Workspace also needs its own corpus tree because resolution is
    // relative to the symlink's parent, not the target's parent.
    fs::create_dir_all(dir.path().join("docs/adr/test")).expect("create symlink-side corpus");
    symlink(&real_marker, dir.path().join("adr-fmt.toml")).expect("create symlink");

    let mut cmd = adr_fmt();
    cmd.current_dir(dir.path()).arg("--lint").assert().success();
}
