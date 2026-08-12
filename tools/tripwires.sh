#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

CHECKS=(projection-lock async-trait pardosa-dep fence-converge dead-code-suppression non-exhaustive)

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
    | grep -v '^crates/gh-report/src/app/state.rs:' || true)
  if [ -n "$offenders" ]; then
    echo "::error::raw .projection_state.lock( outside crates/gh-report/src/app/state.rs"
    echo "::error::use AppState::lock_projection() (state.rs:300-316); COM-0018 + CHE-0048:R7 chokepoint"
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
    body=$(sed -n "${from},${to}p" "$FILE")
    if ! printf '%s\n' "$body" | grep -q 'FencedConflict'; then
      echo "::error::$label ($from-$to) no longer detects FencedConflict — tripwire assumption stale, re-check CHE-0088 amendment"
      return 1
    fi
    if ! printf '%s\n' "$body" | grep -q -E 'rearm_fenced_run\(|rearm_fenced_team_refresh_tick\('; then
      echo "::error::$label ($from-$to) detects FencedConflict without routing through the sanctioned converge sink"
      echo "::error::use rearm_fenced_run(...)/rearm_fenced_team_refresh_tick(...) -> converge_on_fence; CHE-0088 amendment (adr-fmt-3jptm)"
      return 1
    fi
  }
  start1=$(grep -n '^fn spawn_collection_loop' "$FILE" | cut -d: -f1)
  end1=$(awk -v s="$start1" 'NR>s && /^(async )?fn /{print NR; exit}' "$FILE")
  start2=$(grep -n '^fn spawn_team_refresh_loop' "$FILE" | cut -d: -f1)
  end2=$(awk -v s="$start2" 'NR>s && /^(async )?fn /{print NR; exit}' "$FILE")
  check_window "$start1" "$end1" "spawn_collection_loop"
  check_window "$start2" "$end2" "spawn_team_refresh_loop"
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

run_check() {
  case "$1" in
    projection-lock) check_projection_lock ;;
    async-trait) check_async_trait ;;
    pardosa-dep) check_pardosa_dep ;;
    fence-converge) check_fence_converge ;;
    dead-code-suppression) check_dead_code_suppression ;;
    non-exhaustive) check_non_exhaustive ;;
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
