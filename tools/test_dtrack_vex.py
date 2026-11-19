#!/usr/bin/env python3
"""Regression suite for the repository-held VEX seat.

Run: python3 -B tools/test_dtrack_vex.py [--shipset <path>]

Checks `crates/gh-report/dtrack-vex.json` (CycloneDX 1.5 shape, internal
`affects[].ref` resolution, the compile-time-only exclusion rule, uniqueness)
and `.github/workflows/apply-vex.yml` (main-gated dispatch-only delivery, the
closed literal project identity, and the zero-decision notice that keeps an
empty file from reading as applied triage).

`--shipset` names a `tools/sbom_merge.py shipset` output file; when given, every
component must appear in that shipped closure as `name@version`.
"""

import argparse
import copy
import json
import re
import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
VEX_PATH = REPO_ROOT / "crates" / "gh-report" / "dtrack-vex.json"
WORKFLOW_PATH = REPO_ROOT / ".github" / "workflows" / "apply-vex.yml"

ALLOWED_STATES = ("false_positive", "not_affected", "resolved", "exploitable", "in_triage")

# Both advisories are ignored in deny.toml for crates that never reach the
# released inventory: basic-toml sits behind askama_derive's proc-macro, paste
# behind the wasm client's proc-macros. Their acceptance is SEC-0013's.
BANNED_ADVISORIES = {
    "RUSTSEC-2024-0436": "paste, reached through the wasm client's proc-macros",
    "RUSTSEC-2026-0374": "basic-toml, reached through askama_derive's proc-macro",
}
BANNED_RECORD = "deny.toml under SEC-0013"

SHIPSET_PATH = None


def load_json(path):
    with open(str(path), "r", encoding="utf-8") as handle:
        return json.load(handle)


def load_shipset(path):
    with open(str(path), "r", encoding="utf-8") as handle:
        return {
            line.strip()
            for line in handle
            if line.strip() and not line.strip().startswith("#")
        }


def component_coordinates(component):
    name = component.get("name")
    version = component.get("version")
    purl = component.get("purl")
    if isinstance(purl, str) and purl.startswith("pkg:"):
        body = purl[len("pkg:"):].split("?", 1)[0].split("#", 1)[0]
        parts = body.split("/", 1)
        if len(parts) == 2 and "@" in parts[1]:
            purl_name, purl_version = parts[1].rsplit("@", 1)
            if not name:
                name = purl_name
            if not version:
                version = purl_version
    return name, version


def validate(document, shipset=None):
    """Return every shape/reference/exclusion problem; empty means valid."""
    if not isinstance(document, dict):
        return ["document must be a JSON object"]

    problems = []

    if document.get("bomFormat") != "CycloneDX":
        problems.append('bomFormat must be "CycloneDX"')
    if document.get("specVersion") != "1.5":
        problems.append('specVersion must be "1.5"')

    document_version = document.get("version")
    if (
        isinstance(document_version, bool)
        or not isinstance(document_version, int)
        or document_version != 1
    ):
        problems.append("version must be the integer 1")

    metadata = document.get("metadata")
    metadata_component = metadata.get("component") if isinstance(metadata, dict) else None
    if not isinstance(metadata_component, dict):
        problems.append("metadata.component must be an object")
        metadata_component = {}

    metadata_ref = metadata_component.get("bom-ref")
    if not isinstance(metadata_ref, str) or not metadata_ref.strip():
        problems.append("metadata.component.bom-ref must be a non-empty string")
        metadata_ref = None

    metadata_name = metadata_component.get("name")
    if not isinstance(metadata_name, str) or not metadata_name.strip():
        problems.append("metadata.component.name must be a non-empty string")

    if metadata_component.get("type") != "application":
        problems.append('metadata.component.type must be "application"')

    components = document.get("components")
    if not isinstance(components, list):
        problems.append("components must be an array")
        components = []

    component_refs = []
    for index, component in enumerate(components):
        label = "components[%d]" % index
        if not isinstance(component, dict):
            problems.append("%s must be an object" % label)
            continue

        ref = component.get("bom-ref")
        if not isinstance(ref, str) or not ref.strip():
            problems.append("%s.bom-ref must be a non-empty string" % label)
        else:
            component_refs.append(ref)

        purl = component.get("purl")
        if not isinstance(purl, str) or not purl.startswith("pkg:"):
            problems.append(
                "%s (%s) must carry a pkg: purl"
                % (label, component.get("name") or "unnamed")
            )

    for ref in sorted({ref for ref in component_refs if component_refs.count(ref) > 1}):
        problems.append("duplicate components[].bom-ref: %s" % ref)

    known_refs = set(component_refs)
    if metadata_ref is not None:
        known_refs.add(metadata_ref)

    vulnerabilities = document.get("vulnerabilities")
    if not isinstance(vulnerabilities, list):
        problems.append("vulnerabilities must be an array")
        vulnerabilities = []

    for index, vulnerability in enumerate(vulnerabilities):
        label = "vulnerabilities[%d]" % index
        if not isinstance(vulnerability, dict):
            problems.append("%s must be an object" % label)
            continue

        vuln_id = vulnerability.get("id")
        if not isinstance(vuln_id, str) or not vuln_id.strip():
            problems.append("%s.id must be a non-empty string" % label)
            vuln_id = None

        if vuln_id in BANNED_ADVISORIES:
            problems.append(
                "%s.id %s is compile-time-only and stays recorded in %s (%s); "
                "it must not be seeded here, because no released component can "
                "ever match it" % (label, vuln_id, BANNED_RECORD, BANNED_ADVISORIES[vuln_id])
            )

        source = vulnerability.get("source")
        source_name = source.get("name") if isinstance(source, dict) else None
        if not isinstance(source_name, str) or not source_name.strip():
            problems.append("%s.source.name must be a non-empty string" % label)

        analysis = vulnerability.get("analysis")
        state = analysis.get("state") if isinstance(analysis, dict) else None
        if state not in ALLOWED_STATES:
            problems.append(
                "%s.analysis.state must be one of %s, got %r"
                % (label, ", ".join(ALLOWED_STATES), state)
            )

        affects = vulnerability.get("affects")
        if not isinstance(affects, list) or not affects:
            problems.append("%s.affects must be a non-empty array" % label)
            affects = []

        for position, affected in enumerate(affects):
            ref = affected.get("ref") if isinstance(affected, dict) else None
            if not isinstance(ref, str) or not ref.strip():
                problems.append(
                    "%s.affects[%d].ref must be a non-empty string" % (label, position)
                )
            elif ref not in known_refs:
                problems.append(
                    "%s.affects[%d].ref %r does not resolve to a component "
                    "bom-ref or the metadata component" % (label, position, ref)
                )

    if shipset is not None:
        for index, component in enumerate(components):
            if not isinstance(component, dict):
                continue
            name, version = component_coordinates(component)
            if not name or not version:
                problems.append(
                    "components[%d] has no name@version to look up in the shipped closure"
                    % index
                )
                continue
            key = "%s@%s" % (name, version)
            if key not in shipset:
                problems.append(
                    "components[%d] %s is not in the shipped closure" % (index, key)
                )

    return problems


