"""Local workflow parser regression: python3 -B tools/test_web_client_ci.py.

Uses a restricted stdlib workflow reader; does not launch GitHub Actions.
"""

import copy
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import Mock

from workflow_subset import load, read_workflow


ROOT = Path(__file__).resolve().parents[1]
INSTALL = "cargo install --version 0.2.129 --locked wasm-bindgen-cli"
WARM = [f"python3.12 -B tools/verify_web_client.py {mode} --ci"
        for mode in ("status", "native", "browser")]
COLD = [f"python3.12 -B tools/verify_web_client.py {mode} --ci"
        for mode in ("prepare", "wasm", "imports")]
ALL_OWNED = WARM + COLD + ["python3.12 -B tools/test_web_client_verify.py"]
# Every runner-owned frontend command the CI gate must carry exactly once.
# PREBUILDS is intentionally empty: the raw cargo precompile steps it used to
# name (host-lib build, wasm test --no-run, wasm release build, wasm-target
# clippy, wasm-tools import dump) are consolidated into single runner
# operations (prepare/wasm/imports) and must not reappear as raw cargo.
COMMANDS = [f"python3.12 -B tools/verify_web_client.py {mode} --ci"
            for mode in ("prepare", "status", "native", "browser", "wasm", "imports")]
PREBUILDS = []
# The frontend owner gate is a POSITIVE closed step segment, not a shell
# parser or a token-prefix matcher. Between the pinned wasm-bindgen/wasm-tools
# install step and the workspace clippy sentinel, every step's run must be
# exactly one of ALL_OWNED; any other executable step there (raw cargo in any
# spelling — reordered or environment-prefixed — an action step, a renamed
# interpreter) is unaccounted and rejected by construction. Workspace cargo
# and the later tripwire steps outside the segment stay legitimate.
WORKSPACE_CLIPPY_SENTINEL = "cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"


def is_install_step(step):
    return step.get("run") == INSTALL or (
        "install-action" in step.get("uses", "")
        and "wasm-bindgen-cli@0.2.129" in step.get("with", {}).get("tool", "")
    )


def assert_gates(test, workflow):
    job = workflow["jobs"]["build-test-lint"]
    test.assertEqual(job["runs-on"], "ubuntu-latest")
    test.assertNotIn("if", job)
    test.assertNotIn("continue-on-error", job)
    steps = job["steps"]
    test.assertFalse(any("verify_web_client.py bundle" in step.get("run", "") for step in steps))
    matches = [i for i, step in enumerate(steps)
               if step.get("with", {}).get("toolchain") == "1.99.0"]
    test.assertEqual(len(matches), 1, "unique 1.99 toolchain setup")
    setup = matches[0]
    setup_step = steps[setup]
    test.assertTrue(
        setup_step.get("uses", "").startswith("dtolnay/rust-toolchain@"),
        "1.99 setup uses the pinned dtolnay action")
    test.assertEqual(
        set(setup_step["with"]) - {"toolchain", "targets", "components"}, set(),
        "1.99 setup uses only toolchain/targets/components inputs")
    test.assertNotIn("default", setup_step["with"], "1.99 setup sets no default")
    test.assertEqual(setup_step["with"].get("components"), "clippy, rustfmt",
                     "1.99 setup carries clippy/rustfmt components")
    test.assertEqual(setup_step["with"].get("targets"), "wasm32-unknown-unknown",
                     "1.99 setup carries the wasm32 target")
    test.assertNotIn("if", setup_step, "1.99 setup is unconditional")
    test.assertNotIn("continue-on-error", setup_step, "1.99 setup is unconditional")
    canonical_install = next(
        i for i, step in enumerate(steps)
        if step.get("run", "").startswith("cargo +1.99.0 install --git https://github.com/acje/comment-free"))
    test.assertLess(setup, canonical_install,
                    "1.99 setup ordered before canonical comment-free install")
    matches = [i for i, step in enumerate(steps) if is_install_step(step)]
    test.assertEqual(len(matches), 1, INSTALL)
    install = matches[0]
    install_with = steps[install].get("with", {})
    test.assertEqual(install_with.get("fallback"), "none")
    test.assertEqual(install_with.get("checksum"), "true")
    test.assertLess(setup, install)
    test.assertNotIn("if", steps[install])
    test.assertNotIn("continue-on-error", steps[install])
    clippy = next(i for i, step in enumerate(steps)
                  if step.get("run") == WORKSPACE_CLIPPY_SENTINEL)
    segment = steps[install + 1:clippy]
    test.assertEqual(len(segment), len(ALL_OWNED),
                     "closed frontend segment holds exactly the runner-owned operation set")
    for command in ALL_OWNED:
        matches = [(i, step) for i, step in enumerate(segment) if step.get("run") == command]
        test.assertEqual(len(matches), 1, command)
        _, step = matches[0]
        test.assertNotIn("continue-on-error", step)
        test.assertNotIn("uses", step)
        if command == "python3.12 -B tools/verify_web_client.py wasm --ci":
            test.assertEqual(step.get("if"), "always()", "wasm lint+compile gate")
        elif command.endswith(" imports --ci"):
            test.assertEqual(step.get("if"), "always()", "import-key tripwire")
        else:
            test.assertNotIn("if", step)
    for step in segment:
        run = step.get("run", "")
        test.assertIn(run, ALL_OWNED,
                      f"unknown executable step inside the closed frontend segment: {run or '<inert step>'}")
        test.assertNotIn("uses", step, "no action step inside the closed frontend segment")
        test.assertNotIn("continue-on-error", step, "no ignore inside the closed frontend segment")
    cold = next(i for i, step in enumerate(segment)
                if step.get("run") == "python3.12 -B tools/verify_web_client.py prepare --ci")
    for mode in ("native", "browser"):
        warm = next(i for i, step in enumerate(segment)
                    if step.get("run") == f"python3.12 -B tools/verify_web_client.py {mode} --ci")
        test.assertLess(cold, warm, "cold prepare ordered before warm check")
    for run in ("python3.12 -B tools/verify_web_client.py bundle",
                "python3.12 -B tools/verify_web_client.py artifacts"):
        test.assertFalse(any(run in step.get("run", "") for step in steps),
                         "bundle/artifacts stays local (ghr-6fg0y)")


