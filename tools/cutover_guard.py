"""Fail-closed Cloud Run traffic cutover guard.

Local test entry point: python3.12 -B tools/test_cutover_guard.py

Reads untrusted workflow inputs from the environment (never from shell
interpolation), validates them, verifies the live traffic shape matches the
caller-supplied expected current revision, and only then emits the exact
`gcloud run services update-traffic` argument vector. Every command is run
through an injected runner so tests never reach Google Cloud.

This is a precondition recheck, not a global compare-and-swap: a foreign
rollout landing between the recheck and the update is still possible, and the
guard only narrows that window. It does refuse to act on an unsettled service:
a read whose status.observedGeneration does not equal metadata.generation, or
whose service Ready condition is not True, is rejected on both the initial read
and the immediate recheck, so an already-pending foreign rollout is not
overwritten. A settled service says the control plane has observed the current
desired spec; it says nothing about any individual revision, which is immutable
and does not reconcile toward a new spec.

Tag-only traffic entries (a tagged revision with a URL and no percent) are
normalized to 0 and are expected. A percent of 0 as workflow input is rejected
outright rather than silently skipped.
"""

import json
import os
import re
import subprocess
import sys


SERVICE = "ghreport"
REGION = "europe-north1"
PROJECT = "ghreport-d302"

REVISION_PATTERN = re.compile(r"\A[a-z](?:[-a-z0-9]{0,61}[a-z0-9])?\Z")
PERCENT_PATTERN = re.compile(r"\A(?:0|[1-9][0-9]?|100)\Z")


class GuardError(Exception):
    """Rejection: input malformed, or live traffic changed or ambiguous."""


def parse_revision(name, field):
    if not REVISION_PATTERN.match(name or ""):
        raise GuardError(f"{field}: malformed Cloud Run revision name {name!r}")
    return name


def parse_percent(raw):
    if not PERCENT_PATTERN.match(raw or ""):
        raise GuardError(f"percent: expected integer 0..100, got {raw!r}")
    return int(raw)


def describe_argv():
    return [
        "gcloud", "run", "services", "describe", SERVICE,
        "--region", REGION, "--project", PROJECT,
        "--format", "json(metadata.generation,status.observedGeneration,"
                    "status.traffic,status.conditions)",
    ]


def generation(value, field):
    if isinstance(value, bool):
        raise GuardError(f"{field}: not a generation scalar")
    if isinstance(value, int):
        return value
    if isinstance(value, str) and value.isdigit():
        return int(value)
    raise GuardError(f"{field}: missing or not a generation scalar")


def ready_condition(conditions, subject):
    if not isinstance(conditions, list):
        raise GuardError(f"{subject} conditions missing or unreadable")
    ready = [c for c in conditions
             if isinstance(c, dict) and c.get("type") == "Ready"]
    if len(ready) != 1:
        raise GuardError(f"{subject} Ready condition ambiguous")
    state = ready[0].get("status")
    if state != "True":
        raise GuardError(f"{subject} is not Ready (status={state!r})")


def assert_service_settled(payload, status):
    desired = generation(
        (payload.get("metadata") or {}).get("generation")
        if isinstance(payload.get("metadata"), dict) else None,
        "metadata.generation",
    )
    observed = generation(status.get("observedGeneration"),
                          "status.observedGeneration")
    if observed != desired:
        raise GuardError(
            "service rollout pending: observedGeneration "
            f"{observed} != generation {desired}"
        )
    ready_condition(status.get("conditions"), "service")


def update_argv(revision, percent, expected_current):
    if percent == 100:
        assignments = f"{revision}=100"
    else:
        assignments = f"{revision}={percent},{expected_current}={100 - percent}"
    return [
        "gcloud", "run", "services", "update-traffic", SERVICE,
        "--region", REGION, "--project", PROJECT,
        "--to-revisions", assignments,
    ]


