"""Run as: python3.12 -B tools/test_web_client_verify.py.

Builds the current host rlib (120s timeout), then compiles stdin probes with
the pinned rustc (30s each). Exact E0061/message checking lives here, not in
rustdoc's compile_fail annotation. Temporary metadata is removed on exit.
The regression suite also runs in CI.
"""

import contextlib
import io
import json
import runpy
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True

from verify_web_client import NAMES, ROOT, compare_bundles, status_contract
import verify_web_client as verifier


class VerificationGuards(unittest.TestCase):
    def test_ci_unavailable_warns_but_remains_blocking(self):
        code, output, error = self.invoke("browser", which=lambda tool: None, ci=True)
        self.assertEqual(code, 2)
        self.assertIn("::warning::UNAVAILABLE", output)
        self.assertIn("not a demonstrated defect", output)
        self.assertIn("UNAVAILABLE", error)
        self.assertEqual(self.invoke("status", ci=True)[0], 0)
        code, output, _ = self.invoke("status", ci=True, read_text=lambda path, text: text.replace(
            'DotState::Pass => "pass"', 'DotState::Pass => "typo"'))
        self.assertEqual(code, 1)
        self.assertIn("::error::FAILED", output)
        def timeout(*args, **kwargs):
            raise subprocess.TimeoutExpired(args[0], 180)
        code, output, error = self.invoke("browser", child=timeout, which=lambda tool: tool, ci=True)
        self.assertEqual(code, 2)
        self.assertIn("::warning::UNAVAILABLE", output)
        self.assertNotIn("::error::FAILED", output)
        self.assertIn("UNAVAILABLE", error)
        self.assertEqual(self.invoke("status", ci=True)[0], 0)

    def test_browser_is_headless_and_missing_driver_is_nonzero(self):
        def child(args, **kwargs):
            if args[-1] == "--version":
                return subprocess.CompletedProcess(args, 0, stdout="wasm-bindgen-test-runner 0.2.126\n")
            self.assertEqual(kwargs["env"]["HEADLESS"], "1")
            self.assertNotIn("NO_HEADLESS", kwargs["env"])
            self.assertEqual(kwargs["env"]["CHROMEDRIVER"], "/usr/bin/chromedriver")
            return subprocess.CompletedProcess(args, 0)
        with patch.dict(verifier.os.environ, {"NO_HEADLESS": "1"}):
            self.assertEqual(self.invoke("browser", child=child, which=lambda tool: "/usr/bin/" + tool)[0], 0)
        code, _, error = self.invoke("browser", child=child,
                                     which=lambda tool: None if tool == "chromedriver" else tool)
        self.assertEqual(code, 2)
        self.assertIn("no browser tests executed", error)

    def test_bundle_cli_synthetic_baseline_plant_fail_revert_clean(self):
        original = Path.read_bytes
        def child(args, **kwargs):
            if args[-1] == "--version":
                return subprocess.CompletedProcess(args, 0, stdout="wasm-bindgen 0.2.126\n")
            if "--out-dir" in args:
                output = Path(args[args.index("--out-dir") + 1])
                for name in NAMES:
                    (output / name).write_bytes(name.encode())
            return subprocess.CompletedProcess(args, 0)
        def invoke(stale=None):
            def read(path):
                if path.parent == ROOT / "crates/gh-report/templates" and path.name in NAMES:
                    return path.name.encode() + (b"stale" if path.name == stale else b"")
                return original(path)
            with patch.object(Path, "read_bytes", read):
                return self.invoke("bundle", child=child, which=lambda tool: tool)
        self.assertEqual(invoke()[0], 0)
        for name in NAMES:
            code, _, error = invoke(name)
            self.assertEqual(code, 1, error)
            self.assertIn("MISMATCH: " + name, error)
            self.assertEqual(invoke()[0], 0)

    def test_compiler_cli_schema_plant_fail_revert_clean(self):
        diagnostic = {"$message_type": "diagnostic", "level": "error", "code": {"code": "E0061"},
                      "message": "this function takes 4 arguments but 3 arguments were supplied",
                      "spans": [{"label": "argument #4 of type `SortDirection` is missing"}]}
        def invoke_with(stderr, positive_stderr="", positive_code=0):
            calls = []
            def child(args, **kwargs):
                calls.append(args)
                if args[0] == "cargo":
                    artifact = {"reason": "compiler-artifact", "target": {"name": "gh_report_web_client"},
                                "filenames": [str(ROOT / "target/debug/libgh_report_web_client.rlib")]}
                    return subprocess.CompletedProcess(args, 0, stdout=json.dumps(artifact))
                positive = "SortDirection::Ascending" in kwargs["input"]
                return subprocess.CompletedProcess(args, positive_code if positive else 1,
                                                   stderr=positive_stderr if positive else stderr)
            result = self.invoke("compiler", child=child)
            self.assertTrue(calls, "compiler CLI must reach compiler subprocesses")
            return result
        clean = json.dumps(diagnostic)
        self.assertEqual(invoke_with(clean)[0], 0)
        for code in (0, 1):
            self.assertEqual(invoke_with(clean, positive_stderr="schema drift", positive_code=code)[0], 2)
            self.assertEqual(invoke_with(clean)[0], 0)
        for drift in ("not JSON", "[]", "null", "{}", json.dumps({**diagnostic, "spans": None}),
                      json.dumps({**diagnostic, "code": "E0061"}),
                      json.dumps({**diagnostic, "message": 3}),
                      json.dumps({**diagnostic, "spans": [None]})):
            with self.subTest(drift=drift):
                code, _, error = invoke_with(drift)
                self.assertEqual(code, 2, error)
                self.assertIn("UNAVAILABLE", error)
                self.assertEqual(invoke_with(clean)[0], 0)
        mismatch = json.dumps({**diagnostic, "code": {"code": "E0432"}})
        code, _, error = invoke_with(mismatch)
        self.assertEqual(code, 1)
        self.assertIn("EXPECTED_MISSING_DIRECTION", error)
        self.assertEqual(invoke_with(clean)[0], 0)

    def test_compiler_cli_process_success(self):
        done = subprocess.run([sys.executable, "-B", str(ROOT / "tools/verify_web_client.py"), "compiler"],
                              capture_output=True, text=True, timeout=180)
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: compiler", done.stdout)

    def test_compiled_direction_contract_plant_fail_revert_clean(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        built = subprocess.run(
            ["cargo", f"+{channel}", "build", "-p", "gh-report-web-client", "--lib", "--locked", "--message-format=json"],
            cwd=ROOT, capture_output=True, text=True, timeout=120, check=True)
        artifacts = [json.loads(line) for line in built.stdout.splitlines()]
        rlibs = [Path(name) for artifact in artifacts
                 if artifact.get("reason") == "compiler-artifact"
                 and artifact["target"]["name"] == "gh_report_web_client"
                 for name in artifact["filenames"] if name.endswith(".rlib")]
        self.assertEqual(len(rlibs), 1)
        imports = "use gh_report_web_client::sort::{SortType, SortDirection, compare_cells_directed};"
        missing = imports + '\nfn main() { let _ = compare_cells_directed("unknown", "pass", SortType::Status); }'
        positive = missing.replace("SortType::Status);", "SortType::Status, SortDirection::Ascending);")
        with tempfile.TemporaryDirectory(prefix="direction-proof-", dir=ROOT / "target") as directory:
            def compile_source(source):
                return subprocess.run(
                    ["rustc", f"+{channel}", "--edition=2024", "--crate-name=direction_probe",
                     "--error-format=json", "--emit=metadata", "--extern", f"gh_report_web_client={rlibs[0]}",
                     "-L", f"dependency={rlibs[0].parent}", "-o", str(Path(directory) / "probe.rmeta"), "-"],
                    input=source, cwd=ROOT, capture_output=True, text=True, timeout=30)
            good = compile_source(positive)
            self.assertEqual(good.returncode, 0, good.stderr)
            with self.assertRaisesRegex(ValueError, "EXPECTED_MISSING_DIRECTION"):
                verifier.require_missing_direction(good)
            bad = compile_source(missing)
            verifier.require_missing_direction(bad)
            wrong_import = compile_source(missing.replace("::sort::", "::missing_sort::"))
            self.assertNotEqual(wrong_import.returncode, 0)
            self.assertIn('"E0432"', wrong_import.stderr)
            with self.assertRaisesRegex(ValueError, "EXPECTED_MISSING_DIRECTION"):
                verifier.require_missing_direction(wrong_import)
            verifier.require_missing_direction(compile_source(missing))
            self.assertEqual(compile_source(positive).returncode, 0)
            print(f"compiler proof: positive={good.returncode}; missing-direction={bad.returncode} E0061; wrong-import={wrong_import.returncode} E0432 rejected; restored clean")

    def invoke(self, mode, read_text=None, child=None, which=None, ci=False):
        stdout, stderr = io.StringIO(), io.StringIO()
        original_read = Path.read_text
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(sys, "argv", ["verify_web_client.py", mode] + (["--ci"] if ci else [])))
            stack.enter_context(contextlib.redirect_stdout(stdout))
            stack.enter_context(contextlib.redirect_stderr(stderr))
            if read_text:
                stack.enter_context(patch.object(Path, "read_text", lambda path: read_text(path, original_read(path))))
            if child:
                stack.enter_context(patch("subprocess.run", side_effect=child))
            if which:
                stack.enter_context(patch("shutil.which", side_effect=which))
            with self.assertRaises(SystemExit) as exit:
                runpy.run_path(str(ROOT / "tools/verify_web_client.py"), run_name="__main__")
        return exit.exception.code, stdout.getvalue(), stderr.getvalue()

    def test_main_status_exit_contract_plant_fail_revert_clean(self):
        self.assertEqual(self.invoke("status")[0], 0)
        code, _, error = self.invoke("status", read_text=lambda path, text: text.replace('DotState::Pass => "pass"', 'DotState::Pass => "typo"'))
        self.assertEqual(code, 1)
        self.assertIn("INVALID_STATUS_KEYS", error)
        for marker in ("pub fn sort_key(&self)", "fn known_status(value:"):
            with self.subTest(marker=marker):
                code, _, error = self.invoke("status", read_text=lambda path, text: text.replace(marker, "missing_marker"))
                self.assertEqual(code, 2)
                self.assertIn("UNAVAILABLE: UNPARSEABLE_SOURCE", error)
                self.assertNotIn("INVALID_STATUS_KEYS", error)
        self.assertEqual(self.invoke("status")[0], 0)

    def test_main_timeout_is_unavailable(self):
        version = next(p["version"] for p in verifier.tomllib.loads((ROOT / "Cargo.lock").read_text())["package"] if p["name"] == "wasm-bindgen")
        def timeout(*args, **kwargs):
            self.assertEqual(kwargs["timeout"], 180)
            if args[0][-1] == "--version":
                return subprocess.CompletedProcess(args[0], 0, stdout=f"{args[0][0]} {version}\n")
            raise subprocess.TimeoutExpired(args[0], 180)
        for mode in ("bundle", "browser"):
            with self.subTest(mode=mode):
                code, _, error = self.invoke(mode, child=timeout, which=lambda tool: tool)
                self.assertEqual(code, 2)
                self.assertIn("UNAVAILABLE", error)
                self.assertNotIn("FAILED", error)

    def test_main_version_mismatch_is_failure(self):
        for mode in ("bundle", "browser"):
            with self.subTest(mode=mode):
                code, _, error = self.invoke(mode, child=lambda *args, **kwargs: subprocess.CompletedProcess(args[0], 0, stdout="wrong version"), which=lambda tool: tool)
                self.assertEqual(code, 1)
                self.assertIn("must match locked version", error)

    def test_status_cli_process_success(self):
        done = subprocess.run([sys.executable, "-B", str(ROOT / "tools/verify_web_client.py"), "status"], capture_output=True, text=True, timeout=10)
        self.assertEqual(done.returncode, 0)
        self.assertIn("VERIFIED", done.stdout)

    def test_unparseable_source_boundaries_plant_revert_clean(self):
        for source in ("missing", "start body", "start body end start"):
            with self.subTest(source=source):
                with self.assertRaisesRegex(verifier.UnparseableSource, "UNPARSEABLE_SOURCE"):
                    verifier.source_body(source, "start", "end")
                self.assertEqual(verifier.source_body("start body end", "start", "end"), " body ")

    def test_main_missing_tool_is_unavailable(self):
        for mode in ("bundle", "browser"):
            with self.subTest(mode=mode):
                code, _, error = self.invoke(mode, which=lambda tool: None)
                self.assertEqual(code, 2)
                self.assertIn("UNAVAILABLE", error)

    def test_stale_bundle_plant_fail_revert_clean(self):
        with tempfile.TemporaryDirectory(dir=ROOT / "target") as directory:
            generated, committed = Path(directory) / "generated", Path(directory) / "committed"
            generated.mkdir()
            committed.mkdir()
            for name in NAMES:
                data = (ROOT / "crates/gh-report/templates" / name).read_bytes()
                (generated / name).write_bytes(data)
                (committed / name).write_bytes(data)
            compare_bundles(generated, committed)
            for name in NAMES:
                with self.subTest(name=name):
                    original = (committed / name).read_bytes()
                    (committed / name).write_bytes(original + b"stale")
                    with self.assertRaisesRegex(ValueError, "MISMATCH"):
                        compare_bundles(generated, committed)
                    (committed / name).write_bytes(original)
                    compare_bundles(generated, committed)

    def test_invalid_status_key_plant_fail_revert_clean(self):
        producer = (ROOT / "crates/gh-report/src/report/view_model.rs").read_text()
        consumer = (ROOT / "crates/gh-report-web-client/src/sort.rs").read_text()
        status_contract(producer, consumer)
        with self.assertRaisesRegex(ValueError, "INVALID_STATUS_KEYS"):
            status_contract(producer.replace('DotState::Pass => "pass"', 'DotState::Pass => "typo"'), consumer)
        with self.assertRaisesRegex(ValueError, "INVALID_STATUS_KEYS"):
            status_contract(producer, consumer.replace('"pass" => Some(KnownStatus::Pass)', '"typo" => Some(KnownStatus::Pass)'))
        status_contract(producer, consumer)


if __name__ == "__main__":
    unittest.main()
