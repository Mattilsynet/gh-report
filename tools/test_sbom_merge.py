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

The small fixtures sit below the 200-component floor, which is a check-mode
guard; therefore happy-path merge tests run without --check and assert the
floor independently through check_document().
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


def merge_args(out):
    return (
        "merge",
        "--cargo-bom", FIXTURES / "cargo-bom.json",
        "--image-bom", FIXTURES / "image-bom.json",
        "--shipset", FIXTURES / "shipset.txt",
        "--repository-url", REPO_URL,
        "--digest", DIGEST,
        "--tag", TAG,
        "--out", out,
    )


class MergeTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.mkdtemp(prefix="sbom-merge-test-")
        self.addCleanup(shutil.rmtree, self.scratch)

    def merge(self, name="bom.json"):
        out = str(Path(self.scratch) / name)
        proc = run_tool(*merge_args(out))
        self.assertEqual(proc.returncode, 0, proc.stderr)
        with open(out, "r", encoding="utf-8") as handle:
            return proc, handle.read()

    def test_happy_path_merge_shape(self):
        proc, text = self.merge()
        document = json.loads(text)
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
        self.assertEqual(
            sorted(names),
            ["askama", "base-files", "fastrand", "gh-report", "libc6", "serde", "zstd"],
        )
        self.assertEqual(document["version"], 1)
        self.assertNotIn("serialNumber", document)
        self.assertNotIn("timestamp", document["metadata"])
        self.assertNotIn("tools", document["metadata"])
        self.assertNotIn("signature", text)

        dependencies = {entry["ref"]: entry["dependsOn"] for entry in document["dependencies"]}
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

        self.assertIn("shipped-components: 7\n", proc.stdout)
        self.assertIn(
            "excluded-components: askama_derive@0.16.1, basic-toml@0.1.10\n",
            proc.stdout,
        )
        self.assertIn("dropped-image-components: 1\n", proc.stdout)

    def test_two_merges_are_byte_identical(self):
        self.merge("first.json")
        self.merge("second.json")
        first = (Path(self.scratch) / "first.json").read_bytes()
        second = (Path(self.scratch) / "second.json").read_bytes()
        self.assertEqual(first, second)
        self.assertTrue(first.endswith(b"\n"))

    def test_excluded_cargo_components_are_dropped_and_reported(self):
        proc, text = self.merge()
        self.assertNotIn("askama_derive", text)
        self.assertNotIn("basic-toml", text)
        self.assertIn(
            "excluded-components: askama_derive@0.16.1, basic-toml@0.1.10",
            proc.stdout,
        )

    def test_component_without_purl_gets_a_cargo_purl(self):
        _, text = self.merge()
        serde = [
            component
            for component in json.loads(text)["components"]
            if component["name"] == "serde"
        ]
        self.assertEqual(len(serde), 1)
        self.assertEqual(serde[0]["purl"], "pkg:cargo/serde@1.0.229")

    def test_image_component_without_purl_is_dropped_and_counted(self):
        proc, text = self.merge()
        self.assertNotIn("file-gh-report-binary", text)
        self.assertIn("dropped-image-components: 1\n", proc.stdout)

    def test_check_rejects_each_failure_class(self):
        document, _, _ = sbom_merge.build_document(
            load_fixture("cargo-bom.json"),
            load_fixture("image-bom.json"),
            shipset_lines(),
            REPO_URL,
            DIGEST,
            TAG,
        )
        shipset = shipset_lines()
        base = sbom_merge.check_document(document, shipset, DIGEST)
        self.assertEqual(
            [code for code, _ in base],
            ["component-count"],
            "the shape fixture is below the 200-component floor and nothing else",
        )

        def codes(mutate):
            cloned = json.loads(json.dumps(document))
            mutate(cloned)
            return [
                code
                for code, _ in sbom_merge.check_document(cloned, shipset, DIGEST)
            ]

        def wrong_root(cloned):
            cloned["metadata"]["component"]["type"] = "library"

        def github_reference(cloned):
            cloned["components"].append(
                {
                    "type": "library",
                    "bom-ref": "pkg:github/Mattilsynet/gh-report@deadbeef",
                    "name": "gh-report-source",
                    "version": "deadbeef",
                    "purl": "pkg:github/Mattilsynet/gh-report@deadbeef",
                }
            )

        def file_component(cloned):
            cloned["components"].append(
                {
                    "type": "file",
                    "bom-ref": "pkg:generic/not-a-component@1",
                    "name": "not-a-component",
                    "version": "1",
                    "purl": "pkg:generic/not-a-component@1",
                }
            )

        def banned_component(cloned):
            cloned["components"].append(
                {
                    "type": "library",
                    "bom-ref": "pkg:cargo/proptest@1.0.0",
                    "name": "proptest",
                    "version": "1.0.0",
                    "purl": "pkg:cargo/proptest@1.0.0",
                }
            )

        def drop_shipset_crate(cloned):
            cloned["components"] = [
                component for component in cloned["components"] if component["name"] != "askama"
            ]

        def wrong_digest(cloned):
            root = cloned["metadata"]["component"]
            root["purl"] = root["purl"].replace(
                "sha256%3A" + DIGEST.split(":", 1)[1], "sha256%3A" + OTHER_DIGEST.split(":", 1)[1]
            )
            root["bom-ref"] = root["purl"]

        cases = {
            "root-component": wrong_root,
            "github-reference": github_reference,
            "component-type": file_component,
            "banned-component": banned_component,
            "missing-shipset-component": drop_shipset_crate,
            "digest-mismatch": wrong_digest,
        }
        for expected, mutate in cases.items():
            with self.subTest(check=expected):
                observed = codes(mutate)
                self.assertIn(expected, observed, "expected %s in %s" % (expected, observed))
                self.assertIn("component-count", observed)

    def test_duplicate_bom_ref_across_sources_fails(self):
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
        image_path = Path(self.scratch) / "image-duplicate.json"
        image_path.write_text(json.dumps(image), encoding="utf-8")
        args = list(merge_args(str(Path(self.scratch) / "out.json")))
        args[args.index("--image-bom") + 1] = str(image_path)
        proc = run_tool(*args)
        self.assertEqual(proc.returncode, 1, proc.stdout)
        self.assertIn("::error::duplicate-bom-ref:", proc.stdout)
        self.assertIn(ASKAMA_REF, proc.stdout)


class ShipsetTest(unittest.TestCase):
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
        self.assertIn(
            "excluded-components: askama_derive@0.16.1, serde_derive@1.0.229",
            proc.stderr,
        )


if __name__ == "__main__":
    unittest.main()
