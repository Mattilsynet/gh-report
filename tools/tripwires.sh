#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

CHECKS=(projection-lock async-trait pardosa-dep fence-converge dead-code-suppression non-exhaustive gate-citation)

usage() {
  echo "usage: tools/tripwires.sh <check>|all|--list"
  echo "checks:"
  for c in "${CHECKS[@]}"; do
    echo "  $c"
  done
}

check_projection_lock() {
  offenders=$(grep -RIn --include='*.rs' -E '\.projection_state\.lock\(' \
    crates/gh-report/src \
    | grep -v '^crates/gh-report/src/app/state/mod\.rs:' || true)
  if [ -n "$offenders" ]; then
    echo "::error::raw .projection_state.lock( outside crates/gh-report/src/app/state/mod.rs"
    echo "::error::use AppState::lock_projection() (state/mod.rs); COM-0018 + CHE-0048:R7 chokepoint"
    echo "$offenders"
    return 1
  fi
}

check_async_trait() {
  fail=0
  for c in cherry-pit-core cherry-pit-gateway cherry-pit-web cherry-pit-agent \
           cherry-pit-projection cherry-pit-wq cherry-pit-storage cherry-pit-sd-viz; do
    if cargo tree -p "$c" -e features 2>&1 | grep -q async-trait; then
      echo "::error::$c transitively depends on async-trait (CHE-0025:R1+R2)"
      fail=1
    fi
  done
  return $fail
}

check_pardosa_dep() {
  if cargo tree -p cherry-pit-sd-viz 2>&1 | grep -q pardosa; then
    echo "::error::cherry-pit-sd-viz depends on a pardosa* crate (CHE-0029/CHE-0084:R5)"
    return 1
  fi
}

check_fence_converge() {
  FILE=crates/gh-report/src/app/daemon.rs
  check_window() {
    from=$1; to=$2; label=$3
    awk -v from="$from" -v to="$to" -v label="$label" '
      NR < from || NR > to { next }
      {
        ln = $0
        opens = gsub(/{/, "{", ln)
        closes = gsub(/}/, "}", ln)
        depth_before[NR] = depth
        depth += opens - closes
        depth_after[NR] = depth
        line[NR] = $0
      }
      END {
        n = 0
        for (i = from; i <= to; i++) {
          if (line[i] ~ /FencedConflict/) { n++; trig[n] = i }
        }
        if (n == 0) {
          printf("::error::%s (%d-%d) no longer detects FencedConflict — tripwire assumption stale, re-check CHE-0088 amendment\n", label, from, to)
          exit 1
        }
        unpaired = 0
        for (k = 1; k <= n; k++) {
          t = trig[k]
          db = depth_before[t]
          opened = 0
          endline = to
          for (i = t; i <= to; i++) {
            if (depth_after[i] > db) opened = 1
            if (opened && depth_after[i] <= db) { endline = i; break }
          }
          paired = 0
          for (i = t; i <= endline; i++) {
            if (line[i] ~ /rearm_fenced_run\(|rearm_fenced_team_refresh_tick\(/) { paired = 1; break }
          }
          if (!paired) {
            unpaired++
            printf("::error::%s FencedConflict arm at line %d has no paired rearm_fenced_run(...)/rearm_fenced_team_refresh_tick(...) call in its own arm scope\n", label, t)
            printf("::error::use rearm_fenced_run(...)/rearm_fenced_team_refresh_tick(...) -> converge_on_fence for this specific arm; CHE-0088 amendment (adr-fmt-3jptm)\n")
          }
        }
        exit (unpaired > 0)
      }
    ' "$FILE"
  }
  start1=$(grep -n '^fn spawn_collection_loop' "$FILE" | cut -d: -f1)
  end1=$(awk -v s="$start1" 'NR>s && /^(async )?fn /{print NR; exit}' "$FILE")
  start2=$(grep -n '^fn spawn_team_refresh_loop' "$FILE" | cut -d: -f1)
  end2=$(awk -v s="$start2" 'NR>s && /^(async )?fn /{print NR; exit}' "$FILE")
  fail=0
  check_window "$start1" "$end1" "spawn_collection_loop" || fail=1
  check_window "$start2" "$end2" "spawn_team_refresh_loop" || fail=1
  return $fail
}

check_dead_code_suppression() {
  offenders=$(grep -RIn --include='*.rs' -E \
    '#!\[.*(allow|expect)\(.*\bdead_code\b' \
    crates/*/src || true)
  if [ -n "$offenders" ]; then
    echo "::error::crate/module-level inner dead_code suppression is banned in crates/*/src"
    echo "::error::use a targeted OUTER item-level #[expect(dead_code, reason=\"...\")] instead, or delete/cfg-gate the dead item"
    echo "$offenders"
    return 1
  fi
}

check_non_exhaustive() {
  if ! cargo run -p non-exhaustive-check --quiet -- "$ROOT"; then
    echo "::error::missing #[non_exhaustive] on a library error enum (RST-0006:R1+R3)"
    return 1
  fi
}

# RST-0007:R7 — mechanizes RST-0007:R2's id-existence half: every ADR
# rule id cited in a merge-gate step name: or ::error:: string must
# name an ADR that exists (non-stale) and a rule id present in it.
# The invariant-match half (does the cited rule's text actually state
# the invariant) is not mechanically decidable and stays code-review
# tier per RST-0007:R7.
check_gate_citation() {
  local fail=0
  local tokens
  tokens=$( { grep -hoE -- '- name:.*' .github/workflows/ci-reusable.yml || true; \
              grep -hoE '::error::.*' .github/workflows/ci-reusable.yml tools/tripwires.sh || true; } \
            | grep -oE '[A-Z]{2,4}-[0-9]{4}:R[0-9]+(\+R[0-9]+)*' | sort -u )
  while IFS= read -r tok; do
    [ -z "$tok" ] && continue
    adr_id="${tok%%:*}"
    rules="${tok#*:}"
    adr_file=$(find docs/adr -mindepth 2 -maxdepth 2 -name "${adr_id}-*.md" -not -path 'docs/adr/stale/*' -print -quit)
    if [ -z "$adr_file" ]; then
      echo "::error::gate-citation: ${tok} cites ADR ${adr_id}, which does not exist in docs/adr/ (RST-0007:R7)"
      fail=1
      continue
    fi
    IFS='+' read -ra rule_list <<< "$rules"
    for r in "${rule_list[@]}"; do
      if ! grep -qE "^${r} \[" "$adr_file"; then
        echo "::error::gate-citation: ${tok} cites rule ${r}, not present in ${adr_file} (RST-0007:R7)"
        fail=1
      fi
    done
  done <<< "$tokens"
  return $fail
}

run_check() {
  case "$1" in
    projection-lock) check_projection_lock ;;
    async-trait) check_async_trait ;;
    pardosa-dep) check_pardosa_dep ;;
    fence-converge) check_fence_converge ;;
    dead-code-suppression) check_dead_code_suppression ;;
    non-exhaustive) check_non_exhaustive ;;
    gate-citation) check_gate_citation ;;
    *)
      echo "unknown check: $1" >&2
      usage >&2
      return 2
      ;;
  esac
}

main() {
  if [ "$#" -ne 1 ]; then
    usage >&2
    exit 2
  fi

  case "$1" in
    --list)
      usage
      exit 0
      ;;
    all)
      status=0
      for c in "${CHECKS[@]}"; do
        if ! run_check "$c"; then
          status=1
        fi
      done
      exit $status
      ;;
    *)
      run_check "$1"
      ;;
  esac
}

main "$@"
