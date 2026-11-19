#!/usr/bin/env python3
"""Regression suite for tools/sbom_merge.py: python3 -B tools/test_sbom_merge.py.

Fixture provenance: inputs under tools/fixtures/sbom/ are small, hand-shaped
documents mirroring cargo metadata / cargo-cyclonedx 0.5.7 output, not captured
scans. `image-bom.json` is a documented shape fixture, not a claimed scan: the
release base image is distroless `gcr.io/distroless/cc-debian12:nonroot`, which
has no OS package database, so a real scan yields approximately zero OS
components, and `syft` is not installed on the development host (only `podman`
is). The release workflow replaces the fixture with an actual scan of the
pushed digest. See tools/fixtures/sbom/README.md.
"""

import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
TOOL = HERE / "sbom_merge.py"
FIXTURES = HERE / "fixtures" / "sbom"

sys.path.insert(0, str(HERE))
import sbom_merge  # noqa: E402

DIGEST = "sha256:" + "0123456789abcdef" * 4
OTHER_DIGEST = "sha256:" + "fedcba9876543210" * 4
REPO_URL = "https://github.com/Mattilsynet/gh-report"
TAG = "v0.1.108"
ROOT_REF = "path+file:///fixture/repo/crates/gh-report#0.0.0"
ASKAMA_REF = "registry+https://github.com/rust-lang/crates.io-index#askama@0.16.1"
SERDE_REF = "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.229"


def run_tool(*args):
    return subprocess.run(
        [sys.executable, "-B", str(TOOL)] + [str(arg) for arg in args],
        capture_output=True,
        text=True,
    )


def load_fixture(name):
    with open(FIXTURES / name, "r", encoding="utf-8") as handle:
        return json.load(handle)


def shipset_lines():
    with open(FIXTURES / "shipset.txt", "r", encoding="utf-8") as handle:
        return [line.strip() for line in handle if line.strip()]


class ToolTestCase(unittest.TestCase):
    def setUp(self):
        self.scratch = Path(tempfile.mkdtemp(prefix="sbom-merge-test-"))
        self.addCleanup(shutil.rmtree, str(self.scratch))

    def write_json(self, name, payload):
        path = self.scratch / name
        path.write_text(json.dumps(payload), encoding="utf-8")
        return path

    def write_shipset(self, name, lines):
        path = self.scratch / name
        path.write_text("".join(line + "\n" for line in lines), encoding="utf-8")
        return path

    def merge(self, out_name="bom.json", cargo=None, image=None, shipset=None, check=True):
        out = self.scratch / out_name
        args = [
            "merge",
            "--cargo-bom", cargo or FIXTURES / "cargo-bom.json",
            "--image-bom", image or FIXTURES / "image-bom.json",
            "--shipset", shipset or FIXTURES / "shipset.txt",
            "--repository-url", REPO_URL,
            "--digest", DIGEST,
            "--tag", TAG,
            "--out", out,
        ]
        if check:
            args.append("--check")
        proc = run_tool(*args)
        return proc, out

    def merged(self):
        proc, out = self.merge()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        return proc, json.loads(out.read_text(encoding="utf-8"))


