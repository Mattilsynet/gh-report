"""Run as: python3.12 -B tools/test_web_client_verify.py.

Source-only: no test dispatches real cargo, rustc, wasm-bindgen, chromedriver
or a browser. The compiler-contract coverage (host rlib discovery, positive
and missing-direction probes, E0061/missing-direction and E0432/wrong-import
rejection, unparseable-schema boundaries) is exercised through executable
recording stubs on an explicit PATH (see _stub_tools): the stubs record
argv/env/cwd/order and render the requested fixture output and exit, so the
real CLI dispatch and parsing logic is what is verified. Mocked subprocess
paths assert timeout/exit behavior directly. The regression suite also runs
in CI.
"""

import contextlib
import hashlib
import io
import json
import os
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
                return subprocess.CompletedProcess(args, 0, stdout="wasm-bindgen-test-runner 0.2.129\n")
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
                return subprocess.CompletedProcess(args, 0, stdout="wasm-bindgen 0.2.129\n")
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
                if "::missing_sort::" in kwargs["input"]:
                    wrong = {"$message_type": "diagnostic", "level": "error",
                             "code": {"code": "E0432"},
                             "message": "cannot find sort in gh_report_web_client", "spans": []}
                    return subprocess.CompletedProcess(args, 1, stderr=json.dumps(wrong))
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

    E0061 = {"$message_type": "diagnostic", "level": "error", "code": {"code": "E0061"},
             "message": "this function takes 4 arguments but 3 arguments were supplied",
             "spans": [{"label": "argument #4 of type `SortDirection` is missing"}]}

    def _stub_tools(self, tmpdir, tools, plan=None):
        """Write executable recording stubs for the named tools.

        The stubs never invoke real cargo/rustc/wasm-bindgen/wasm-tools/browser:
        each call appends one JSON record (prog, argv, cwd, env subset, stdin
        markers) to WD_RECORD and renders the fixture output/exit described by
        the plan file (WD_STUB_PLAN). The real runner is executed as a
        subprocess with the stub dir prepended to PATH. The shebang is this
        interpreter's absolute path, so stub execution never depends on PATH
        resolution; callers assert every tool stub exists before running, so a
        missing stub cannot silently fall back to a real binary.
        """
        stub_dir = Path(tmpdir) / "stubs"
        stub_dir.mkdir()
        record_path = Path(tmpdir) / "calls.jsonl"
        plan_path = Path(tmpdir) / "plan.json"
        plan_path.write_text(json.dumps(plan or {}))
        stub = f"""#!{sys.executable}
import hashlib
import json, os, sys
from pathlib import Path
prog = os.path.basename(sys.argv[0])
plan = json.loads(open(os.environ["WD_STUB_PLAN"]).read())
src = sys.stdin.read()
rec = {{"prog": prog, "argv": sys.argv[1:], "cwd": os.getcwd(),
       "stdin_sha256": hashlib.sha256(src.encode()).hexdigest(),
       "env": {{k: os.environ.get(k) for k in ("PATH", "CARGO_TARGET_DIR", "CARGO_TERM_PROGRESS_WHEN",
                                               "CHROMEDRIVER", "WASM_BINDGEN_USE_BROWSER", "HEADLESS",
                                               "NO_HEADLESS", "WASM_BINDGEN_TEST_TIMEOUT",
                                               "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER")}}}}
if src:
    rec["stdin_head"] = src[:160]
    rec["stdin_has_ascending"] = "SortDirection::Ascending" in src
    rec["stdin_has_missing_sort"] = "::missing_sort::" in src
with open(os.environ["WD_RECORD"], "a") as f:
    f.write(json.dumps(rec) + "\\n")
cfg = plan.get(prog, {{}})
if prog == "wasm-tools" and len(sys.argv) > 1 and sys.argv[1] == "validate":
    if cfg.get("validate_fail") or cfg.get("validate_rc"):
        sys.stderr.write(cfg.get("validate_stderr", "invalid wasm text\\n"))
        sys.exit(cfg.get("validate_rc", 1))
    sys.exit(0)
if cfg.get("write_bundle") and "--out-dir" in sys.argv:
    out = Path(sys.argv[sys.argv.index("--out-dir") + 1])
    tmpl = Path(plan["_bundle_templates"])
    for name in ("gh-report-web-client.js", "gh-report-web-client_bg.wasm"):
        (out / name).write_bytes((tmpl / name).read_bytes())
if cfg.get("force_stderr_garbage"):
    sys.stderr.write(cfg.get("stderr", "garbage\\n"))
    sys.exit(cfg.get("exit", 0))
if prog == "rustc":
    if "::missing_sort::" in src:
        if cfg.get("direction_wrong"):
            code, msg = "E0061", "this function takes 4 arguments but 3 arguments were supplied"
        elif cfg.get("e0432_sparse"):
            code, msg = "E0432", "unresolved import `gh_report_web_client::missing_sort`"
        else:
            code, msg = "E0432", "cannot find sort in gh_report_web_client"
    elif "SortDirection::Ascending" in src and cfg.get("fault") != "positive_e0432":
        sys.exit(0)
    elif cfg.get("fault") == "e0432_missing":
        code, msg = "E0432", "cannot find sort in gh_report_web_client"
    else:
        code, msg = "E0061", "this function takes 4 arguments but 3 arguments were supplied"
    label = ("could not find `missing_sort` in `gh_report_web_client`"
             if cfg.get("e0432_sparse") and "::missing_sort::" in src
             else "argument #4 of type `SortDirection` is missing")
    sys.stderr.write(json.dumps({{"$message_type": "diagnostic", "level": "error",
                                 "code": {{"code": code}}, "message": msg,
                                 "spans": [{{"label": label}}]}}) + "\\n")
    sys.exit(1)
artifact = {{"reason": "compiler-artifact", "target": {{"name": "gh_report_web_client"}},
            "filenames": ["/stub-target/libgh_report_web_client.rlib"]}}
stdout = cfg.get("stdout", json.dumps(artifact) + "\\n")
sys.stdout.write(stdout if isinstance(stdout, str) else json.dumps(stdout) + "\\n")
if cfg.get("stderr"):
    sys.stderr.write(cfg["stderr"])
sys.exit(cfg.get("exit", 0))
"""
        for name in tools:
            (stub_dir / name).write_text(stub)
            (stub_dir / name).chmod(0o755)
        return stub_dir, record_path, plan_path

    def _run_stubbed(self, mode, tools, plan=None):
        env = os.environ.copy()
        with tempfile.TemporaryDirectory(prefix="web-stub-") as tmpdir:
            stub_dir, record_path, plan_path = self._stub_tools(tmpdir, tools, plan)
            for name in tools:
                self.assertTrue((stub_dir / name).exists(),
                                f"missing stub {name} (PATH would fall back to a real binary)")
            env["PATH"] = str(stub_dir) + os.pathsep + env.get("PATH", "")
            env["WD_RECORD"] = str(record_path)
            env["WD_STUB_PLAN"] = str(plan_path)
            done = subprocess.run(
                [sys.executable, "-B", str(ROOT / "tools/verify_web_client.py"), mode],
                cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
            calls = [json.loads(line) for line in record_path.read_text().splitlines()]
        return done, calls

    def test_compiler_cli_recording_stub_process_success(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        done, calls = self._run_stubbed("compiler", ("cargo", "rustc"))
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: compiler", done.stdout)
        self.assertEqual([c["prog"] for c in calls], ["cargo", "rustc", "rustc", "rustc"])
        cargo_argv, cargo_cwd = calls[0]["argv"], Path(calls[0]["cwd"])
        self.assertEqual(cargo_argv[0], f"+{channel}")
        self.assertEqual(cargo_argv[1:], ["build", "-p", "gh-report-web-client", "--lib",
                                          "--locked", "--message-format=json"])
        self.assertEqual(cargo_cwd, ROOT)
        self.assertEqual(calls[0]["env"]["CARGO_TARGET_DIR"], str(ROOT / "target"))
        self.assertEqual(calls[0]["env"]["CARGO_TERM_PROGRESS_WHEN"], "never")
        for idx, rustc_call in enumerate(calls[1:]):
            argv, cwd = rustc_call["argv"], Path(rustc_call["cwd"])
            self.assertEqual(argv[0], f"+{channel}")
            self.assertEqual(argv[1], "--edition=2024")
            self.assertIn("--error-format=json", argv)
            self.assertIn("--emit=metadata", argv)
            self.assertIn("--extern", argv)
            self.assertIn("gh_report_web_client=/stub-target/libgh_report_web_client.rlib", argv)
            self.assertIn("-L", argv)
            self.assertEqual(cwd, ROOT)
            self.assertEqual(rustc_call["env"]["CARGO_TARGET_DIR"], str(ROOT / "target"))
            self.assertEqual(rustc_call["env"]["CARGO_TERM_PROGRESS_WHEN"], "never")
        self.assertIs(calls[1]["stdin_has_ascending"], True)
        self.assertIs(calls[2]["stdin_has_ascending"], False)
        self.assertIs(calls[2]["stdin_has_missing_sort"], False)
        self.assertIs(calls[3]["stdin_has_missing_sort"], True)

    def test_compiler_cli_recording_stub_plant_fail_revert_clean(self):
        done, _ = self._run_stubbed("compiler", ("cargo", "rustc"))
        self.assertEqual(done.returncode, 0, done.stderr)
        for plant, expect, marker in (
            ({"rustc": {"force_stderr_garbage": True, "stderr": "schema drift\n"}}, 2, "UNAVAILABLE"),
            ({"rustc": {"fault": "positive_e0432"}}, 1, "POSITIVE_COMPILER_PROBE"),
            ({"rustc": {"fault": "e0432_missing"}}, 1, "EXPECTED_MISSING_DIRECTION"),
            ({"cargo": {"stdout": "not json\n"}}, 2, "UNPARSEABLE_ARTIFACTS"),
        ):
            with self.subTest(plant=plant):
                done, _ = self._run_stubbed("compiler", ("cargo", "rustc"), plan=plant)
                self.assertEqual(done.returncode, expect, done.stderr)
                self.assertIn(marker, done.stderr)
                done, _ = self._run_stubbed("compiler", ("cargo", "rustc"))
                self.assertEqual(done.returncode, 0, done.stderr)

    def test_compiler_recording_stub_wrong_import_sparse_e0432_verifies(self):
        done, calls = self._run_stubbed(
            "compiler", ("cargo", "rustc"), {"rustc": {"e0432_sparse": True}})
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: compiler", done.stdout)
        self.assertEqual([c["prog"] for c in calls], ["cargo", "rustc", "rustc", "rustc"])
        wrong = next(c for c in calls if c.get("stdin_has_missing_sort"))
        self.assertIs(wrong["stdin_has_missing_sort"], True)

    def test_compiler_recording_stub_direction_wrong_e0061_rejected(self):
        done, _ = self._run_stubbed(
            "compiler", ("cargo", "rustc"), {"rustc": {"direction_wrong": True}})
        self.assertEqual(done.returncode, 1, done.stdout)
        self.assertIn("WRONG_IMPORT_PROBE", done.stderr)

    def test_prepare_recording_stub_matrix(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        done, calls = self._run_stubbed("prepare", ("cargo",))
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: prepare", done.stdout)
        self.assertEqual([c["prog"] for c in calls], ["cargo", "cargo"])
        build, tests = calls
        self.assertEqual(build["argv"], [f"+{channel}", "build", "-p", "gh-report-web-client",
                                         "--lib", "--locked"])
        self.assertEqual(tests["argv"], [f"+{channel}", "test", "-p", "gh-report-web-client",
                                         "--target", "wasm32-unknown-unknown", "--locked", "--lib",
                                         "--no-run"])
        for call in calls:
            self.assertEqual(Path(call["cwd"]), ROOT)
            self.assertEqual(call["env"]["CARGO_TARGET_DIR"], str(ROOT / "target"))
            self.assertEqual(call["env"]["CARGO_TERM_PROGRESS_WHEN"], "never")

    def test_wasm_recording_stub_matrix(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        done, calls = self._run_stubbed("wasm", ("cargo",))
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: wasm", done.stdout)
        self.assertEqual([c["prog"] for c in calls], ["cargo", "cargo"])
        build, clippy = calls
        self.assertEqual(build["argv"], [f"+{channel}", "build", "-p", "gh-report-web-client",
                                         "--target", "wasm32-unknown-unknown", "--release", "--locked"])
        self.assertEqual(clippy["argv"], [f"+{channel}", "clippy", "-p", "gh-report-web-client",
                                          "--target", "wasm32-unknown-unknown", "--all-targets",
                                          "--locked", "--", "-D", "warnings"])
        for call in calls:
            self.assertEqual(Path(call["cwd"]), ROOT)
            self.assertEqual(call["env"]["CARGO_TARGET_DIR"], str(ROOT / "target"))

    def test_imports_recording_stub_matrix(self):
        dump = '(module (import "./gh-report-web-client_bg.js" (func)))\n'
        done, calls = self._run_stubbed(
            "imports", ("wasm-tools",), {"wasm-tools": {"stdout": dump}})
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: imports", done.stdout)
        self.assertEqual([c["prog"] for c in calls], ["wasm-tools", "wasm-tools"])
        print_call, validate_call = calls
        self.assertEqual(Path(print_call["cwd"]), ROOT)
        self.assertEqual(print_call["argv"][0], "print")
        self.assertEqual(print_call["argv"][1],
                         str(ROOT / "crates/gh-report/templates/gh-report-web-client_bg.wasm"))
        self.assertEqual(validate_call["argv"], ["validate"])
        self.assertEqual(Path(validate_call["cwd"]), ROOT)
        self.assertEqual(validate_call["stdin_sha256"], hashlib.sha256(dump.encode()).hexdigest(),
                         "wasm-tools validate consumes the EXACT captured print text")

    def test_imports_recording_stub_fault_matrix(self):
        dump = '(module (import "./gh-report-web-client_bg.js" (func)))\n'
        valid = {"wasm-tools": {"stdout": dump}}
        done, _ = self._run_stubbed("imports", ("wasm-tools",), valid)
        self.assertEqual(done.returncode, 0, done.stderr)
        for plant, expect, marker in (
            ({"wasm-tools": {"stdout": 'garbage (module (import "./gh-report-web-client_bg.js" (func 0))',
                             "validate_fail": True}}, 2, "UNPARSEABLE_IMPORTS"),
            ({"wasm-tools": {"stdout": dump, "validate_fail": True}}, 2, "UNPARSEABLE_IMPORTS"),
            ({"wasm-tools": {"stdout": '(module (import "./bad.js" (func)))\n'}}, 1, "wasm import modules"),
        ):
            with self.subTest(plant=plant):
                done, _ = self._run_stubbed("imports", ("wasm-tools",), plant)
                self.assertEqual(done.returncode, expect, done.stderr)
                self.assertIn(marker, done.stderr)
                done, _ = self._run_stubbed("imports", ("wasm-tools",), valid)
                self.assertEqual(done.returncode, 0, done.stderr)

    def test_imports_recording_stub_empty_import_detected(self):
        expected_only = '(module\n  (type (;0;) (func))\n  (import "./gh-report-web-client_bg.js" "f" (func (;0;) (type 0)))\n)\n'
        empty_extra = '(module\n  (type (;0;) (func))\n  (import "./gh-report-web-client_bg.js" "f" (func (;0;) (type 0)))\n  (import "" "g" (func (;1;) (type 0)))\n)\n'
        done, _ = self._run_stubbed("imports", ("wasm-tools",), {"wasm-tools": {"stdout": empty_extra}})
        self.assertEqual(done.returncode, 1, done.stderr)
        self.assertIn("wasm import modules", done.stderr)
        done, _ = self._run_stubbed("imports", ("wasm-tools",), {"wasm-tools": {"stdout": expected_only}})
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: imports", done.stdout)

    def test_browser_recording_stub_matrix(self):
        version = next(p["version"] for p in verifier.tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
                       if p["name"] == "wasm-bindgen")
        runner = "wasm-bindgen-test-runner"
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        with tempfile.TemporaryDirectory(prefix="web-stub-") as tmpdir:
            stub_dir, record_path, plan_path = self._stub_tools(
                tmpdir, (runner, "chromedriver", "cargo"),
                {runner: {"stdout": f"{runner} {version}\n"}})
            env = os.environ.copy()
            env["PATH"] = str(stub_dir) + os.pathsep + env.get("PATH", "")
            env["WD_RECORD"] = str(record_path)
            env["WD_STUB_PLAN"] = str(plan_path)
            env["NO_HEADLESS"] = "1"
            done = subprocess.run(
                [sys.executable, "-B", str(ROOT / "tools/verify_web_client.py"), "browser"],
                cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
            calls = [json.loads(line) for line in record_path.read_text().splitlines()]
        self.assertEqual(done.returncode, 0, done.stderr)
        self.assertIn("VERIFIED: browser", done.stdout)
        self.assertEqual([c["prog"] for c in calls], [runner, "cargo"])
        self.assertEqual(calls[0]["argv"], ["--version"])
        self.assertEqual(Path(calls[0]["cwd"]), ROOT)
        self.assertEqual(calls[0]["env"]["CARGO_TARGET_DIR"],
                         os.environ.get("CARGO_TARGET_DIR"),
                         "--version inherits ambient env, not bound_env")
        self.assertEqual(calls[1]["argv"], [f"+{channel}", "test", "-p", "gh-report-web-client",
                                            "--target", "wasm32-unknown-unknown", "--locked", "--lib"])
        self.assertEqual(Path(calls[1]["cwd"]), ROOT, "browser cargo test runs at ROOT")
        browser_env = calls[1]["env"]
        self.assertEqual(browser_env["HEADLESS"], "1")
        self.assertEqual(browser_env["CHROMEDRIVER"], str(stub_dir / "chromedriver"))
        self.assertEqual(browser_env["WASM_BINDGEN_USE_BROWSER"], "1")
        self.assertEqual(browser_env["CARGO_TARGET_DIR"], str(ROOT / "target"))
        self.assertEqual(browser_env["WASM_BINDGEN_TEST_TIMEOUT"], "60")
        self.assertEqual(browser_env["CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER"], runner)
        self.assertIsNone(browser_env["NO_HEADLESS"], "NO_HEADLESS is popped for headless browser tests")

    def test_artifacts_bundle_alias_parity_recording_stub(self):
        version = next(p["version"] for p in verifier.tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
                       if p["name"] == "wasm-bindgen")
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        plan = {"wasm-bindgen": {"stdout": f"wasm-bindgen {version}\n", "write_bundle": True},
                "_bundle_templates": str(ROOT / "crates/gh-report/templates")}
        expected_bind = [str(ROOT / "target/wasm32-unknown-unknown/release/gh_report_web_client.wasm"),
                         "--target", "web", "--out-dir", "<out-dir>", "--out-name", "gh-report-web-client"]
        def normalize(mode_calls):
            result = []
            for c in mode_calls:
                argv = list(c["argv"])
                for i in range(len(argv) - 1):
                    if argv[i] == "--out-dir":
                        argv[i + 1] = "<out-dir>"
                result.append((c["prog"], argv))
            return result
        def assert_full_matrix(mode, calls):
            self.assertEqual([c["prog"] for c in calls],
                             ["wasm-bindgen", "cargo", "wasm-bindgen"],
                             f"{mode} must dispatch version/build/bind exactly once each")
            for call in calls:
                self.assertEqual(Path(call["cwd"]), ROOT,
                                 f"{mode}: every dispatch runs at ROOT")
            version_call, build_call, bind_call = calls
            self.assertEqual(version_call["argv"], ["--version"])
            self.assertEqual(version_call["env"]["CARGO_TARGET_DIR"],
                             os.environ.get("CARGO_TARGET_DIR"),
                             "--version inherits ambient env, not bound_env")
            self.assertEqual(version_call["env"]["CARGO_TERM_PROGRESS_WHEN"],
                             os.environ.get("CARGO_TERM_PROGRESS_WHEN"))
            self.assertEqual(build_call["argv"],
                             [f"+{channel}", "build", "-p", "gh-report-web-client",
                              "--target", "wasm32-unknown-unknown", "--locked", "--release"])
            self.assertEqual(build_call["env"]["CARGO_TARGET_DIR"], str(ROOT / "target"))
            self.assertEqual(build_call["env"]["CARGO_TERM_PROGRESS_WHEN"], "never")
            self.assertEqual(normalize([bind_call])[0][1], expected_bind)
            self.assertEqual(bind_call["env"]["CARGO_TARGET_DIR"], str(ROOT / "target"))
            self.assertEqual(bind_call["env"]["CARGO_TERM_PROGRESS_WHEN"], "never")
        matrices = []
        for mode in ("artifacts", "bundle"):
            done, calls = self._run_stubbed(mode, ("cargo", "wasm-bindgen"), plan)
            self.assertEqual(done.returncode, 0, done.stderr)
            assert_full_matrix(mode, calls)
            matrices.append(normalize(calls))
        self.assertEqual(matrices[0], matrices[1], "artifacts/bundle alias parity")

    def test_native_compiler_alias_parity_recording_stub(self):
        def normalize(entry):
            prog, argv, stdin_head = entry
            return (prog, [a if not a.endswith("probe.rmeta") else "<probe-out>" for a in argv],
                    stdin_head)
        matrices = []
        for mode in ("native", "compiler"):
            done, calls = self._run_stubbed(mode, ("cargo", "rustc"))
            self.assertEqual(done.returncode, 0, done.stderr)
            matrices.append([normalize((c["prog"], c["argv"], c.get("stdin_head", ""))) for c in calls])
        self.assertEqual(matrices[0], matrices[1], "native/compiler alias parity")

    def test_compiler_subprocess_timeouts(self):
        def child(args, **kwargs):
            if args[0] == "cargo":
                artifact = {"reason": "compiler-artifact", "target": {"name": "gh_report_web_client"},
                            "filenames": [str(ROOT / "target/debug/libgh_report_web_client.rlib")]}
                return subprocess.CompletedProcess(args, 0, stdout=json.dumps(artifact))
            src = kwargs.get("input", "")
            if "::missing_sort::" in src:
                wrong = {"$message_type": "diagnostic", "level": "error",
                         "code": {"code": "E0432"},
                         "message": "cannot find sort in gh_report_web_client", "spans": []}
                return subprocess.CompletedProcess(args, 1, stdout="", stderr=json.dumps(wrong))
            positive = "SortDirection::Ascending" in src
            if positive:
                return subprocess.CompletedProcess(args, 0, stdout="", stderr="")
            return subprocess.CompletedProcess(args, 1, stdout="", stderr=json.dumps(self.E0061))
        seen = []
        def child_with_timeout(args, **kwargs):
            seen.append((args[0], kwargs.get("timeout")))
            return child(args, **kwargs)
        code, _, error = self.invoke("compiler", child=child_with_timeout)
        self.assertEqual(code, 0, error)
        cargo = next(s for s in seen if s[0] == "cargo")
        probes = [s for s in seen if s[0] == "rustc"]
        self.assertEqual(cargo[1], 120)
        self.assertEqual(len(probes), 3)
        self.assertTrue(all(s[1] == 30 for s in probes), probes)

    def test_prepare_cold_arg_env_timeout_matrix(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        calls = []
        def child(args, **kwargs):
            calls.append((args, kwargs.get("timeout"), kwargs.get("env")))
            return subprocess.CompletedProcess(args, 0)
        code, output, error = self.invoke("prepare", child=child)
        self.assertEqual(code, 0, error)
        self.assertIn("VERIFIED: prepare", output)
        build, tests = calls
        self.assertEqual(build[0], ["cargo", f"+{channel}", "build", "-p", "gh-report-web-client",
                                    "--lib", "--locked"])
        self.assertEqual(tests[0], ["cargo", f"+{channel}", "test", "-p", "gh-report-web-client",
                                    "--target", "wasm32-unknown-unknown", "--locked", "--lib", "--no-run"])
        for _, timeout, env in calls:
            self.assertEqual(timeout, verifier.COLD_JOB_BUDGET)
            self.assertEqual(verifier.COLD_JOB_BUDGET, 1800)
            self.assertEqual(env["CARGO_TARGET_DIR"], str(ROOT / "target"))
            self.assertEqual(env["CARGO_TERM_PROGRESS_WHEN"], "never")

    def test_wasm_cold_arg_env_timeout_matrix(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        calls = []
        def child(args, **kwargs):
            calls.append((args, kwargs.get("timeout")))
            return subprocess.CompletedProcess(args, 0)
        code, output, error = self.invoke("wasm", child=child)
        self.assertEqual(code, 0, error)
        self.assertIn("VERIFIED: wasm", output)
        build, clippy = calls
        self.assertEqual(build[0], ["cargo", f"+{channel}", "build", "-p", "gh-report-web-client",
                                    "--target", "wasm32-unknown-unknown", "--release", "--locked"])
        self.assertEqual(clippy[0], ["cargo", f"+{channel}", "clippy", "-p", "gh-report-web-client",
                                     "--target", "wasm32-unknown-unknown", "--all-targets", "--locked",
                                     "--", "-D", "warnings"])
        self.assertEqual(build[1], verifier.COLD_JOB_BUDGET)
        self.assertEqual(clippy[1], verifier.COLD_JOB_BUDGET)
        self.assertEqual(verifier.COLD_JOB_BUDGET, 1800)

    def test_prepare_cold_timeout_distinct_from_warm(self):
        def timeout(args, **kwargs):
            self.assertEqual(kwargs["timeout"], verifier.COLD_JOB_BUDGET)
            self.assertNotEqual(kwargs["timeout"], verifier.WARM_HELPER_TIMEOUT)
            raise subprocess.TimeoutExpired(args, kwargs["timeout"])
        code, _, error = self.invoke("prepare", child=timeout)
        self.assertEqual(code, 2)
        self.assertIn("UNAVAILABLE", error)
        self.assertNotIn("FAILED", error)

    def test_imports_plant_fail_revert_clean(self):
        valid_dump = '(module (import "./gh-report-web-client_bg.js" (func)))\n'
        valid_glue = '"./gh-report-web-client_bg.js": import0,'
        def wasm_child(dump=valid_dump, print_rc=0, validate_rc=0, calls=None):
            def child(args, **kwargs):
                self.assertEqual(args[0], "wasm-tools")
                self.assertEqual(kwargs["timeout"], 60)
                if calls is not None:
                    calls.append((args[1], kwargs.get("input", None)))
                if args[1] == "print":
                    return subprocess.CompletedProcess(args, print_rc, stdout=dump, stderr="")
                self.assertEqual(args[1], "validate")
                self.assertEqual(kwargs["input"], dump,
                                 "validate must receive the exact captured print text")
                return subprocess.CompletedProcess(args, validate_rc, stdout="", stderr="")
            return child
        calls = []
        def clean():
            calls.clear()
            return self.invoke("imports", child=wasm_child(calls=calls),
                               which=lambda tool: tool if tool == "wasm-tools" else None)
        code, output, error = clean()
        self.assertEqual(code, 0, error)
        self.assertIn("VERIFIED: imports", output)
        self.assertEqual([c[0] for c in calls], ["print", "validate"])
        def with_glue(glue, dump=valid_dump, print_rc=0, validate_rc=0):
            def read(path, original):
                if path.name == "gh-report-web-client.js":
                    return glue
                return original
            return self.invoke("imports", read_text=read,
                               child=wasm_child(dump, print_rc, validate_rc, calls),
                               which=lambda tool: tool if tool == "wasm-tools" else None)
        code, _, error = with_glue('"bad.js": import0,')
        self.assertEqual(code, 1)
        self.assertIn("glue import key", error)
        self.assertEqual(clean()[0], 0)
        code, _, error = with_glue(valid_glue, dump='(module (import "./bad.js" (func)))\n')
        self.assertEqual(code, 1)
        self.assertIn("wasm import modules", error)
        self.assertEqual(clean()[0], 0)
        for rc in (0, 1):
            with self.subTest(rc=rc):
                code, _, error = with_glue(valid_glue, dump="", print_rc=rc)
                self.assertEqual(code, 2)
                self.assertIn("UNPARSEABLE_IMPORTS", error)
        self.assertEqual(clean()[0], 0)
        for dump in ('garbage (module (import "./gh-report-web-client_bg.js" (func 0))',
                     '(import (func 0))\n'):
            with self.subTest(dump=dump):
                code, _, error = with_glue(valid_glue, dump=dump, validate_rc=1)
                self.assertEqual(code, 2, error)
                self.assertIn("UNPARSEABLE_IMPORTS", error)
                self.assertNotIn("FAILED", error)
        self.assertEqual(clean()[0], 0)

    def test_imports_missing_wasm_tools_is_unavailable(self):
        code, _, error = self.invoke("imports", which=lambda tool: None)
        self.assertEqual(code, 2)
        self.assertIn("UNAVAILABLE: wasm-tools", error)

    def test_cold_job_budget_preserved_30min_distinct_from_warm(self):
        self.assertEqual(verifier.COLD_JOB_BUDGET, 1800,
                         "cold job-owned ceiling stays 30min; the 300s narrowing is a regression")
        self.assertEqual(verifier.WARM_HELPER_TIMEOUT, 180)
        self.assertGreater(verifier.COLD_JOB_BUDGET, verifier.WARM_HELPER_TIMEOUT)

    def test_wasm_release_failure_still_runs_clippy_aggregate(self):
        channel = verifier.tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
        calls = []
        def child(args, **kwargs):
            calls.append(list(args))
            rc = 1 if "build" in args else 0
            return subprocess.CompletedProcess(args, rc,
                                               stderr="release build exploded\n" if rc else "")
        code, _, error = self.invoke("wasm", child=child)
        self.assertEqual(code, 1)
        self.assertIn("WASM_FAILED", error)
        self.assertIn("release build exploded", error)
        self.assertEqual([c[1:3] for c in calls], [[f"+{channel}", "build"], [f"+{channel}", "clippy"]],
                         "release-build failure must not suppress the wasm-target clippy lane")
        def both_fail(args, **kwargs):
            return subprocess.CompletedProcess(args, 1, stderr=f"{args[1]} failed\n")
        code, _, error = self.invoke("wasm", child=both_fail)
        self.assertEqual(code, 1)
        self.assertIn("build", error)
        self.assertIn("clippy", error)
        self.assertIn("WASM_FAILED", error)

    def test_prepare_missing_cargo_operational_is_unavailable(self):
        def missing(args, **kwargs):
            raise FileNotFoundError(2, "No such file or directory", args[0])
        code, _, error = self.invoke("prepare", child=missing)
        self.assertEqual(code, 2)
        self.assertIn("UNAVAILABLE", error)
        self.assertNotIn("FAILED", error)

    def test_wasm_operational_oserror_is_unavailable(self):
        def failed(args, **kwargs):
            raise OSError(13, "Permission denied", args[0])
        code, _, error = self.invoke("wasm", child=failed)
        self.assertEqual(code, 2)
        self.assertIn("UNAVAILABLE", error)
        self.assertNotIn("FAILED", error)

    def test_wasm_mixed_failed_and_unavailable_aggregate_exit_1(self):
        def mixed(args, **kwargs):
            if "build" in args:
                return subprocess.CompletedProcess(args, 1, stdout="", stderr="BUILD_DOMAIN_WITNESS\n")
            raise FileNotFoundError(2, "No such file or directory", str(args[0]))
        code, _, error = self.invoke("wasm", child=mixed)
        self.assertEqual(code, 1, error)
        self.assertIn("BUILD_DOMAIN_WITNESS", error)
        self.assertIn("FileNotFoundError", error)
        self.assertNotIn("timed out", error)

    def test_wasm_all_unavailable_exit_2_no_synthetic_timeout(self):
        def denied(args, **kwargs):
            raise OSError(13, "Permission denied", args[0])
        code, _, error = self.invoke("wasm", child=denied)
        self.assertEqual(code, 2, error)
        self.assertIn("UNAVAILABLE", error)
        self.assertIn("build", error)
        self.assertIn("clippy", error)
        self.assertNotIn("FAILED", error)
        self.assertNotIn("timed out", error)

    def test_imports_garbage_dump_is_unavailable_not_failed(self):
        def garbage(args, **kwargs):
            self.assertEqual(args[0], "wasm-tools")
            if args[1] == "print":
                return subprocess.CompletedProcess(args, 0, stdout='(import (func 0))\n')
            self.assertEqual(args[1], "validate")
            return subprocess.CompletedProcess(args, 1, stdout="", stderr="invalid wasm text\n")
        code, _, error = self.invoke("imports", child=garbage,
                                     which=lambda tool: tool if tool == "wasm-tools" else None)
        self.assertEqual(code, 2)
        self.assertIn("UNPARSEABLE_IMPORTS", error)
        self.assertNotIn("FAILED", error)

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
        self.assertEqual(
            self.invoke("status", child=lambda *args, **kwargs: self.fail(
                "status is a source-only scan and must not dispatch subprocesses"))[0], 0)
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
