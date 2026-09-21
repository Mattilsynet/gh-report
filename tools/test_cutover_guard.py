"""Local cutover guard regression: python3.12 -B tools/test_cutover_guard.py.

Uses a mocked command runner; performs no cloud reads or writes.
"""

import json
from pathlib import Path
import subprocess
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from cutover_guard import (  # noqa: E402
    GuardError, generation, main, plan, read_live_traffic, update_argv,
)
from workflow_subset import read_workflow  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github" / "workflows" / "cutover.yml"
AGENTS = ROOT / "AGENTS.md"
LOCAL_GATE = "python3.12 -B tools/test_cutover_guard.py"

TARGET = "ghreport-00311-fkq"
CURRENT = "ghreport-00310-wzb"
FOREIGN = "ghreport-00312-hhd"


def traffic(*entries, gen=7, observed=7, service_ready="True"):
    status = {"traffic": list(entries), "observedGeneration": observed,
              "conditions": [{"type": "Ready", "status": service_ready}]}
    if observed is None:
        del status["observedGeneration"]
    return json.dumps({"metadata": {"generation": gen}, "status": status})


def share(name, percent):
    return {"revisionName": name, "percent": percent}


def tagged(name, tag="v24-candidate"):
    return {"revisionName": name, "tag": tag,
            "url": f"https://{tag}---ghreport.example.run.app"}


def ready(state="True"):
    return json.dumps({"status": {"conditions": [
        {"type": "Active", "status": "True"},
        {"type": "Ready", "status": state},
    ]}})


BASELINE = traffic(share(CURRENT, 100))


class Runner:
    """Records argv; replays scripted describe payloads."""

    def __init__(self, describes, readies=None, update_returncode=0):
        self.describes = list(describes)
        self.readies = list(readies) if readies is not None else [ready()] * 4
        self.update_returncode = update_returncode
        self.calls = []

    def __call__(self, argv):
        self.calls.append(argv)
        if "describe" in argv:
            source = self.readies if "revisions" in argv else self.describes
            stdout = source.pop(0)
            code = 1 if stdout is None else 0
            return subprocess.CompletedProcess(argv, code, stdout or "", "")
        return subprocess.CompletedProcess(argv, self.update_returncode, "", "")

    @property
    def updates(self):
        return [argv for argv in self.calls if "update-traffic" in argv]


def env(revision=TARGET, percent="100", current=CURRENT):
    return {
        "CUTOVER_REVISION": revision,
        "CUTOVER_PERCENT": percent,
        "CUTOVER_EXPECTED_CURRENT": current,
    }


class UpdateCommand(unittest.TestCase):
    def test_full_cutover_command(self):
        runner = Runner([BASELINE] * 2)
        self.assertEqual(main(env(), runner), 0)
        self.assertEqual(runner.updates, [[
            "gcloud", "run", "services", "update-traffic", "ghreport",
            "--region", "europe-north1", "--project", "ghreport-d302",
            "--to-revisions", f"{TARGET}=100",
        ]])

    def test_partial_remainder_uses_expected_current(self):
        runner = Runner([BASELINE] * 2)
        self.assertEqual(main(env(percent="25"), runner), 0)
        self.assertEqual(runner.updates[0][-1], f"{TARGET}=25,{CURRENT}=75")

    def test_update_argv_never_embeds_shell_syntax(self):
        argv = update_argv(TARGET, 10, CURRENT)
        self.assertTrue(all(isinstance(part, str) for part in argv))
        self.assertFalse(any(char in part for part in argv for char in ";|&$`"))

    def test_failed_update_reports_nonzero(self):
        runner = Runner([BASELINE] * 2, update_returncode=1)
        self.assertEqual(main(env(), runner), 1)


class TagOnlyEntries(unittest.TestCase):
    """Cloud Run lists tagged zero-traffic revisions without a percent key."""

    def accept(self, payload, percent="25"):
        runner = Runner([payload] * 2)
        self.assertEqual(main(env(percent=percent), runner), 0)
        return runner

    def test_tag_only_candidate_entry_is_normalized_to_zero(self):
        payload = traffic(share(CURRENT, 100), tagged(TARGET))
        runner = self.accept(payload)
        self.assertEqual(runner.updates[0][-1], f"{TARGET}=25,{CURRENT}=75")

    def test_tag_only_entry_for_unrelated_revision(self):
        self.accept(traffic(share(CURRENT, 100), tagged(FOREIGN, "v23-old")))

    def test_explicit_zero_percent_entry_is_not_serving(self):
        self.accept(traffic(share(CURRENT, 100), share(FOREIGN, 0)))

    def test_duplicate_entry_with_one_zero_share_is_accepted(self):
        self.accept(traffic(share(CURRENT, 100), tagged(CURRENT)))