VEX_DOCUMENT = load_json(VEX_PATH)
WORKFLOW_TEXT = WORKFLOW_PATH.read_text(encoding="utf-8")


def seeded_document():
    """The checked-in document plus one valid component and one decision."""
    document = copy.deepcopy(VEX_DOCUMENT)
    document["components"] = [
        {
            "bom-ref": "pkg:cargo/example@1.2.3",
            "type": "library",
            "name": "example",
            "version": "1.2.3",
            "purl": "pkg:cargo/example@1.2.3",
        }
    ]
    document["vulnerabilities"] = [
        {
            "id": "RUSTSEC-0000-0001",
            "source": {"name": "OSV"},
            "analysis": {"state": "not_affected"},
            "affects": [{"ref": "pkg:cargo/example@1.2.3"}],
        }
    ]
    return document


def trigger_block(text):
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        if line == "on:":
            start = index
            break
    if start is None:
        return ""
    block = []
    for line in lines[start + 1:]:
        if line and not line.startswith(" "):
            break
        block.append(line)
    return "\n".join(block)


class CheckedInDocumentTest(unittest.TestCase):
    def test_checked_in_document_is_valid(self):
        shipset = load_shipset(SHIPSET_PATH) if SHIPSET_PATH is not None else None
        self.assertEqual([], validate(VEX_DOCUMENT, shipset))

    def test_no_compile_time_only_advisory_is_seeded(self):
        ids = [
            entry.get("id")
            for entry in VEX_DOCUMENT.get("vulnerabilities") or []
            if isinstance(entry, dict)
        ]
        for advisory in BANNED_ADVISORIES:
            self.assertNotIn(
                advisory,
                ids,
                "%s is compile-time-only; its record is %s" % (advisory, BANNED_RECORD),
            )

    def test_identity_is_the_application_document(self):
        component = VEX_DOCUMENT["metadata"]["component"]
        self.assertEqual("gh-report", component["name"])
        self.assertEqual("application", component["type"])


class NegativeDocumentTest(unittest.TestCase):
    def test_seeded_baseline_is_valid(self):
        self.assertEqual([], validate(seeded_document()))

    def test_dangling_affects_ref_is_rejected(self):
        document = seeded_document()
        document["vulnerabilities"][0]["affects"][0]["ref"] = "pkg:cargo/missing@9.9.9"
        problems = "\n".join(validate(document))
        self.assertIn("does not resolve", problems)

    def test_compile_time_only_advisories_are_rejected_with_their_record(self):
        for advisory in BANNED_ADVISORIES:
            with self.subTest(advisory=advisory):
                document = seeded_document()
                document["vulnerabilities"][0]["id"] = advisory
                problems = "\n".join(validate(document))
                self.assertIn(advisory, problems)
                self.assertIn("deny.toml", problems)
                self.assertIn("SEC-0013", problems)

    def test_purl_less_component_is_rejected(self):
        document = seeded_document()
        del document["components"][0]["purl"]
        problems = "\n".join(validate(document))
        self.assertIn("pkg: purl", problems)

    def test_duplicate_bom_ref_is_rejected(self):
        document = seeded_document()
        document["components"].append(copy.deepcopy(document["components"][0]))
        problems = "\n".join(validate(document))
        self.assertIn("duplicate", problems)

    def test_unknown_analysis_state_is_rejected(self):
        document = seeded_document()
        document["vulnerabilities"][0]["analysis"]["state"] = "ignored"
        problems = "\n".join(validate(document))
        self.assertIn("analysis.state", problems)

    def test_empty_affects_is_rejected(self):
        document = seeded_document()
        document["vulnerabilities"][0]["affects"] = []
        problems = "\n".join(validate(document))
        self.assertIn("affects", problems)


