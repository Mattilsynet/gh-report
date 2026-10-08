#!/usr/bin/env python3
"""Local BOUNDARY ownership regression: python3 -B tools/test_verify.py.

Records and asserts the exact expanded command ledger executed by
scripts/verify.sh and the single-reference BOUNDARY block in AGENTS.md.

Two evidence classes, clearly separated:

1. EXECUTION evidence (the load-bearing proofs): the actual runner is invoked
   on the actual checkout with `cargo` and `timeout` stubbed through PATH.
   The stubs record full argv + cwd per invocation; tools/tripwires.sh is
   transiently planted with an executable recording stub (save->plant->run->
   restore in finally, original bytes+mode demonstrated restored). These tests
   prove exact argv/order/root normalization from a different invocation cwd,
   each gate's distinct nonzero failure with no downstream execution and no
   success banner, timeout 124 raw propagation with no success, and Cargo
   no-fail-fast (via atest alias) as independent of the caller's sequential
   stop-on-first-failure. No real cargo build/test/clippy/fmt and no service
   is launched.

2. STATIC evidence (clearly NOT execution proof): AGENTS.md BOUNDARY fence is
   a single `sh scripts/verify.sh` reference with no expanded workspace
   duplicate and no repeated executable spelling in prose; hard obligations
   (exit-124 Surprise, --no-fail-fast, doctests) stay in doctrine; verify.sh
   stays strict (set -eu, banner last, no masking); sh -n syntax.

Correction note (2026-10-08): an earlier 12/12 static-only pass claimed the
ledger but did NOT prove execution. A reviewer demonstrated a false-clean by
early-exiting the runner in memory while all static tests passed. This suite
therefore derives the ledger from actual stub-recording runs, and the static
checks are labeled non-execution.

Prerequisite (documented): ~/.cargo/config.toml must define the canonical
aliases (atest = test --quiet --no-fail-fast; aclippy = clippy --quiet
--message-format=short). Missing file or alias is an actionable failure, not a
skip. Real alias dispatch is additionally probed with help-only `cargo atest
--help` / `cargo aclippy --help` (no compilation or runtime coverage).

Mission-scratch discipline: every temporary directory created by this suite
lives under .ooda/tmp/boundary-owner-20261008/ (the mission's own scratch,
gitignored) and only directories this run created are removed on cleanup —
never unrelated scratch.
"""

import configparser
import contextlib
import os
import re
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
VERIFY = ROOT / "scripts" / "verify.sh"
AGENTS = ROOT / "AGENTS.md"
CONFIG = Path.home() / ".cargo" / "config.toml"
TRIPWIRES = ROOT / "tools" / "tripwires.sh"
MISSION_TMP = ROOT / ".ooda" / "tmp" / "boundary-owner-20261008"


def _mission_tmp():
    # Mission-local scratch only; create the parent safely, never anything else.
    MISSION_TMP.mkdir(parents=True, exist_ok=True)
    return str(MISSION_TMP)

EXPECTED_SUCCESS = [
    ("tripwires", ("all",)),
    ("cargo", ("build", "--workspace", "--all-features", "--locked")),
    ("timeout", ("900", "cargo", "atest", "--workspace", "--all-features", "--locked")),
    ("cargo", ("atest", "--workspace", "--all-features", "--locked")),
    ("cargo", ("aclippy", "--workspace", "--all-targets", "--all-features", "--locked", "--", "-D", "warnings")),
    ("cargo", ("fmt", "--all", "--", "--check")),
]

# Distinct nonzero per gate + timeout 124; each stops the caller (set -eu) with
# no downstream stage and no success banner.
FAILURE_SCENARIOS = {
    "tripwires": dict(tripwires_exit=7, expected_exit=7, expected_rows=1),
    "build": dict(fail_stage="build", fail_code=3, expected_exit=3, expected_rows=2),
    "test": dict(fail_stage="atest", fail_code=2, expected_exit=2, expected_rows=4),
    "timeout": dict(timeout_exit=124, expected_exit=124, expected_rows=3),
    "clippy": dict(fail_stage="aclippy", fail_code=9, expected_exit=9, expected_rows=5),
    "fmt": dict(fail_stage="fmt", fail_code=4, expected_exit=4, expected_rows=6),
}