class Rejections(unittest.TestCase):
    def reject(self, environ, describes, readies=None):
        runner = Runner(describes, readies)
        self.assertEqual(main(environ, runner), 1)
        self.assertEqual(runner.updates, [], "no cloud write on rejection")

    def test_malformed_percent(self):
        for bad in ("101", "-1", "50.5", "", "1e2", "50; rm -rf /"):
            with self.subTest(bad=bad):
                self.reject(env(percent=bad), [BASELINE] * 2)

    def test_malformed_revision(self):
        for bad in ("", "Ghreport-1", "rev;whoami", "$(id)", "a" * 64):
            with self.subTest(bad=bad):
                self.reject(env(revision=bad), [BASELINE] * 2)

    def test_malformed_expected_current(self):
        self.reject(env(current=f"{CURRENT}`id`"), [BASELINE] * 2)

    def test_zero_percent_is_rejected(self):
        self.reject(env(percent="0"), [BASELINE] * 2)

    def test_split_against_same_revision_rejected(self):
        self.reject(env(revision=CURRENT, percent="25"), [BASELINE] * 2)

    def test_live_traffic_differs_from_expected_current(self):
        self.reject(env(percent="25"), [traffic(share(FOREIGN, 100))] * 2)

    def test_live_traffic_ambiguous_split(self):
        payload = traffic(share(CURRENT, 60), share(FOREIGN, 40))
        self.reject(env(percent="25"), [payload] * 2)

    def test_conflicting_duplicate_entries(self):
        payload = traffic(share(CURRENT, 100), share(CURRENT, 40))
        self.reject(env(percent="25"), [payload] * 2)

    def test_percent_out_of_range(self):
        for bad in (-1, 101, 1000):
            with self.subTest(bad=bad):
                payload = traffic(share(CURRENT, 100), share(FOREIGN, bad))
                self.reject(env(percent="25"), [payload] * 2)

    def test_percent_boolean_is_not_an_integer(self):
        payload = traffic(share(CURRENT, 100), share(FOREIGN, True))
        with self.assertRaisesRegex(GuardError, "percent unreadable"):
            read_live_traffic(Runner([payload]))
        self.reject(env(percent="25"), [payload] * 2)

    def test_percent_wrong_type(self):
        for bad in ("100", 12.5, None, [100]):
            with self.subTest(bad=bad):
                payload = traffic(share(CURRENT, bad))
                self.reject(env(), [payload] * 2)

    def test_entry_missing_revision_name(self):
        payload = traffic(share(CURRENT, 100), {"percent": 0})
        self.reject(env(), [payload] * 2)

    def test_malformed_root_and_status_raise_guard_error(self):
        for payload in ("[]", "null", '"text"', "5", '{"status": []}',
                        '{"status": {"traffic": {}}}', '{"other": 1}'):
            with self.subTest(payload=payload):
                self.reject(env(), [payload] * 2)

    def test_malformed_entry_shape(self):
        self.reject(env(), ['{"status": {"traffic": ["ghreport"]}}'] * 2)

    def test_live_traffic_read_failure(self):
        self.reject(env(), [None, None])

    def test_live_traffic_unparsable(self):
        self.reject(env(), ["not json", "not json"])

    def test_live_traffic_empty(self):
        self.reject(env(), ['{"status": {"traffic": []}}'] * 2)

    def test_recheck_catches_concurrent_foreign_rollout(self):
        runner = Runner([BASELINE, traffic(share(FOREIGN, 100))])
        self.assertEqual(main(env(percent="25"), runner), 1)
        self.assertEqual(runner.updates, [])

    def test_plan_raises_guard_error(self):
        with self.assertRaises(GuardError):
            plan(env(percent="999"), Runner([]))


class ServiceSettled(unittest.TestCase):
    """An already-pending foreign rollout must not be overwritten."""

    def reject(self, payloads, percent="25"):
        runner = Runner(payloads)
        self.assertEqual(main(env(percent=percent), runner), 1)
        self.assertEqual(runner.updates, [], "no cloud write on rejection")

    def settled(self, **kwargs):
        return traffic(share(CURRENT, 100), **kwargs)

    def test_settled_service_is_accepted(self):
        runner = Runner([self.settled(gen=12, observed=12)] * 2)
        self.assertEqual(main(env(percent="25"), runner), 0)

    def test_string_generations_compare_numerically(self):
        runner = Runner([self.settled(gen="12", observed="12")] * 2)
        self.assertEqual(main(env(percent="25"), runner), 0)
        self.assertEqual(generation("12", "f"), generation(12, "f"))

    def test_pending_rollout_rejected(self):
        self.reject([self.settled(gen=13, observed=12)] * 2)

    def test_observed_ahead_of_generation_rejected(self):
        self.reject([self.settled(gen=12, observed=13)] * 2)

    def test_pending_rollout_detected_on_recheck_only(self):
        runner = Runner([self.settled(gen=12, observed=12),
                         self.settled(gen=13, observed=12)])
        self.assertEqual(main(env(percent="25"), runner), 1)
        self.assertEqual(runner.updates, [])

    def test_observed_generation_absent(self):
        self.reject([self.settled(observed=None)] * 2)

    def test_metadata_generation_absent(self):
        payload = json.dumps({"status": {
            "traffic": [share(CURRENT, 100)], "observedGeneration": 7,
            "conditions": [{"type": "Ready", "status": "True"}]}})
        self.reject([payload] * 2)

    def test_metadata_not_an_object(self):
        payload = json.dumps({"metadata": [], "status": {
            "traffic": [share(CURRENT, 100)], "observedGeneration": 7,
            "conditions": [{"type": "Ready", "status": "True"}]}})
        self.reject([payload] * 2)

    def test_generation_scalar_types_rejected(self):
        for bad in (True, 7.5, "seven", None, [7], {"v": 7}, ""):
            with self.subTest(bad=bad):
                self.reject([self.settled(gen=bad)] * 2)
                self.reject([self.settled(observed=bad)] * 2)

    def test_service_not_ready(self):
        for state in ("False", "Unknown", None, ""):
            with self.subTest(state=state):
                self.reject([self.settled(service_ready=state)] * 2)

    def test_service_ready_condition_missing_or_ambiguous(self):
        for conditions in ([], [{"type": "Active", "status": "True"}],
                           [{"type": "Ready", "status": "True"}] * 2, {}):
            with self.subTest(conditions=conditions):
                payload = json.dumps({"metadata": {"generation": 7}, "status": {
                    "traffic": [share(CURRENT, 100)], "observedGeneration": 7,
                    "conditions": conditions}})
                self.reject([payload] * 2)


