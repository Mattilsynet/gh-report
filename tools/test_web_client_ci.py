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
INSTALL = "cargo install --version 0.2.128 --locked wasm-bindgen-cli"
COMMANDS = [f"python3.12 -B tools/verify_web_client.py {mode} --ci"
            for mode in ("status", "compiler", "browser")]
PREBUILDS = [
    "cargo build -p gh-report-web-client --lib --locked",
    "cargo test -p gh-report-web-client --target wasm32-unknown-unknown --locked --lib --no-run",
]


def assert_gates(test, workflow):
    job = workflow["jobs"]["build-test-lint"]
    test.assertEqual(job["runs-on"], "ubuntu-latest")
    test.assertNotIn("if", job)
    test.assertNotIn("continue-on-error", job)
    steps = job["steps"]
    test.assertFalse(any("verify_web_client.py bundle" in step.get("run", "") for step in steps))
    matches = [i for i, step in enumerate(steps)
               if step.get("with", {}).get("toolchain") == "1.98.0"]
    test.assertEqual(len(matches), 1, "toolchain setup")
    setup = matches[0]
    matches = [i for i, step in enumerate(steps) if step.get("run") == INSTALL]
    test.assertEqual(len(matches), 1, INSTALL)
    install = matches[0]
    test.assertLess(setup, install)
    test.assertNotIn("if", steps[install])
    test.assertNotIn("continue-on-error", steps[install])
    for command in COMMANDS + PREBUILDS + ["python3.12 -B tools/test_web_client_verify.py"]:
        matches = [(i, step) for i, step in enumerate(steps) if step.get("run") == command]
        test.assertEqual(len(matches), 1, command)
        index, step = matches[0]
        test.assertLess(install, index)
        test.assertNotIn("continue-on-error", step)
        test.assertNotIn("if", step)
    for build, mode in zip(PREBUILDS, ("compiler", "browser")):
        before = next(i for i, step in enumerate(steps) if step.get("run") == build)
        after = next(i for i, step in enumerate(steps)
                     if step.get("run") == f"python3.12 -B tools/verify_web_client.py {mode} --ci")
        test.assertLess(before, after)
        test.assertEqual(steps[before].get("env", {}).get("CARGO_TARGET_DIR"), "target")


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
                           if (step.get("with", {}).get("toolchain") == "1.98.0"
                               if anchor == "setup" else step.get("run") == INSTALL)]
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
        for build in PREBUILDS:
            changed = copy.deepcopy(workflow)
            steps = changed["jobs"]["build-test-lint"]["steps"]
            step = next(step for step in steps if step.get("run") == build)
            steps.remove(step)
            steps.append(step)
            with self.assertRaises(AssertionError):
                assert_gates(self, changed)
            assert_gates(self, workflow)
            changed = copy.deepcopy(workflow)
            step = next(step for step in changed["jobs"]["build-test-lint"]["steps"]
                        if step.get("run") == build)
            step["env"]["CARGO_TARGET_DIR"] = "other-target"
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
                    step for step in job["steps"] if step.get("run") == INSTALL)
                target[field] = value
                with self.assertRaises(AssertionError):
                    assert_gates(self, changed)
                assert_gates(self, workflow)


if __name__ == "__main__":
    read_workflow(ROOT / ".github/workflows/ci-reusable.yml")
    unittest.main()