WORKSPACE_SPELLINGS = (
    "cargo test --workspace",
    "cargo clippy --workspace",
    "cargo build --workspace",
    "cargo fmt --all",
    "tools/tripwires.sh all",
)

_CARGO_STUB = """#!/bin/sh
{
  printf 'cargo'
  for a in "$@"; do
    printf '\\t%s' "$a"
  done
  printf '\\t%s\\n' "$(pwd)"
} >> "$STUB_LOG"
code=0
if [ -n "${STUB_FAIL_STAGE:-}" ] && [ "$1" = "$STUB_FAIL_STAGE" ]; then
  [ -n "${STUB_FAIL_CODE:-}" ] && code=$STUB_FAIL_CODE
fi
exit "$code"
"""

_TIMEOUT_STUB = """#!/bin/sh
{
  printf 'timeout'
  for a in "$@"; do
    printf '\\t%s' "$a"
  done
  printf '\\t%s\\n' "$(pwd)"
} >> "$STUB_LOG"
if [ -n "${STUB_TIMEOUT_EXIT:-}" ]; then
  exit "$STUB_TIMEOUT_EXIT"
fi
shift
exec "$@"
"""

_TRIPWIRES_STUB = """#!/bin/sh
{
  printf 'tripwires'
  for a in "$@"; do
    printf '\\t%s' "$a"
  done
  printf '\\t%s\\n' "$(pwd)"
} >> "$STUB_LOG"
code=0
[ -n "${STUB_TRIPWIRES_EXIT:-}" ] && code=$STUB_TRIPWIRES_EXIT
exit "$code"
"""


def _write_stub(path, content, mode=None):
    path.write_text(content, encoding="utf-8")
    os.chmod(path, mode if mode is not None else 0o755)


@contextlib.contextmanager
def runner_fixture(**scenario):
    """Create stubs, plant the tripwires recording stub, yield controls.

    Restores tools/tripwires.sh bytes+mode in all paths; the caller asserts
    restoration after the with-block.
    """
    scratch = tempfile.mkdtemp(prefix="verify-run-", dir=_mission_tmp())
    stub_dir = Path(scratch) / "bin"
    stub_dir.mkdir()
    log = Path(scratch) / "log.tsv"
    env_extra = {"STUB_LOG": str(log)}
    _write_stub(stub_dir / "cargo", _CARGO_STUB)
    _write_stub(stub_dir / "timeout", _TIMEOUT_STUB)
    original = TRIPWIRES.read_bytes()
    orig_mode = stat.S_IMODE(TRIPWIRES.stat().st_mode)
    try:
        for key in ("fail_stage", "fail_code", "timeout_exit", "tripwires_exit"):
            value = scenario.get(key)
            if value is not None:
                env_extra["STUB_" + key.upper()] = str(value)
        _write_stub(TRIPWIRES, _TRIPWIRES_STUB, mode=orig_mode | stat.S_IXUSR)
        yield {
            "log": log,
            "stub_dir": stub_dir,
            "env": env_extra,
            "original_tripwires": original,
            "orig_mode": orig_mode,
        }
    finally:
        TRIPWIRES.write_bytes(original)
        os.chmod(TRIPWIRES, orig_mode)
        shutil.rmtree(scratch, ignore_errors=True)


def run_verify(fx, invocation_cwd):
    env = dict(os.environ)
    env["PATH"] = str(fx["stub_dir"]) + os.pathsep + env.get("PATH", "")
    env.update(fx["env"])
    return subprocess.run(
        ["/bin/sh", str(VERIFY)],
        cwd=str(invocation_cwd),
        env=env,
        capture_output=True,
        text=True,
    )