class MergeTest(ToolTestCase):
    def test_happy_path_merge_shape(self):
        proc, document = self.merged()
        root = document["metadata"]["component"]
        self.assertEqual(root["type"], "container")
        self.assertEqual(root["name"], "gh-report")
        self.assertEqual(root["version"], TAG)
        self.assertEqual(root["bom-ref"], root["purl"])
        self.assertTrue(root["purl"].startswith("pkg:oci/gh-report@sha256%3A"))
        self.assertIn("repository_url=https%3A%2F%2Fgithub.com%2F", root["purl"])
        self.assertIn("&tag=" + TAG, root["purl"])

        components = document["components"]
        refs = [component["bom-ref"] for component in components]
        names = [component["name"] for component in components]
        self.assertEqual(len(components), 7)
        self.assertEqual(len(set(refs)), 7)
        self.assertEqual(refs, sorted(refs))
        self.assertEqual(
            sorted(names),
            ["askama", "base-files", "fastrand", "gh-report", "libc6", "serde", "zstd"],
        )
        self.assertEqual(document["version"], 1)
        self.assertNotIn("serialNumber", document)
        self.assertNotIn("timestamp", document["metadata"])
        self.assertNotIn("tools", document["metadata"])
        self.assertNotIn("signature", json.dumps(document))
        self.assertNotIn(
            "download_url=file:",
            json.dumps(document),
            "build-host file: download_url qualifiers must not reach the release document",
        )
        application = [
            component
            for component in components
            if component["bom-ref"] == ROOT_REF
        ]
        self.assertEqual(len(application), 1)
        self.assertEqual(application[0]["purl"], "pkg:cargo/gh-report@0.0.0")

        dependencies = {entry["ref"]: entry["dependsOn"] for entry in document["dependencies"]}
        self.assertEqual([entry["ref"] for entry in document["dependencies"]],
                         sorted(dependencies))
        self.assertIn(ROOT_REF, dependencies[root["purl"]])
        self.assertIn("pkg:deb/debian/base-files@12.3?arch=amd64", dependencies[root["purl"]])
        self.assertIn(ASKAMA_REF, dependencies[root["purl"]])
        self.assertNotIn(
            "registry+https://github.com/rust-lang/crates.io-index#askama_derive@0.16.1",
            dependencies[root["purl"]],
        )
        self.assertEqual(
            dependencies[ASKAMA_REF],
            [],
            "the excluded proc-macro edge must not survive into the graph",
        )
        for ref, targets in dependencies.items():
            self.assertIn(ref, refs + [root["purl"]], "dangling dependency ref " + ref)
            for target in targets:
                self.assertIn(target, refs + [root["purl"]], "dangling dependsOn " + target)

    def test_summary_is_a_json_object_on_stdout_and_reports_go_to_stderr(self):
        proc, out = self.merge()
        self.assertEqual(proc.returncode, 0, proc.stderr)
        summary = json.loads(proc.stdout)
        self.assertEqual(
            summary,
            {
                "bom": str(out),
                "check": "passed",
                "components": 7,
                "dropped_image_components": 1,
                "excluded_components": 2,
            },
        )
        self.assertEqual(proc.stdout.strip().count("\n"), 0)
        self.assertIn("excluded: askama_derive@0.16.1 (not-in-shipset)", proc.stderr)
        self.assertIn("excluded: basic-toml@0.1.10 (not-in-shipset)", proc.stderr)
        self.assertIn("dropped-image: file-gh-report-binary (no-purl)", proc.stderr)
        self.assertNotIn("excluded:", proc.stdout)
        self.assertNotIn("dropped-image:", proc.stdout)

    def test_two_merges_are_byte_identical(self):
        first_proc, first = self.merge("first.json")
        second_proc, second = self.merge("second.json")
        self.assertEqual(first_proc.returncode, 0, first_proc.stderr)
        self.assertEqual(second_proc.returncode, 0, second_proc.stderr)
        first_bytes = first.read_bytes()
        self.assertEqual(first_bytes, second.read_bytes())
        self.assertTrue(first_bytes.endswith(b"\n"))

    def test_excluded_cargo_components_are_dropped_and_reported(self):
        proc, out = self.merge()
        text = out.read_text(encoding="utf-8")
        self.assertNotIn("askama_derive", text)
        self.assertNotIn("basic-toml", text)
        self.assertIn("excluded: askama_derive@0.16.1 (not-in-shipset)", proc.stderr)
        self.assertIn("excluded: basic-toml@0.1.10 (not-in-shipset)", proc.stderr)
        self.assertEqual(
            [line for line in proc.stderr.splitlines() if line.startswith("excluded:")],
            [
                "excluded: askama_derive@0.16.1 (not-in-shipset)",
                "excluded: basic-toml@0.1.10 (not-in-shipset)",
            ],
            "the dropped set is complete and sorted",
        )

    def test_component_without_purl_gets_a_cargo_purl(self):
        _, document = self.merged()
        serde = [
            component for component in document["components"] if component["name"] == "serde"
        ]
        self.assertEqual(len(serde), 1)
        self.assertEqual(serde[0]["purl"], "pkg:cargo/serde@1.0.229")
        self.assertEqual(serde[0]["bom-ref"], SERDE_REF)

    def test_image_component_without_purl_is_dropped_and_reported(self):
        proc, out = self.merge()
        text = out.read_text(encoding="utf-8")
        self.assertNotIn("file-gh-report-binary", text)
        self.assertIn("dropped-image: file-gh-report-binary (no-purl)", proc.stderr)
        self.assertEqual(json.loads(proc.stdout)["dropped_image_components"], 1)

    def test_bad_digest_is_a_usage_error(self):
        proc = run_tool(
            "merge",
            "--cargo-bom", FIXTURES / "cargo-bom.json",
            "--image-bom", FIXTURES / "image-bom.json",
            "--shipset", FIXTURES / "shipset.txt",
            "--repository-url", REPO_URL,
            "--digest", "sha256:nothex",
            "--tag", TAG,
            "--out", self.scratch / "bom.json",
        )
        self.assertEqual(proc.returncode, 2)
        self.assertIn("::error::usage: --digest must be sha256:", proc.stderr)


