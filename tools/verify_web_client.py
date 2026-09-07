"""Local checks: python3.12 -B tools/verify_web_client.py {bundle,browser,status,compiler}.

Exit 0 means verified; 1 means mismatch/failure; 2 means unavailable.
Byte identity is host/build-environment specific, not cross-host reproducibility.
Requires Python 3.11+, pinned Rust target and matching installed bindgen tools.
Browser mode requires an installed chromedriver and its compatible Chrome.
No tools are downloaded and committed bundles are never overwritten.
Timeouts and unparseable source markers mean unavailable, not a defect.
Status checks are source scans; vocabulary changes require updating
the explicit expected set here. CI invokes status/compiler/browser; bundle
remains local pending hosted acceptance (ghr-6fg0y). --ci annotates unavailable
as a warning while retaining exit 2: incomplete verification blocks CI.
CI precompiles host and wasm test targets under its existing 30-minute job
budget before timed checks. Cold CI duration is unmeasured; these deadlines
are execution limits, not a claim that cold builds finish within them.
General direct children have a 180s timeout; browser descendants are not bounded or
reaped by this script. Browser execution requires separate environment validation.
Compiler proof: python3.12 -B tools/verify_web_client.py compiler
builds the host library, accepts the directed call, and demands E0061 for the
missing direction. Unrecognized compiler JSON/schema is unavailable (exit 2).
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
try:
    import tomllib
except ModuleNotFoundError:
    tomllib = None

ROOT = Path(__file__).resolve().parents[1]
NAMES = ("gh-report-web-client.js", "gh-report-web-client_bg.wasm")


def run(args, env=None, capture=False):
    return subprocess.run(args, cwd=ROOT, env=env, check=True, timeout=180,
                          text=True, stdout=subprocess.PIPE if capture else None)


def compare_bundles(generated, committed):
    mismatches = [name for name in NAMES
                  if (generated / name).read_bytes() != (committed / name).read_bytes()]
    if mismatches:
        raise ValueError("MISMATCH: " + ", ".join(mismatches))


def compiler_diagnostics(result):
    try:
        diagnostics = [json.loads(line) for line in result.stderr.splitlines()]
    except json.JSONDecodeError as error:
        raise UnparseableSource("UNPARSEABLE_DIAGNOSTICS: non-JSON rustc output") from error
    for item in diagnostics:
        if (not isinstance(item, dict) or item.get("$message_type") != "diagnostic"
                or not isinstance(item.get("level"), str)
                or not isinstance(item.get("message"), str)
                or "code" not in item
                or (item["code"] is not None and
                    (not isinstance(item["code"], dict) or not isinstance(item["code"].get("code"), str)))
                or not isinstance(item.get("spans"), list)
                or any(not isinstance(span, dict) or "label" not in span
                       or (span["label"] is not None and not isinstance(span["label"], str))
                       for span in item["spans"])):
            raise UnparseableSource("UNPARSEABLE_DIAGNOSTICS: rustc schema drift")
    if result.returncode != 0 and not diagnostics:
        raise UnparseableSource("UNPARSEABLE_DIAGNOSTICS: missing rustc diagnostics")
    return diagnostics


def require_missing_direction(result):
    diagnostics = compiler_diagnostics(result)
    errors = [item for item in diagnostics if item.get("level") == "error" and item.get("code")]
    if (result.returncode != 1 or len(errors) != 1
            or errors[0]["code"]["code"] != "E0061"
            or errors[0]["message"] != "this function takes 4 arguments but 3 arguments were supplied"
            or not any(span.get("label") == "argument #4 of type `SortDirection` is missing"
                       for span in errors[0]["spans"])):
        raise ValueError(f"EXPECTED_MISSING_DIRECTION: exit={result.returncode}\n{result.stderr}")


class UnparseableSource(Exception):
    pass


def compiler_contract(channel):
    built = subprocess.run(
        ["cargo", f"+{channel}", "build", "-p", "gh-report-web-client", "--lib", "--locked", "--message-format=json"],
        cwd=ROOT, capture_output=True, text=True, timeout=120, check=True)
    try:
        artifacts = [json.loads(line) for line in built.stdout.splitlines()]
        rlibs = [Path(name) for artifact in artifacts
                 if artifact.get("reason") == "compiler-artifact"
                 and artifact["target"]["name"] == "gh_report_web_client"
                 for name in artifact["filenames"] if name.endswith(".rlib")]
    except (ValueError, TypeError, KeyError, AttributeError) as error:
        raise UnparseableSource("UNPARSEABLE_ARTIFACTS: cargo schema drift") from error
    if len(rlibs) != 1:
        raise UnparseableSource("UNPARSEABLE_ARTIFACTS: expected one host rlib")
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
        compiler_diagnostics(good)
        if good.returncode != 0:
            raise ValueError(f"POSITIVE_COMPILER_PROBE: {good.stderr}")
        require_missing_direction(compile_source(missing))
    print("VERIFIED: compiler; positive=0; missing-direction=1 E0061")
    return 0


def source_body(source, marker, end):
    if source.count(marker) != 1:
        raise UnparseableSource(f"UNPARSEABLE_SOURCE: {marker}")
    _, _, remaining = source.partition(marker)
    body, separator, _ = remaining.partition(end)
    if not separator:
        raise UnparseableSource(f"UNPARSEABLE_SOURCE: missing closing marker for {marker}")
    return body


def status_contract(producer, consumer):
    body = source_body(producer, "pub fn sort_key(&self)", "\n    }")
    emitted = set(re.findall(r'=> "([^"]+)"', body))
    body = source_body(consumer, "fn known_status(value:", "\n}")
    accepted = set(re.findall(r'"([^"]+)" => Some\(KnownStatus::', body))
    if emitted != {"fail", "partial", "pass", "indeterminate"} or accepted != emitted - {"indeterminate"}:
        raise ValueError(f"INVALID_STATUS_KEYS: emitted={sorted(emitted)}, accepted={sorted(accepted)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("bundle", "browser", "status", "compiler"))
    parser.add_argument("--ci", action="store_true", help="annotate verdicts without relaxing exit codes")
    args = parser.parse_args()
    if tomllib is None:
        print("UNAVAILABLE: Python 3.11+ required (try python3.12)", file=sys.stderr)
        return 2
    if args.mode == "status":
        status_contract((ROOT / "crates/gh-report/src/report/view_model.rs").read_text(),
                        (ROOT / "crates/gh-report-web-client/src/sort.rs").read_text())
        print("VERIFIED: source-level status vocabulary (not a compiled cross-crate contract)")
        return 0
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    if args.mode == "compiler":
        return compiler_contract(channel)
    packages = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    versions = {p["version"] for p in packages if p["name"] == "wasm-bindgen"}
    if len(versions) != 1:
        raise ValueError("ambiguous locked wasm-bindgen version")
    version = versions.pop()
    tool = "wasm-bindgen" if args.mode == "bundle" else "wasm-bindgen-test-runner"
    if not shutil.which(tool):
        print(f"UNAVAILABLE: {tool}", file=sys.stderr)
        return 2
    if run([tool, "--version"], capture=True).stdout.strip() != f"{tool} {version}":
        raise ValueError(f"{tool} must match locked version {version}")
    env = os.environ.copy()
    env["CARGO_TERM_PROGRESS_WHEN"] = "never"
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    cargo = ["cargo", f"+{channel}"]
    common = ["-p", "gh-report-web-client", "--target", "wasm32-unknown-unknown", "--locked"]
    if args.mode == "browser":
        driver = shutil.which("chromedriver")
        if not driver:
            print("UNAVAILABLE: chromedriver/compatible Chrome required; no browser tests executed", file=sys.stderr)
            return 2
        env.update(CHROMEDRIVER=driver, WASM_BINDGEN_USE_BROWSER="1", HEADLESS="1",
                   CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=tool,
                   WASM_BINDGEN_TEST_TIMEOUT="60")
        env.pop("NO_HEADLESS", None)
        run(cargo + ["test"] + common + ["--lib"], env=env)
    else:
        run(cargo + ["build"] + common + ["--release"], env=env)
        with tempfile.TemporaryDirectory(prefix="web-client-verify-", dir=ROOT / "target") as output:
            run([tool, str(ROOT / "target/wasm32-unknown-unknown/release/gh_report_web_client.wasm"),
                 "--target", "web", "--out-dir", output, "--out-name", "gh-report-web-client"], env=env)
            compare_bundles(Path(output), ROOT / "crates/gh-report/templates")
    print(f"VERIFIED: {args.mode}; rust={channel}; bindgen={version}")
    return 0


if __name__ == "__main__":
    try:
        result = main()
    except (subprocess.TimeoutExpired, UnparseableSource) as error:
        print(f"UNAVAILABLE: {error}", file=sys.stderr)
        result = 2
    except (ValueError, OSError, IndexError, subprocess.SubprocessError) as error:
        print(f"FAILED: {error}", file=sys.stderr)
        result = 1
    if "--ci" in sys.argv:
        if result == 2:
            print("::warning::UNAVAILABLE: verification incomplete, not a demonstrated defect; blocking CI (exit 2)")
        elif result != 0:
            print("::error::FAILED: web-client verification failed (exit 1); see diagnostics")
    sys.exit(result)
