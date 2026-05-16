//! The 12 criteria. Each runner is a free fn that takes `&Context` and
//! returns a `CriterionResult`. Side-table `CRITERIA` is iterated by main.
//!
//! Criteria 10 and 11 are aliases that mirror #4's verdict (per mission
//! brief: audit-trail and replay-equivalence-alias are subsumed by the
//! `smi_replay_equivalence` integration test).

use std::path::Path;
use std::sync::Mutex;

use crate::runner::{count_rg_matches, find_substring, run};
use crate::{Context, Criterion, CriterionResult, Verdict};

/// Cache of #4's verdict so #10 and #11 can mirror it without re-running.
static C4_VERDICT: Mutex<Option<Verdict>> = Mutex::new(None);

fn record_c4(v: Verdict) {
    *C4_VERDICT.lock().unwrap() = Some(v);
}

fn mirror_c4() -> Verdict {
    C4_VERDICT.lock().unwrap().unwrap_or(Verdict::Fail)
}

pub const CRITERIA: &[Criterion] = &[
    Criterion {
        num: "1",
        short_name: "smi-rename-gate",
        runner: c1_smi_rename_gate,
    },
    Criterion {
        num: "2",
        short_name: "eventstore-confinement",
        runner: c2_eventstore_confinement,
    },
    Criterion {
        num: "3",
        short_name: "gh-report-workspace-tests",
        runner: c3_gh_report_tests,
    },
    Criterion {
        num: "4",
        short_name: "smi-replay-equivalence",
        runner: c4_smi_replay_equivalence,
    },
    Criterion {
        num: "5",
        short_name: "loc-gate-server-rs",
        runner: c5_loc_gate,
    },
    Criterion {
        num: "6a",
        short_name: "workspace-build",
        runner: c6a_workspace_build,
    },
    Criterion {
        num: "6b",
        short_name: "workspace-test-all-features",
        runner: c6b_workspace_test_all_features,
    },
    Criterion {
        num: "7",
        short_name: "workspace-clippy",
        runner: c7_clippy,
    },
    Criterion {
        num: "8",
        short_name: "cargo-fmt-check",
        runner: c8_fmt,
    },
    Criterion {
        num: "9",
        short_name: "adr-fmt-lint",
        runner: c9_adr_fmt_lint,
    },
    Criterion {
        num: "10",
        short_name: "audit-trail",
        runner: c10_audit_trail,
    },
    Criterion {
        num: "11",
        short_name: "replay-equivalence-alias",
        runner: c11_replay_alias,
    },
    Criterion {
        num: "12",
        short_name: "doc-reconciliation",
        runner: c12_doc_reconciliation,
    },
];

// ---- criterion 1: SMI rename gate -----------------------------------------

fn c1_smi_rename_gate(ctx: &Context) -> CriterionResult {
    // rg exits 1 on no-match, 0 on match. We want zero matches → PASS.
    let (out, dur) = run(
        &ctx.workspace_root,
        "rg",
        &[
            "-n",
            "sequence_tracker|run_index|repo_index|delivery_index",
            "crates/gh-report/src/",
        ],
    );
    let matches = count_rg_matches(&out.stdout);
    let (verdict, note) = if matches == 0 {
        (Verdict::Pass, "zero matches".to_string())
    } else {
        (
            Verdict::Fail,
            format!("{matches} match(es) of forbidden identifiers"),
        )
    };
    CriterionResult {
        verdict,
        metric: matches.to_string(),
        note,
        duration_ms: dur,
    }
}

// ---- criterion 2: EventStore confinement ----------------------------------