def records(log_path):
    if not log_path.exists():
        # No gate ever ran (e.g. the runner early-exited): represent this as an
        # explicitly empty execution trace so assertions fail cleanly instead of
        # raising a missing-file error that masks "nothing executed".
        return []
    rows = []
    for line in log_path.read_text(encoding="utf-8").splitlines():
        parts = line.split("\t")
        rows.append((parts[0], tuple(parts[1:-1]), parts[-1]))
    return rows


def cargo_alias(name):
    cfg = configparser.RawConfigParser()
    with CONFIG.open(encoding="utf-8") as fh:
        cfg.read_file(fh)
    return cfg.get("alias", name, raw=True)


def strip_fences(text):
    lines = []
    in_fence = False
    for line in text.splitlines():
        if line.strip().startswith("```"):
            in_fence = not in_fence
            continue
        if not in_fence:
            lines.append(line)
    return "\n".join(lines)


def fenced_sh_blocks(text):
    blocks = []
    current = []
    in_fence = False
    for line in text.splitlines():
        if line.strip().startswith("```"):
            if in_fence:
                blocks.append(current)
                current = []
            in_fence = not in_fence
            continue
        if in_fence:
            current.append(line)
    return blocks


class AgtsBoundaryReferenceTest(unittest.TestCase):
    """Static AGENTS.md doctrine checks (non-execution)."""

    def test_boundary_fence_single_reference(self):
        text = AGENTS.read_text(encoding="utf-8")
        blocks = [
            block
            for block in fenced_sh_blocks(text)
            if any("scripts/verify.sh" in ln for ln in block)
        ]
        self.assertEqual(len(blocks), 1, "exactly one AGENTS code fence names scripts/verify.sh")
        self.assertEqual([ln.strip() for ln in blocks[0] if ln.strip()], ["sh scripts/verify.sh"])

    def test_no_expanded_workspace_duplicate_in_boundary(self):
        text = AGENTS.read_text(encoding="utf-8")
        block = next(
            block
            for block in fenced_sh_blocks(text)
            if any("scripts/verify.sh" in ln for ln in block)
        )
        joined = "\n".join(block)
        for needle in WORKSPACE_SPELLINGS:
            self.assertNotIn(needle, joined)

    def test_no_repeated_executable_spelling_outside_fences(self):
        text = strip_fences(AGENTS.read_text(encoding="utf-8"))
        for needle in WORKSPACE_SPELLINGS:
            self.assertNotIn(
                needle,
                text,
                "executable command spelling must live in the script, not AGENTS prose",
            )

    def test_local_cadence_heading_is_honest(self):
        text = AGENTS.read_text(encoding="utf-8")
        heading = next(ln for ln in text.splitlines() if ln.strip().startswith("### Build / test / verify"))
        self.assertIn("local", heading.lower())
        self.assertNotIn("mirror", heading.lower())

    def test_boundary_prose_keeps_hard_obligations(self):
        text = AGENTS.read_text(encoding="utf-8")
        self.assertIn("Exit 124 is `Outcome::Surprise`, NEVER a test failure.", text)
        self.assertIn("--no-fail-fast", text)
        self.assertIn("doctests", text.lower())