class FailClosedTest(ToolTestCase):
    def assert_fails(self, proc, code, detail):
        self.assertEqual(proc.returncode, 1, proc.stdout + proc.stderr)
        self.assertIn("::error::%s: %s" % (code, detail), proc.stderr)

    def test_banned_component_fails_closed(self):
        cargo = load_fixture("cargo-bom.json")
        cargo["components"].append(
            {
                "type": "library",
                "bom-ref": "registry+https://github.com/rust-lang/crates.io-index#proptest@1.0.0",
                "name": "proptest",
                "version": "1.0.0",
                "purl": "pkg:cargo/proptest@1.0.0",
            }
        )
        cargo_path = self.write_json("cargo-banned.json", cargo)
        shipset = self.write_shipset("shipset-banned.txt", shipset_lines() + ["proptest@1.0.0"])
        proc, out = self.merge(cargo=cargo_path, shipset=shipset)
        self.assert_fails(proc, "banned-component", "proptest@1.0.0")
        self.assertFalse(out.exists(), "check mode must not leave a rejected document behind")

    def test_file_component_fails_closed(self):
        image = load_fixture("image-bom.json")
        image["components"].append(
            {
                "type": "file",
                "bom-ref": "pkg:generic/not-a-component@1",
                "name": "not-a-component",
                "version": "1",
                "purl": "pkg:generic/not-a-component@1",
            }
        )
        image_path = self.write_json("image-file.json", image)
        proc, _ = self.merge(image=image_path)
        self.assert_fails(proc, "component-type", "not-a-component")

    def test_github_reference_fails_closed(self):
        image = load_fixture("image-bom.json")
        image["components"].append(
            {
                "type": "library",
                "bom-ref": "pkg:github/Mattilsynet/gh-report@deadbeef",
                "name": "gh-report-source",
                "version": "deadbeef",
                "purl": "pkg:github/Mattilsynet/gh-report@deadbeef",
            }
        )
        image_path = self.write_json("image-github.json", image)
        proc, _ = self.merge(image=image_path)
        self.assert_fails(proc, "github-reference", "pkg:github/Mattilsynet/gh-report@deadbeef")

    def test_missing_shipset_component_fails_closed(self):
        cargo = load_fixture("cargo-bom.json")
        cargo["components"] = [
            component for component in cargo["components"] if component["name"] != "askama"
        ]
        cargo["dependencies"] = [
            entry for entry in cargo["dependencies"] if entry["ref"] != ASKAMA_REF
        ]
        cargo_path = self.write_json("cargo-missing.json", cargo)
        proc, _ = self.merge(cargo=cargo_path)
        self.assert_fails(proc, "missing-shipset-component", "askama@0.16.1")

    def test_duplicate_bom_ref_across_sources_fails_closed(self):
        image = load_fixture("image-bom.json")
        image["components"].append(
            {
                "type": "library",
                "bom-ref": ASKAMA_REF,
                "name": "askama-scan-copy",
                "version": "0.16.1",
                "purl": "pkg:deb/debian/askama@0.16.1",
            }
        )
        image_path = self.write_json("image-duplicate.json", image)
        proc, _ = self.merge(image=image_path)
        self.assert_fails(proc, "duplicate-bom-ref", ASKAMA_REF)

    def test_check_rejects_each_document_failure_class(self):
        document, _, _ = sbom_merge.build_document(
            load_fixture("cargo-bom.json"),
            load_fixture("image-bom.json"),
            shipset_lines(),
            REPO_URL,
            DIGEST,
            TAG,
        )
        shipset = shipset_lines()
        self.assertEqual(
            sbom_merge.check_document(document, shipset, DIGEST),
            [],
            "the merged shape fixture must satisfy check mode",
        )

        def codes(mutate):
            cloned = json.loads(json.dumps(document))
            mutate(cloned)
            return [code for code, _ in sbom_merge.check_document(cloned, shipset, DIGEST)]

        def wrong_root_type(cloned):
            cloned["metadata"]["component"]["type"] = "library"

        def wrong_root_name(cloned):
            root = cloned["metadata"]["component"]
            root["purl"] = root["purl"].replace("pkg:oci/gh-report@", "pkg:oci/other@")
            root["bom-ref"] = root["purl"]

        def wrong_digest(cloned):
            root = cloned["metadata"]["component"]
            root["purl"] = root["purl"].replace(
                "sha256%3A" + DIGEST.split(":", 1)[1],
                "sha256%3A" + OTHER_DIGEST.split(":", 1)[1],
            )
            root["bom-ref"] = root["purl"]

        def local_download_qualifier(cloned):
            for component in cloned["components"]:
                if component["bom-ref"] == ASKAMA_REF:
                    component["purl"] = (
                        "pkg:cargo/askama@0.16.1?download_url=file:///build/host"
                    )

        def missing_crate(cloned):
            cloned["components"] = [
                component for component in cloned["components"] if component["name"] != "askama"
            ]
            cloned["dependencies"] = [
                entry
                for entry in cloned["dependencies"]
                if entry["ref"] != ASKAMA_REF and ASKAMA_REF not in entry["dependsOn"]
            ]

        def expect(mutate, expected, exact=True):
            observed = codes(mutate)
            self.assertIn(expected, observed, "expected %s in %s" % (expected, observed))
            if exact:
                self.assertEqual(
                    observed, [expected], "each mutation must trip exactly one check"
                )

        expect(wrong_root_type, "root-component")
        expect(wrong_root_name, "root-component", exact=False)
        expect(wrong_digest, "digest-mismatch", exact=False)
        expect(local_download_qualifier, "component-purl")
        expect(missing_crate, "missing-shipset-component")