class WorkflowGates(unittest.TestCase):
    def test_workflow_read_failures(self):
        for error, code in ((FileNotFoundError("missing workflow"), 1),
                            (PermissionError("denied"), 2)):
            with self.subTest(error=type(error).__name__):
                path = Mock()
                path.read_text.side_effect = error
                with self.assertRaises(SystemExit) as caught:
                    read_workflow(path)
                self.assertEqual(caught.exception.code, code)

    def test_source_mutations_and_unsupported_shapes(self):
        source = (ROOT / ".github/workflows/ci-reusable.yml").read_text()
        for command in COMMANDS + PREBUILDS:
            changed = source.replace("run: " + command, "run: disabled", 1)
            self.assertNotEqual(changed, source)
            with self.assertRaises(AssertionError):
                assert_gates(self, load(changed))
            assert_gates(self, load(source))
        for old, new in (
            ("jobs:", "jobs: &jobs"),
            ("jobs:", "jobs: disabled"),
            ("    steps:", "    steps: *steps"),
            ("    runs-on: ubuntu-latest", "    runs-on: ubuntu-latest\n    runs-on: other"),
            ("    steps:", "    steps: []"),
            ("    steps:", "    steps: disabled"),
            ("run: |", "run: >"),
            ("    steps:", "    <<: *defaults\n    steps:"),
            ("    steps:", "   steps:"),
            ("    runs-on: ubuntu-latest", '    runs-on: "ubuntu-latest"'),
        ):
            changed = source.replace(old, new, 1)
            self.assertNotEqual(changed, source)
            result = subprocess.run(
                [sys.executable, "-S", "-B", str(ROOT / "tools/workflow_subset.py")],
                input=changed, text=True, capture_output=True, timeout=10)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn("UNAVAILABLE:", result.stderr)
            assert_gates(self, load(source))

    def test_missing_tomllib_subprocess_annotation(self):
        for mode in ("status", "browser"):
            result = subprocess.run(
                [sys.executable, "-S", "-B", "-c",
                 "import sys, runpy; sys.modules['tomllib'] = None; "
                 "sys.argv = ['tools/verify_web_client.py', " + repr(mode) + ", '--ci']; "
                 "runpy.run_path(sys.argv[0], run_name='__main__')"],
                cwd=ROOT, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertIn("UNAVAILABLE: Python 3.11+ required", result.stderr)
            self.assertIn("::warning::UNAVAILABLE:", result.stdout)
            self.assertNotIn("::error::", result.stdout)

    def test_workflow_gate_plant_fail_revert_clean(self):
        workflow = read_workflow(ROOT / ".github/workflows/ci-reusable.yml")
        assert_gates(self, workflow)
        for anchor in ("setup", "install"):
            for mutation in ("remove", "duplicate"):
                changed = copy.deepcopy(workflow)
                steps = changed["jobs"]["build-test-lint"]["steps"]
                matches = [step for step in steps
                           if (step.get("with", {}).get("toolchain") == "1.99.0"
                               if anchor == "setup" else is_install_step(step))]
                self.assertEqual(len(matches), 1, anchor)
                if mutation == "remove":
                    steps.remove(matches[0])
                else:
                    steps.append(copy.deepcopy(matches[0]))
                with self.assertRaises(AssertionError):
                    assert_gates(self, changed)
                assert_gates(self, workflow)
        for command in COMMANDS + PREBUILDS + ["python3.12 -B tools/test_web_client_verify.py"]:
            for mutation in ("remove", "ignore", "skip", "interpreter"):
                if mutation == "interpreter" and not command.startswith("python3.12"):
                    continue
                changed = copy.deepcopy(workflow)
                steps = changed["jobs"]["build-test-lint"]["steps"]
                step = next(step for step in steps if step.get("run") == command)
                if mutation == "remove":
                    steps.remove(step)
                elif mutation == "ignore":
                    step["continue-on-error"] = "true"
                elif mutation == "skip":
                    step["if"] = "false"
                else:
                    step["run"] = command.replace("python3.12", "python3")
                with self.assertRaises(AssertionError):
                    assert_gates(self, changed)
                assert_gates(self, workflow)
        cold_prepare = "python3.12 -B tools/verify_web_client.py prepare --ci"
        for mutation in ("reorder", "rawcargo"):
            changed = copy.deepcopy(workflow)
            steps = changed["jobs"]["build-test-lint"]["steps"]
            if mutation == "reorder":
                step = next(step for step in steps if step.get("run") == cold_prepare)
                steps.remove(step)
                steps.append(step)
            else:
                imports_index = next(i for i, step in enumerate(steps)
                                     if step.get("run") == "python3.12 -B tools/verify_web_client.py imports --ci")
                steps.insert(imports_index,
                             {"run": "cargo build -p gh-report-web-client --lib --locked"})
            with self.assertRaises(AssertionError):
                assert_gates(self, changed)
            assert_gates(self, workflow)
        changed = copy.deepcopy(workflow)
        changed["jobs"]["build-test-lint"]["steps"].append(
            {"run": "python3 -B tools/verify_web_client.py bundle"})
        with self.assertRaises(AssertionError):
            assert_gates(self, changed)
        assert_gates(self, workflow)
        for location in ("job", "install"):
            for field, value in (("if", "false"), ("continue-on-error", "true")):
                changed = copy.deepcopy(workflow)
                job = changed["jobs"]["build-test-lint"]
                target = job if location == "job" else next(
                    step for step in job["steps"] if is_install_step(step))
                target[field] = value
                with self.assertRaises(AssertionError):
                    assert_gates(self, changed)
                assert_gates(self, workflow)

    def test_toolchain_1_99_setup_guard_plant_fail_revert_clean(self):
        # The single 1.99 dtolnay/rust-toolchain setup is the canonical
        # availability source for the canonical comment-free install. The
        # guard pins its uniqueness, exact pin, relevant inputs, absence of
        # inappropriate inputs, unconditionality, and ordering before the
        # install. Each plant writes the REAL workflow file, asserts the guard
        # rejects it, restores byte-identically, and reasserts green.
        workflow_path = ROOT / ".github/workflows/ci-reusable.yml"
        setup_block = (
            "      - uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8 # stable\n"
            "        with:\n"
            "          toolchain: 1.99.0\n"
            "          components: clippy, rustfmt\n"
            "          targets: wasm32-unknown-unknown\n"
        )
        install_step = (
            "      - name: install canonical comment-free\n"
            "        run: cargo +1.99.0 install --git https://github.com/acje/comment-free --rev b4666626bbeee4e74ca41fd6ff1048b2f167dd27 --locked comment-free\n"
        )

        def plant(source, variant):
            if variant == "missing":
                return source.replace(setup_block, "")
            if variant == "wrongpin":
                return source.replace("          toolchain: 1.99.0\n",
                                      "          toolchain: 1.97.0\n", 1)
            if variant == "reorder":
                return source.replace(setup_block, "").replace(
                    install_step, install_step + setup_block, 1)
            return source.replace("          components: clippy, rustfmt\n",
                                  "          default: false\n          components: clippy, rustfmt\n", 1)

        original = workflow_path.read_text()
        assert_gates(self, read_workflow(workflow_path))
        try:
            for variant in ("missing", "wrongpin", "reorder", "defaultinput"):
                planted = plant(original, variant)
                self.assertNotEqual(planted, original, variant)
                workflow_path.write_text(planted)
                with self.assertRaises(AssertionError):
                    assert_gates(self, read_workflow(workflow_path))
                workflow_path.write_text(original)
                assert_gates(self, read_workflow(workflow_path))
        finally:
            workflow_path.write_text(original)
        self.assertEqual(workflow_path.read_bytes(), original.encode(),
                         "byte-identical restore")

    def test_supply_chain_install_steps_exact_pin(self):
        workflow_path = ROOT / ".github/workflows/ci-reusable.yml"

        def assert_supply_chain_gates(test):
            workflow = read_workflow(workflow_path)
            steps = workflow["jobs"]["supply-chain"]["steps"]
            audit = [s for s in steps
                     if s.get("with", {}).get("tool", "").startswith("cargo-audit@")]
            deny = [s for s in steps
                    if s.get("with", {}).get("tool", "").startswith("cargo-deny@")]
            test.assertEqual(len(audit), 1, "cargo-audit install step")
            test.assertEqual(len(deny), 1, "cargo-deny install step")
            for step in audit + deny:
                test.assertNotIn("if", step)
                test.assertNotIn("continue-on-error", step)
                test.assertEqual(step.get("with", {}).get("fallback"), "none")
                test.assertEqual(step.get("with", {}).get("checksum"), "true")
            test.assertEqual(audit[0]["with"]["tool"], "cargo-audit@0.22.2")
            test.assertEqual(deny[0]["with"]["tool"], "cargo-deny@0.20.2")

        source = workflow_path.read_text()
        assert_supply_chain_gates(self)
        try:
            for old, plant in (
                ("tool: cargo-audit@0.22.2", "tool: cargo-audit@0.22.1"),
                ("tool: cargo-audit@0.22.2", "tool: cargo-audit"),
                ("tool: cargo-deny@0.20.2", "tool: cargo-deny@0.19.4"),
            ):
                changed = source.replace(old, plant, 1)
                self.assertNotEqual(changed, source)
                workflow_path.write_text(changed)
                with self.assertRaises(AssertionError):
                    assert_supply_chain_gates(self)
                workflow_path.write_text(source)
        finally:
            workflow_path.write_text(source)
        assert_supply_chain_gates(self)


    def test_closed_frontend_segment_guard_plant_fail_revert_clean(self):
        # Real-file four-step ledger for the closed frontend segment: a step
        # planted INSIDE the segment whose run is not exactly one of ALL_OWNED
        # is rejected by construction (no shell parser, no token prefixes).
        # Covers the round-2 witnesses: pinned selectors, shell blocks, raw
        # cargo test, REORDERED flags and ENVIRONMENT-PREFIXED cargo all fail.
        workflow_path = ROOT / ".github/workflows/ci-reusable.yml"
        anchor = "        run: python3.12 -B tools/verify_web_client.py imports --ci\n"
        blocks = {
            "pinned": "      - name: raw cargo pinned-selector add\n"
                      "        run: cargo +1.99.0 build -p gh-report-web-client --lib --locked\n",
            "shell": "      - name: raw cargo shell-block add\n"
                     "        run: |\n"
                     "          set -euo pipefail\n"
                     "          cargo build -p gh-report-web-client --lib --locked\n",
            "tester": "      - name: raw cargo test add\n"
                      "        run: cargo test -p gh-report-web-client --target wasm32-unknown-unknown --locked --lib\n",
            "reordered": "      - name: raw cargo reordered flags add\n"
                         "        run: cargo +1.99.0 build --locked -p gh-report-web-client --lib\n",
            "envprefixed": "      - name: raw cargo env-prefixed add\n"
                           "        run: CARGO_TARGET_DIR=target cargo +1.99.0 build -p gh-report-web-client --lib --locked\n",
        }
        original = workflow_path.read_text()
        assert_gates(self, read_workflow(workflow_path))
        try:
            for variant, block in blocks.items():
                self.assertIn(anchor, original, variant)
                planted = original.replace(anchor, anchor + block, 1)
                self.assertNotEqual(planted, original, variant)
                workflow_path.write_text(planted)
                with self.assertRaises(AssertionError):
                    assert_gates(self, read_workflow(workflow_path))
                workflow_path.write_text(original)
                assert_gates(self, read_workflow(workflow_path))
        finally:
            workflow_path.write_text(original)
        self.assertEqual(workflow_path.read_bytes(), original.encode(),
                         "byte-identical restore")

if __name__ == "__main__":
    read_workflow(ROOT / ".github/workflows/ci-reusable.yml")
    unittest.main()