class VerifyShStructureTest(unittest.TestCase):
    """Static script-shape guards (non-execution)."""

    def test_script_syntax_valid(self):
        proc = subprocess.run(
            ["sh", "-n", str(VERIFY)], cwd=ROOT, capture_output=True, text=True
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_strict_stop_no_mask_no_false_success(self):
        lines = VERIFY.read_text(encoding="utf-8").splitlines()
        joined = "\n".join(lines)
        self.assertTrue(
            re.search(r"^set\s+(-e|\-eu)", joined, re.MULTILINE),
            "script must be strict (set -e) so a failing stage stops the run",
        )
        self.assertNotIn("set +e", joined)
        self.assertNotIn("|| true", joined)
        self.assertNotIn("|| :", joined)
        self.assertNotRegex(joined, r"^if\s+!", re.MULTILINE)
        nonblank = [ln for ln in lines if ln.strip()]
        self.assertIn("checks passed.", nonblank[-1], "success banner must be the last line")


class RunnerExecutionTest(unittest.TestCase):
    """EXECUTION evidence: actual checkout, stubbed cargo/timeout/tripwires."""

    def test_exact_ledger_argv_order_and_root_normalization(self):
        with tempfile.TemporaryDirectory(prefix="verify-invoked-", dir=_mission_tmp()) as invocation:
            with runner_fixture() as fx:
                self.assertNotEqual(Path(invocation).resolve(), ROOT)
                proc = run_verify(fx, invocation)
                rows = records(fx["log"])
                original = fx["original_tripwires"]
                orig_mode = fx["orig_mode"]
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            self.assertIn("checks passed.", proc.stdout)
            self.assertEqual([(name, args) for name, args, _cwd in rows], EXPECTED_SUCCESS)
            self.assertTrue(
                all(cwd == str(ROOT) for _name, _args, cwd in rows),
                "each staged step must run with cwd == repo ROOT",
            )
            self.assertEqual(TRIPWIRES.read_bytes(), original)
            self.assertEqual(stat.S_IMODE(TRIPWIRES.stat().st_mode), orig_mode, "mode restored")

    def test_each_gate_failure_distinct_no_downstream_no_banner(self):
        with tempfile.TemporaryDirectory(prefix="verify-invoked-", dir=_mission_tmp()) as invocation:
            for name, scenario in FAILURE_SCENARIOS.items():
                with self.subTest(gate=name):
                    with runner_fixture(**scenario) as fx:
                        proc = run_verify(fx, invocation)
                        rows = records(fx["log"])
                        original = fx["original_tripwires"]
                        orig_mode = fx["orig_mode"]
                    self.assertEqual(proc.returncode, scenario["expected_exit"], name)
                    self.assertNotIn("checks passed.", proc.stdout)
                    self.assertEqual(len(rows), scenario["expected_rows"], name)
                    self.assertEqual(
                        [(n, a) for n, a, _c in rows],
                        EXPECTED_SUCCESS[: scenario["expected_rows"]],
                        name,
                    )
                    self.assertEqual(TRIPWIRES.read_bytes(), original)
                    self.assertEqual(stat.S_IMODE(TRIPWIRES.stat().st_mode), orig_mode)

    def test_no_fail_fast_in_cargo_via_alias_is_independent_of_caller_stop(self):
        # Cargo no-fail-fast rides the atest alias; the caller still stops
        # sequentially across stages (proven by test_each_gate_failure...).
        alias = cargo_alias("atest")
        self.assertIn("--no-fail-fast", alias)
        with tempfile.TemporaryDirectory(prefix="verify-invoked-", dir=_mission_tmp()) as invocation:
            with runner_fixture() as fx:
                proc = run_verify(fx, invocation)
                rows = records(fx["log"])
            self.assertEqual(proc.returncode, 0)
            self.assertIn(
                ("cargo", ("atest", "--workspace", "--all-features", "--locked")),
                [(n, a) for n, a, _c in rows],
            )


class CargoAliasCompatibilityTest(unittest.TestCase):
    """Real alias presence + help-only dispatch compatibility (no compile)."""

    def test_pinned_aliases_present_with_actionable_errors(self):
        self.assertTrue(CONFIG.exists(), "missing ~/.cargo/config.toml (canonical aliases required)")
        atest = cargo_alias("atest")
        aclippy = cargo_alias("aclippy")
        self.assertIn("--no-fail-fast", atest)
        self.assertIn("--quiet", atest)
        self.assertIn("--quiet", aclippy)
        self.assertIn("--message-format=short", aclippy)

    @unittest.skipUnless(shutil.which("cargo"), "cargo not on PATH")
    def test_real_alias_dispatch_help(self):
        for alias in ("atest", "aclippy"):
            with self.subTest(alias=alias):
                proc = subprocess.run(
                    ["cargo", alias, "--help"], cwd=str(ROOT), capture_output=True, text=True
                )
                self.assertEqual(proc.returncode, 0, proc.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