class TargetReadiness(unittest.TestCase):
    def reject(self, readies):
        runner = Runner([BASELINE] * 2, readies)
        self.assertEqual(main(env(), runner), 1)
        self.assertEqual(runner.updates, [])

    def test_target_not_ready(self):
        self.reject([ready("False")] * 2)

    def test_target_ready_unknown(self):
        self.reject([ready("Unknown")] * 2)

    def test_revision_status_read_failure(self):
        self.reject([None, None])

    def test_revision_status_unparsable(self):
        self.reject(["not json"] * 2)

    def test_revision_status_malformed(self):
        for payload in ("[]", '{"status": {}}', '{"status": {"conditions": {}}}',
                        '{"status": {"conditions": []}}'):
            with self.subTest(payload=payload):
                self.reject([payload] * 2)

    def test_readiness_is_checked_before_any_update(self):
        runner = Runner([BASELINE] * 2, [ready("False")] * 2)
        main(env(), runner)
        self.assertTrue(any("revisions" in argv for argv in runner.calls))


class WorkflowWiring(unittest.TestCase):
    def setUp(self):
        self.workflow = read_workflow(WORKFLOW)
        self.job = self.workflow["jobs"]["cutover"]
        self.steps = self.job["steps"]

    def index_of(self, predicate, label):
        matches = [i for i, step in enumerate(self.steps) if predicate(step)]
        self.assertEqual(len(matches), 1, label)
        return matches[0]

    def test_expected_current_input_declared(self):
        inputs = self.workflow["on"]["workflow_dispatch"]["inputs"]
        self.assertIn("expected_current_revision", inputs)
        self.assertEqual(inputs["expected_current_revision"]["required"], "true")
        for name in ("revision", "percent", "expected_current_revision"):
            self.assertNotIn("default", inputs[name], f"{name} must be explicit")

    def test_no_hardcoded_historical_revision(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        for stale in ("ghreport-00101-bzs", "ghreport-00102-2ln"):
            self.assertNotIn(stale, text)

    def test_inputs_never_interpolated_into_run_scripts(self):
        for step in self.steps:
            self.assertNotIn("${{ inputs.", step.get("run", ""))

    def test_guard_is_the_only_traffic_mutator(self):
        runs = [step.get("run", "") for step in self.steps]
        self.assertFalse(any("update-traffic" in run for run in runs))
        self.assertTrue(any("tools/cutover_guard.py" in run for run in runs))

    def test_execution_is_serialized(self):
        self.assertIn("concurrency", self.workflow)
        self.assertEqual(self.workflow["concurrency"]["cancel-in-progress"], "false")

    def test_guard_tests_run_before_auth_and_mutation(self):
        selftest = self.index_of(
            lambda step: "tools/test_cutover_guard.py" in step.get("run", ""),
            "guard self-test step",
        )
        auth = self.index_of(
            lambda step: "google-github-actions/auth" in step.get("uses", ""),
            "auth step",
        )
        mutate = self.index_of(
            lambda step: "tools/cutover_guard.py" in step.get("run", "")
            and "test_cutover_guard.py" not in step.get("run", ""),
            "guard mutation step",
        )
        checkout = self.index_of(
            lambda step: "actions/checkout" in step.get("uses", ""), "checkout step"
        )
        self.assertLess(checkout, selftest)
        self.assertLess(selftest, auth)
        self.assertLess(auth, mutate)


class LocalGateDocumented(unittest.TestCase):
    def test_agents_documents_local_entry_point(self):
        self.assertIn(LOCAL_GATE, AGENTS.read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main(verbosity=2)