class ShipsetTest(unittest.TestCase):
    def test_component_outside_shipset_is_rejected(self):
        problems = "\n".join(validate(seeded_document(), {"gh-report@0.0.0"}))
        self.assertIn("not in the shipped closure", problems)
        self.assertIn("example@1.2.3", problems)

    def test_component_inside_shipset_is_accepted(self):
        self.assertEqual([], validate(seeded_document(), {"example@1.2.3"}))


class WorkflowTest(unittest.TestCase):
    def test_dispatch_is_the_only_trigger(self):
        block = trigger_block(WORKFLOW_TEXT)
        self.assertIn("workflow_dispatch:", block)
        for trigger in ("push:", "pull_request:", "schedule:", "workflow_call:", "inputs:"):
            self.assertNotIn(trigger, block)

    def test_job_is_gated_to_main(self):
        self.assertIn("if: github.ref == 'refs/heads/main'", WORKFLOW_TEXT)

    def test_runs_in_the_prod_environment(self):
        self.assertIn("environment: prod", WORKFLOW_TEXT)

    def test_checkout_is_pinned_and_does_not_persist_credentials(self):
        self.assertRegex(WORKFLOW_TEXT, r"actions/checkout@[0-9a-f]{40}")
        self.assertIn("persist-credentials: false", WORKFLOW_TEXT)

    def test_posts_to_the_vex_endpoint_with_literal_identity(self):
        self.assertIn('"https://api.dtrack.mattilsynet.io/api/v1/vex"', WORKFLOW_TEXT)
        self.assertIn('-F "projectName=gh-report"', WORKFLOW_TEXT)
        self.assertIn('-F "projectVersion=prod"', WORKFLOW_TEXT)
        self.assertIn('-F "vex=@crates/gh-report/dtrack-vex.json"', WORKFLOW_TEXT)

    def test_decision_count_is_computed_and_printed_before_the_post(self):
        before_post = WORKFLOW_TEXT.split("curl -sS", 1)[0]
        self.assertIn("decisions=", before_post)
        self.assertIn("triage decision(s)", before_post)

    def test_post_is_not_gated_by_the_decision_count(self):
        before_post = WORKFLOW_TEXT.split("curl -sS", 1)[0]
        for early_exit in ("exit 0", "exit 1", "-eq 0", "-ne 0"):
            self.assertNotIn(early_exit, before_post)
        self.assertIn("curl -sS", WORKFLOW_TEXT)

    def test_zero_decision_path_emits_one_explicit_notice(self):
        self.assertEqual(1, WORKFLOW_TEXT.count("::notice::"))
        self.assertIn("contains 0 triage decisions", WORKFLOW_TEXT)
        self.assertIn("applied nothing", WORKFLOW_TEXT)
        self.assertIn("not VEX suppression", WORKFLOW_TEXT)

    def test_success_is_never_phrased_as_triage_applied(self):
        lowered = WORKFLOW_TEXT.lower()
        for claim in (
            "triage applied",
            "triage was applied",
            "applied triage",
            "vex applied",
            "vex was applied",
            "suppression proved",
            "suppression verified",
        ):
            self.assertNotIn(claim, lowered)

    def test_missing_token_is_a_distinct_warning(self):
        self.assertIn("BomUploadResponse token", WORKFLOW_TEXT)
        warning_lines = [
            line for line in WORKFLOW_TEXT.splitlines() if "::warning::" in line
        ]
        self.assertEqual(1, len(warning_lines))
        self.assertIn("token", warning_lines[0])

    def test_response_body_and_token_are_never_printed(self):
        for line in WORKFLOW_TEXT.splitlines():
            stripped = line.strip()
            if stripped.startswith(("echo", "printf")):
                for value in ("$response", "${response}", "$token", "${token}"):
                    self.assertNotIn(
                        value,
                        stripped,
                        "the response body and token must never be printed",
                    )

    def test_status_branches_are_distinct(self):
        for branch in ("200)", "403)", "404)", "*)"):
            self.assertIn(branch, WORKFLOW_TEXT)


def main():
    global SHIPSET_PATH
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--shipset", default=None)
    known, remaining = parser.parse_known_args()
    if known.shipset:
        SHIPSET_PATH = Path(known.shipset).resolve()
    sys.argv = [sys.argv[0]] + remaining
    unittest.main()


if __name__ == "__main__":
    main()
