#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

CHECKS=(projection-lock async-trait pardosa-dep fence-converge dead-code-suppression non-exhaustive gate-citation adr-number-collision deny-ignore-lifecycle forbid-unsafe-total)

# Activated (ghr-y4hkd discharged, ghr-zcr7c/ghr-swxy8): deny.toml now
# satisfies SEC-0013:R3 (table-form ignores) as of commit be14235; the
# check is safe to run on every PR.
#
# forbid-unsafe-total activated (ghr-5rewy): the two previously uncovered
# crate roots now carry #![forbid(unsafe_code)], so RST-0005:R1 coverage
# is total across every workspace compilation root.
PENDING_CHECKS=()

usage() {
  echo "usage: tools/tripwires.sh <check>|all|--list"
  echo "checks:"
  for c in "${CHECKS[@]}"; do
    echo "  $c"
  done
  if [ "${#PENDING_CHECKS[@]}" -gt 0 ]; then
    echo "pending activation (not in all):"
    for c in "${PENDING_CHECKS[@]}"; do
      echo "  $c"
    done
  fi
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
            printf("::error::use rearm_fenced_run(...)/rearm_fenced_team_refresh_tick(...) -> converge_on_fence for this specific arm; CHE-0088 amendment (ghr-c905de05)\n")
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
    echo "::error::crate/module-level inner dead_code suppression is banned in crates/*/src (RST-0003:R6)"
    echo "::error::use a targeted OUTER item-level #[expect(dead_code, reason=\"...\")] instead, or delete/cfg-gate the dead item (RST-0003:R6)"
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
# Second half (ghr-xaoyd): R2 also requires every gate to name AT LEAST
# ONE rule id. Validating only the ids that happen to be present let a
# zero-citation gate pass vacuously — exactly how
# dead-code-inner-suppression-tripwire stayed green while violating R2.
# Every job under jobs: must therefore carry a PREFIX-NNNN:RN token on
# one of its own step `- name:` lines. Per RST-0007:R3 the job `name:`
# is deliberately NOT consulted: the citation must not live there.
# Fail-open guard, per check_adr_number_collision: the job enumeration
# is asserted to return a plausible non-zero count before the assertion
# runs — a parser that matches nothing would exit 0 forever and is
# worse than no check.
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

  local job_report
  job_report=$(awk '
    /^jobs:[[:space:]]*$/ { injobs = 1; next }
    injobs == 0 { next }
    /^[^[:space:]#]/ { injobs = 0; next }
    /^  [A-Za-z0-9_-]+:[[:space:]]*$/ {
      if (job != "") printf "%s\t%d\t%d\n", job, cited, steps
      job = $0
      sub(/^  /, "", job)
      sub(/:[[:space:]]*$/, "", job)
      cited = 0
      steps = 0
      next
    }
    job != "" && /^[[:space:]]*- name:/ {
      steps++
      if ($0 ~ /[A-Z][A-Z][A-Z]?[A-Z]?-[0-9][0-9][0-9][0-9]:R[0-9]/) cited = 1
    }
    END { if (job != "") printf "%s\t%d\t%d\n", job, cited, steps }
  ' .github/workflows/ci-reusable.yml)

  local job_count step_total
  job_count=$(printf '%s\n' "$job_report" | grep -c . || true)
  step_total=$(printf '%s\n' "$job_report" | awk -F'\t' '{s += $3} END {print s + 0}')
  if [ "$job_count" -eq 0 ] || [ "$step_total" -eq 0 ]; then
    echo "::error::gate-citation: enumerated ${job_count} job(s) and ${step_total} step name(s) from .github/workflows/ci-reusable.yml — fail-open guard tripped, refusing to pass silently (RST-0007:R7)"
    return 1
  fi

  local job_id job_cited
  while IFS=$'\t' read -r job_id job_cited _; do
    [ -z "$job_id" ] && continue
    if [ "$job_cited" -eq 0 ]; then
      echo "::error::gate-citation: job ${job_id} cites no ADR rule id on any of its step name: lines — every merge gate MUST name at least one rule it enforces (RST-0007:R2+R7)"
      fail=1
    fi
  done <<< "$job_report"

  return $fail
}

# GND-0009 / COM-0017:R4 (oracle finding A6: no ADR governs ADR-id collision)
# — mechanizes the enforcement surface this invariant needed: no two files
# under docs/adr/** (INCLUDING docs/adr/stale/) may claim the same
# PREFIX-NNNN id. Born from the SEC-0013 collision incident (two ADRs both
# claimed SEC-0013 across branches; resolved by renumbering one to SEC-0014).
# RESIDUAL GAP, recorded here deliberately (do not remove this notice): this
# guard sees ONLY the current branch's docs/adr/ tree. A number claimed on an
# unmerged remote/epic branch is still invisible until that branch merges —
# cross-branch/remote-ref detection was evaluated and is NOT shipped here
# (not cheaply/reliably achievable in CI); it may recur on the next
# long-lived branch and is not caught until merge time.
# Fail-open guard: the enumeration substep is asserted to return a non-zero,
# plausible ADR count before the duplicate check runs — a matcher that
# enumerates nothing would exit 0 forever and is worse than no check.
check_adr_number_collision() {
  local files
  files=$(find "$ROOT/docs/adr" -type f -name '*.md')

  local file_count
  file_count=$(printf '%s\n' "$files" | grep -c . || true)
  if [ "$file_count" -eq 0 ]; then
    echo "::error::adr-number-collision: enumerated ZERO files under docs/adr/**/*.md — fail-open guard tripped, refusing to pass silently"
    return 1
  fi

  local ids
  ids=$(printf '%s\n' "$files" | xargs -n1 basename | grep -oE '^[A-Z]{2,4}-[0-9]{4}')

  local dups
  dups=$(printf '%s\n' "$ids" | sort | uniq -d)
  if [ -z "$dups" ]; then
    return 0
  fi

  local fail=0
  while IFS= read -r dup; do
    [ -z "$dup" ] && continue
    echo "::error::adr-number-collision: duplicate ADR id ${dup} claimed by more than one file under docs/adr/ (GND-0009)"
    printf '%s\n' "$files" | xargs -n1 basename | grep -E "^${dup}-" | while IFS= read -r f; do
      echo "::error::adr-number-collision:   ${f}"
    done
    fail=1
  done <<< "$dups"
  return $fail
}

# SEC-0013:R2+R3+R4 — enforces the deny.toml advisory-ignore lifecycle:
# every ignore entry must be table form with a machine-parseable
# expires=/owner=/class= reason prefix, expiry must not be past-due, and
# unused-ignored-advisory must stay >= warn. Active in CHECKS (runs under
# `all`): ghr-y4hkd is discharged and deny.toml is in R3 shape.
# Manifest path overridable via DENY_TOML for fixture-based proof runs
# without touching the real deny.toml.
check_deny_ignore_lifecycle() {
  local manifest="${DENY_TOML:-$ROOT/deny.toml}"

  if [ ! -f "$manifest" ]; then
    echo "::error::deny-ignore-lifecycle: manifest not found at $manifest (SEC-0013:R3)"
    return 1
  fi

  local unused_setting
  unused_setting=$(grep -E '^[[:space:]]*unused-ignored-advisory[[:space:]]*=' "$manifest" | head -1 | sed -E 's/.*=[[:space:]]*"([^"]*)".*/\1/')
  if [ "$unused_setting" != "warn" ] && [ "$unused_setting" != "deny" ]; then
    echo "::error::deny-ignore-lifecycle: unused-ignored-advisory must be \"warn\" or stricter, found \"${unused_setting:-<unset>}\" (SEC-0013:R2)"
    return 1
  fi

  if ! grep -qE '^[[:space:]]*ignore[[:space:]]*=[[:space:]]*\[' "$manifest"; then
    if grep -q 'RUSTSEC-' "$manifest"; then
      echo "::error::deny-ignore-lifecycle: no [advisories] ignore = [ ] block matched, but RUSTSEC- text is present in $manifest — ignore entries appear present but none were parsed — parser/grammar drift, refusing to pass silently (SEC-0013:R3)"
      return 1
    fi
    echo "deny-ignore-lifecycle: no [advisories] ignore = [ ] block present and no RUSTSEC- text found in $manifest — genuinely clean, passing (SEC-0013:R3)"
    return 0
  fi

  local block
  block=$(awk '/^[[:space:]]*ignore[[:space:]]*=[[:space:]]*\[/{flag=1; next} flag && /^[[:space:]]*\]/{exit} flag{print}' "$manifest")

  local raw_lines
  raw_lines=$(printf '%s\n' "$block" | grep -vE '^[[:space:]]*(#.*)?$' || true)

  if [ -z "$raw_lines" ]; then
    if grep -q 'RUSTSEC-' "$manifest"; then
      echo "::error::deny-ignore-lifecycle: ignore = [ ] block present but empty in $manifest, yet RUSTSEC- text is present elsewhere — ignore entries appear present but none were parsed — parser/grammar drift, refusing to pass silently (SEC-0013:R3)"
      return 1
    fi
    echo "deny-ignore-lifecycle: ignore = [ ] block present but empty and no RUSTSEC- text found in $manifest — genuinely clean, passing (SEC-0013:R3)"
    return 0
  fi

  local fail=0
  local entry_count=0
  local today
  today=$(date +%F)

  while IFS= read -r line; do
    [ -z "$line" ] && continue
    trimmed=$(printf '%s' "$line" | sed -E 's/^[[:space:]]+|[[:space:]]+$//g')

    if printf '%s' "$trimmed" | grep -qE '^\{.*id[[:space:]]*=[[:space:]]*"[^"]+".*reason[[:space:]]*=[[:space:]]*"[^"]*".*\}'; then
      entry_count=$((entry_count + 1))
      local id reason
      id=$(printf '%s' "$trimmed" | sed -E 's/.*id[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/')
      reason=$(printf '%s' "$trimmed" | sed -E 's/.*reason[[:space:]]*=[[:space:]]*"([^"]*)".*/\1/')

      if printf '%s' "$reason" | grep -q 'class=vulnerability'; then
        echo "::error::deny-ignore-lifecycle: ${id} reason declares class=vulnerability, which MUST NOT be ignored (SEC-0013:R1)"
        fail=1
        continue
      fi

      if ! printf '%s' "$reason" | grep -qE '^expires=[0-9]{4}-[0-9]{2}-[0-9]{2} owner=[^ ]+ class=(unmaintained|notice) -- '; then
        echo "::error::deny-ignore-lifecycle: ${id} reason does not match required grammar 'expires=YYYY-MM-DD owner=<handle> class=unmaintained|notice -- ' (SEC-0013:R3)"
        fail=1
        continue
      fi

      local expires
      expires=$(printf '%s' "$reason" | sed -E 's/^expires=([0-9]{4}-[0-9]{2}-[0-9]{2}).*/\1/')
      if [ "$expires" \< "$today" ]; then
        echo "::error::deny-ignore-lifecycle: ${id} expires=${expires} is past-due (today=${today}) (SEC-0013:R4)"
        fail=1
      fi
    elif printf '%s' "$trimmed" | grep -qE '^"[^"]+"[[:space:]]*,?[[:space:]]*$'; then
      entry_count=$((entry_count + 1))
      local id
      id=$(printf '%s' "$trimmed" | sed -E 's/^"([^"]+)".*/\1/')
      echo "::error::deny-ignore-lifecycle: ${id} is bare-string form, not the required { id, reason } table form (SEC-0013:R3)"
      fail=1
    fi
  done <<< "$raw_lines"

  if [ "$entry_count" -eq 0 ]; then
    echo "::error::deny-ignore-lifecycle: ignore block has content but zero entries were parsed — parser/grammar drift, refusing to pass silently (SEC-0013:R3)"
    return 1
  fi

  return $fail
}

# RST-0005:R1 — mechanizes the "CI grep" enforcement half of "Every crate
# in the workspace includes #![forbid(unsafe_code)] at the crate root,
# enforced by clippy's disallowed-macros or CI grep". Before this check the
# attribute was carried by convention only: a new crate could silently omit
# it and nothing failed. Totality is the point — a non-total check is not a
# check.
# Scope: every workspace member listed in the root Cargo.toml, and for each
# member every COMPILATION root it owns (src/lib.rs, src/main.rs, and each
# src/bin/*.rs), since #![forbid] is an inner attribute scoped to one root.
# Fail-open guards: zero members parsed, or zero roots discovered, are hard
# failures — a matcher that enumerates nothing would exit 0 forever and is
# worse than no check. Manifest overridable via FORBID_UNSAFE_MANIFEST and
# member paths resolved under FORBID_UNSAFE_ROOT for fixture-based proof
# runs.
check_forbid_unsafe_total() {
  local manifest="${FORBID_UNSAFE_MANIFEST:-$ROOT/Cargo.toml}"
  local base="${FORBID_UNSAFE_ROOT:-$ROOT}"

  if [ ! -f "$manifest" ]; then
    echo "::error::forbid-unsafe-total: workspace manifest not found at $manifest (RST-0005:R1)"
    return 1
  fi

  local members
  members=$(awk '/^members[[:space:]]*=[[:space:]]*\[/{flag=1; next} flag && /^[[:space:]]*\]/{exit} flag{print}' "$manifest" \
    | grep -oE '"[^"]+"' | tr -d '"')

  local member_count
  member_count=$(printf '%s\n' "$members" | grep -c . || true)
  if [ "$member_count" -eq 0 ]; then
    echo "::error::forbid-unsafe-total: parsed ZERO workspace members from $manifest — fail-open guard tripped, refusing to pass silently (RST-0005:R1)"
    return 1
  fi

  local roots=()
  local m
  while IFS= read -r m; do
    [ -z "$m" ] && continue
    local dir="$base/$m"
    if [ ! -d "$dir" ]; then
      echo "::error::forbid-unsafe-total: workspace member ${m} listed in $manifest has no directory at ${dir} (RST-0005:R1)"
      return 1
    fi
    local candidate
    for candidate in "$dir/src/lib.rs" "$dir/src/main.rs"; do
      [ -f "$candidate" ] && roots+=("$candidate")
    done
    if [ -d "$dir/src/bin" ]; then
      while IFS= read -r candidate; do
        [ -n "$candidate" ] && roots+=("$candidate")
      done < <(find "$dir/src/bin" -maxdepth 1 -type f -name '*.rs' | sort)
    fi
  done <<< "$members"

  if [ "${#roots[@]}" -eq 0 ]; then
    echo "::error::forbid-unsafe-total: discovered ZERO crate roots across ${member_count} workspace members — fail-open guard tripped, refusing to pass silently (RST-0005:R1)"
    return 1
  fi

  local fail=0
  local root_file
  for root_file in "${roots[@]}"; do
    if ! grep -qE '^#!\[forbid\(unsafe_code\)\]' "$root_file"; then
      echo "::error::forbid-unsafe-total: ${root_file#"$base"/} lacks #![forbid(unsafe_code)] at the crate root (RST-0005:R1)"
      fail=1
    fi
  done

  if [ "$fail" -eq 0 ]; then
    echo "forbid-unsafe-total: ${#roots[@]} crate roots across ${member_count} workspace members all carry #![forbid(unsafe_code)] (RST-0005:R1)"
  fi
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
    adr-number-collision) check_adr_number_collision ;;
    deny-ignore-lifecycle) check_deny_ignore_lifecycle ;;
    forbid-unsafe-total) check_forbid_unsafe_total ;;
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