fn c2_eventstore_confinement(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(
        &ctx.workspace_root,
        "rg",
        &["-n", "EventStore", "crates/gh-report/src/"],
    );
    let matches = count_rg_matches(&out.stdout);
    let (verdict, note) = if matches <= ctx.eventstore_ceiling {
        (
            Verdict::Pass,
            format!("below ceiling {}", ctx.eventstore_ceiling),
        )
    } else {
        (
            Verdict::Fail,
            format!(
                "{matches} matches > ceiling {} (raise via --eventstore-ceiling N if legitimate)",
                ctx.eventstore_ceiling
            ),
        )
    };
    CriterionResult {
        verdict,
        metric: matches.to_string(),
        note,
        duration_ms: dur,
    }
}

// ---- criterion 3: gh-report workspace tests -------------------------------

fn c3_gh_report_tests(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(
        &ctx.workspace_root,
        "cargo",
        &["test", "-p", "gh-report", "--workspace"],
    );
    verdict_from_exit(&out, dur, "-")
}

// ---- criterion 4: SMI replay equivalence ----------------------------------

fn c4_smi_replay_equivalence(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(
        &ctx.workspace_root,
        "cargo",
        &[
            "test",
            "-p",
            "gh-report",
            "--test",
            "smi_replay_equivalence",
        ],
    );
    let result = verdict_from_exit(&out, dur, "-");
    record_c4(result.verdict);
    result
}

// ---- criterion 5: LOC gate ------------------------------------------------
// Amended FOCUS.md v0.6 (2026-05-16): production-LOC non-regression gate.
// `prod-loc` (syn AST walker) counts top-level item spans outside
// `#[cfg(test)]` modules + `tests/` dirs. Threshold 1007 = Phase-2-v2
// baseline measured at HEAD f634de9 pre-Track-4.3. Doctrine is
// non-regression going forward, not a tightening floor.
const TRACK4_LOC_BASELINE: usize = 1007;

fn c5_loc_gate(ctx: &Context) -> CriterionResult {
    let start = std::time::Instant::now();
    let (out, _) = run(
        &ctx.workspace_root,
        "cargo",
        &[
            "run",
            "--quiet",
            "--manifest-path",
            "scripts/prod-loc/Cargo.toml",
            "--",
            "crates/gh-report/src/infra/server/server.rs",
        ],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // prod-loc emits tab-separated records; first line is `PROD-LOC\t<n>`.
    let n: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("PROD-LOC\t").and_then(|v| v.parse().ok()))
        .unwrap_or_else(|| {
            panic!(
                "prod-loc did not emit `PROD-LOC\\t<n>` line; stdout=`{stdout}`. \
                 track4-verify assumes scripts/prod-loc is buildable."
            )
        });
    let dur = start.elapsed().as_millis();
    let (verdict, note) = if n <= TRACK4_LOC_BASELINE {
        (
            Verdict::Pass,
            format!("PROD-LOC {n} <= baseline {TRACK4_LOC_BASELINE}"),
        )
    } else {
        (
            Verdict::Fail,
            format!("PROD-LOC {n} > baseline {TRACK4_LOC_BASELINE}"),
        )
    };
    CriterionResult {
        verdict,
        metric: n.to_string(),
        note,
        duration_ms: dur,
    }
}

// ---- criterion 6a: workspace build ----------------------------------------

fn c6a_workspace_build(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(&ctx.workspace_root, "cargo", &["build", "--workspace"]);
    verdict_from_exit(&out, dur, "-")
}

// ---- criterion 6b: workspace test all-features ----------------------------

fn c6b_workspace_test_all_features(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(
        &ctx.workspace_root,
        "cargo",
        &["test", "--workspace", "--all-features"],
    );
    verdict_from_exit(&out, dur, "-")
}

// ---- criterion 7: clippy --------------------------------------------------

fn c7_clippy(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(
        &ctx.workspace_root,
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    );
    verdict_from_exit(&out, dur, "-")
}

// ---- criterion 8: fmt check -----------------------------------------------

fn c8_fmt(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(&ctx.workspace_root, "cargo", &["fmt", "--check"]);
    verdict_from_exit(&out, dur, "-")
}

// ---- criterion 9: adr-fmt --lint -----------------------------------------