class ShipsetTest(ToolTestCase):
    def test_shipset_excludes_proc_macro_and_its_subtree(self):
        proc = run_tool(
            "shipset",
            "--metadata", FIXTURES / "cargo-metadata.json",
            "--root", "gh-report",
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        shipped = proc.stdout.split()
        self.assertEqual(
            shipped,
            [
                "askama@0.16.1",
                "fastrand@2.5.0",
                "gh-report@0.0.0",
                "serde@1.0.229",
                "zstd@0.14.0",
            ],
        )
        self.assertNotIn("askama_derive@0.16.1", shipped)
        self.assertNotIn("askama_parser@0.16.1", shipped)
        self.assertNotIn("serde_derive@1.0.229", shipped)
        self.assertNotIn("basic-toml@0.1.10", shipped)
        self.assertNotIn("build-helper@1.0.0", shipped)
        self.assertEqual(
            [line for line in proc.stderr.splitlines() if line.startswith("excluded:")],
            [
                "excluded: askama_derive@0.16.1 (proc-macro)",
                "excluded: askama_parser@0.16.1 (behind-proc-macro)",
                "excluded: serde_derive@1.0.229 (proc-macro)",
            ],
        )
        self.assertIn("shipset: 5 shipped, 3 excluded", proc.stderr)

    def test_shipset_out_file_keeps_stdout_empty(self):
        out = self.scratch / "shipset.txt"
        proc = run_tool(
            "shipset",
            "--metadata", FIXTURES / "cargo-metadata.json",
            "--root", "gh-report",
            "--out", out,
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertEqual(proc.stdout, "")
        self.assertEqual(
            out.read_text(encoding="utf-8").split(), sorted(shipset_lines())
        )


if __name__ == "__main__":
    unittest.main()
