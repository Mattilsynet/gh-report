#!/usr/bin/env bash
#
# adr-fmt lint ratchet — the CI wrapper AFM-0003:R3 specifies.
#
# AFM-0003:R1/R2 make adr-fmt advisory-only: every finding is
# Severity::Warning and the tool exits 0 for all lint findings, exit 1 only
# on infra failure. R3 places enforcement in a CI wrapper that parses
# "## Diagnostics: N warning(s)" and fails when N exceeds the project
# threshold. "The project threshold" is undefined by any ADR; this wrapper
# defines it in its own surface, docs/adr/.lint-threshold, rather than
# amending the corpus.
#
# NON-BLOCKING BY CONSTRUCTION: this script is wired only into the
# schedule-only Scheduled workflow, which carries no push/pull_request/
# workflow_call trigger. Per RST-0007:R1 a merge gate is "a build-time,
# merge-blocking CI check" — conjunctive — so this sits outside RST-0007.
# Making it a required context would trigger RST-0007:R5, which requires
# AFM-0003 to carry a COM-0017:R4 statement naming the job by id. It has
# none. Do NOT add this job to branch protection.
#
# Raising the threshold must be its own commit with a stated reason
# (COM-0035:R5 weakening discipline), otherwise the ratchet degrades into a
# rubber stamp.

set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

threshold_file="docs/adr/.lint-threshold"

if [ ! -r "$threshold_file" ]; then
  echo "::error::missing threshold file $threshold_file (AFM-0003:R3 wrapper)"
  exit 1
fi

threshold=$(tr -d '[:space:]' < "$threshold_file")

case "$threshold" in
  '' | *[!0-9]*)
    echo "::error::threshold in $threshold_file is not a non-negative integer: '$threshold'"
    exit 1
    ;;
esac

if ! out=$(adr-fmt --lint); then
  echo "::error::adr-fmt --lint failed to run (infra failure, AFM-0003:R1)"
  printf '%s\n' "$out"
  exit 1
fi

n=$(printf '%s\n' "$out" | sed -n 's/^## Diagnostics: \([0-9]\{1,\}\) warning(s).*/\1/p' | head -n 1)

if [ -z "$n" ]; then
  echo "::error::could not parse '## Diagnostics: N warning(s)' from adr-fmt --lint output (infra failure, AFM-0003:R1/R3)"
  printf '%s\n' "$out"
  exit 1
fi

echo "adr-fmt lint: $n warning(s), threshold $threshold"

if [ "$n" -gt "$threshold" ]; then
  echo "::error::adr-fmt lint warnings rose $threshold -> $n (AFM-0003:R3 wrapper threshold). Fix the new findings, or raise $threshold_file in its own commit with a stated reason."
  printf '%s\n' "$out"
  exit 1
fi

if [ "$n" -lt "$threshold" ]; then
  echo "::notice::adr-fmt lint warnings fell $threshold -> $n; ratchet $threshold_file down to $n in its own commit."
fi

exit 0