fn c9_adr_fmt_lint(ctx: &Context) -> CriterionResult {
    let (out, dur) = run(
        &ctx.workspace_root,
        "cargo",
        &["run", "-p", "adr-fmt", "--", "--lint"],
    );
    verdict_from_exit(&out, dur, "warnings-only")
}

// ---- criterion 10: audit-trail (alias of #4) ------------------------------

fn c10_audit_trail(_ctx: &Context) -> CriterionResult {
    let v = mirror_c4();
    CriterionResult {
        verdict: v,
        metric: "mirror-#4".to_string(),
        note: "covered by smi-replay-equivalence (#4)".to_string(),
        duration_ms: 0,
    }
}

// ---- criterion 11: replay-equivalence alias of #4 -------------------------

fn c11_replay_alias(_ctx: &Context) -> CriterionResult {
    let v = mirror_c4();
    CriterionResult {
        verdict: v,
        metric: "mirror-#4".to_string(),
        note: "alias of #4".to_string(),
        duration_ms: 0,
    }
}

// ---- criterion 12: doc reconciliation -------------------------------------

fn c12_doc_reconciliation(ctx: &Context) -> CriterionResult {
    let start = std::time::Instant::now();
    let focus = ctx.workspace_root.join("FOCUS.md");
    let roadmap = ctx.workspace_root.join("docs/c4/roadmap.md");
    let needle = "Track 4";

    let focus_hit = find_substring(&focus, needle);
    let roadmap_hit = find_substring(&roadmap, needle);
    let dur = start.elapsed().as_millis();

    let (focus_ok, focus_note) = match focus_hit {
        Ok(Some(line)) => (true, format!("FOCUS.md:{line}")),
        Ok(None) => (false, "FOCUS.md: no 'Track 4' hit".to_string()),
        Err(e) => (false, format!("FOCUS.md: {e}")),
    };
    let (roadmap_ok, roadmap_note) = match roadmap_hit {
        Ok(Some(line)) => (true, format!("roadmap.md:{line}")),
        Ok(None) => (false, "roadmap.md: no 'Track 4' hit".to_string()),
        Err(e) => (false, format!("roadmap.md: {e}")),
    };

    let both_ok = focus_ok && roadmap_ok;
    let verdict = if both_ok {
        Verdict::Pass
    } else if ctx.strict_docs {
        Verdict::Fail
    } else {
        Verdict::Manual
    };

    let metric = "heuristic".to_string();
    let note = if both_ok {
        format!("{focus_note}; {roadmap_note}")
    } else {
        format!(
            "{focus_note}; {roadmap_note}{}",
            if ctx.strict_docs {
                ""
            } else {
                " (MANUAL; use --strict-docs to fail)"
            }
        )
    };
    CriterionResult {
        verdict,
        metric,
        note,
        duration_ms: dur,
    }
}

// ---- shared helper --------------------------------------------------------

fn verdict_from_exit(
    out: &std::process::Output,
    duration_ms: u128,
    pass_note: &str,
) -> CriterionResult {
    let code = out.status.code().unwrap_or(-1);
    let (verdict, note) = if out.status.success() {
        (Verdict::Pass, pass_note.to_string())
    } else {
        let tail = tail_stderr(&out.stderr, 200);
        (Verdict::Fail, format!("exit {code}: {tail}"))
    };
    CriterionResult {
        verdict,
        metric: code.to_string(),
        note,
        duration_ms,
    }
}

fn tail_stderr(stderr: &[u8], max: usize) -> String {
    let s = String::from_utf8_lossy(stderr);
    let trimmed = s.trim();
    if trimmed.len() <= max {
        trimmed.replace('\t', " ").replace('\n', " | ")
    } else {
        let start = trimmed.len() - max;
        format!("...{}", &trimmed[start..])
            .replace('\t', " ")
            .replace('\n', " | ")
    }
}

// Touch Path to avoid unused-import lint when tests are off.
#[allow(dead_code)]
fn _force_use(_: &Path) {}