def read_live_traffic(run):
    completed = run(describe_argv())
    if completed.returncode != 0:
        raise GuardError("live traffic read failed; refusing to shift traffic")
    try:
        payload = json.loads(completed.stdout)
    except (TypeError, ValueError) as error:
        raise GuardError(f"live traffic payload unreadable: {error}") from error
    if not isinstance(payload, dict):
        raise GuardError("live traffic payload is not an object")
    status = payload.get("status")
    if not isinstance(status, dict):
        raise GuardError("live traffic status missing or unreadable")
    entries = status.get("traffic")
    if not isinstance(entries, list) or not entries:
        raise GuardError("live traffic absent; refusing to shift traffic")
    assert_service_settled(payload, status)
    shares = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise GuardError("live traffic entry unreadable")
        revision = entry.get("revisionName")
        if not isinstance(revision, str):
            raise GuardError("live traffic entry missing revisionName")
        percent = entry_percent(entry, revision)
        previous = shares.get(revision)
        if previous is not None and previous != 0 and percent != 0:
            raise GuardError(
                f"live traffic ambiguous: conflicting entries for {revision}"
            )
        shares[revision] = percent if previous in (None, 0) else previous
    return shares


def entry_percent(entry, revision):
    """Tag-only entries carry no percent and serve no traffic."""
    if "percent" not in entry:
        return 0
    percent = entry["percent"]
    if isinstance(percent, bool) or not isinstance(percent, int):
        raise GuardError(f"live traffic percent unreadable for {revision}")
    if not 0 <= percent <= 100:
        raise GuardError(f"live traffic percent out of range for {revision}")
    return percent


def assert_expected_current(shares, expected_current):
    serving = {name: pct for name, pct in shares.items() if pct > 0}
    if serving != {expected_current: 100}:
        raise GuardError(
            "live traffic changed or ambiguous: expected "
            f"{{{expected_current!r}: 100}}, found {serving!r}"
        )


def run_subprocess(argv):
    return subprocess.run(argv, capture_output=True, text=True, check=False)


def revision_ready_argv(revision):
    return [
        "gcloud", "run", "revisions", "describe", revision,
        "--region", REGION, "--project", PROJECT,
        "--format", "json(status.conditions)",
    ]


def assert_revision_ready(run, revision):
    """Reject a target revision that is not observed Ready=True.

    A Cloud Run revision is immutable and does not reconcile toward a new
    desired spec; this only confirms the revision itself came up healthy.
    Service-level settledness is checked separately by assert_service_settled.
    """
    completed = run(revision_ready_argv(revision))
    if completed.returncode != 0:
        raise GuardError(f"revision status read failed for {revision}")
    try:
        payload = json.loads(completed.stdout)
    except (TypeError, ValueError) as error:
        raise GuardError(f"revision status unreadable: {error}") from error
    if not isinstance(payload, dict) or not isinstance(payload.get("status"), dict):
        raise GuardError(f"revision status malformed for {revision}")
    ready_condition(payload["status"].get("conditions"), f"revision {revision}")


def plan(env, run):
    revision = parse_revision(env.get("CUTOVER_REVISION"), "revision")
    percent = parse_percent(env.get("CUTOVER_PERCENT"))
    expected_current = parse_revision(
        env.get("CUTOVER_EXPECTED_CURRENT"), "expected_current_revision"
    )
    if percent == 0:
        raise GuardError("percent: 0 shifts no traffic; nothing to do")
    if percent < 100 and revision == expected_current:
        raise GuardError(
            "expected_current_revision must differ from revision for a split"
        )
    assert_expected_current(read_live_traffic(run), expected_current)
    assert_revision_ready(run, revision)
    return update_argv(revision, percent, expected_current)


def main(env=None, run=None):
    env = os.environ if env is None else env
    run = run_subprocess if run is None else run
    try:
        argv = plan(env, run)
        assert_expected_current(
            read_live_traffic(run), parse_revision(
                env.get("CUTOVER_EXPECTED_CURRENT"), "expected_current_revision"
            )
        )
    except GuardError as error:
        print(f"cutover guard rejected: {error}", file=sys.stderr)
        return 1
    completed = run(argv)
    print(" ".join(argv))
    if completed.returncode != 0:
        print("update-traffic failed", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
