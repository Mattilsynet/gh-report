#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

CHECKS=(projection-lock async-trait fence-converge dead-code-suppression non-exhaustive gate-citation adr-number-collision deny-ignore-lifecycle forbid-unsafe-total)

# Activated (ghr-y4hkd discharged, ghr-zcr7c/ghr-swxy8): deny.toml now
# satisfies SEC-0013:R3 (table-form ignores) as of commit be14235; the
# check is safe to run on every PR.
#
# forbid-unsafe-total activated (ghr-5rewy): the two previously uncovered
# crate roots now carry #![forbid(unsafe_code)], so RST-0005:R1 coverage
# is total across every workspace compilation root.
PENDING_CHECKS=()

# Wall-clock bound for the read-only `cargo metadata` / `cargo tree` probes.
# A hung probe reaches NO verdict, so it must never be folded into the
# no-violation result (AGENTS.md code-quality rule 1: error is not a negative
# finding). The containing CI job's own 30-minute timeout bounds the job but
# not the verdict: without this, a probe wedged behind a registry stall or a
# lock consumes the job and the gate's answer is decided by the clock rather
# than by the code. Overridable via TRIPWIRE_PROBE_TIMEOUT_SECS.
# Scope: the read-only metadata/tree probes only. `cargo run -p
# non-exhaustive-check` is deliberately NOT wrapped — it compiles, so its
# runtime is unbounded by design on a cold cache and a wall-clock bound there
# would convert a slow build into a false gate failure.
PROBE_TIMEOUT_SECS="${TRIPWIRE_PROBE_TIMEOUT_SECS:-120}"

probe_out=""

run_probe() {
  local label=$1
  shift

  if ! command -v timeout >/dev/null 2>&1; then
    echo "::error::${label}: 'timeout' not found on PATH — refusing to run an unbounded probe, because a probe that hangs reaches no verdict and cannot be distinguished from a clean one"
    return 1
  fi
  if [[ ! "$PROBE_TIMEOUT_SECS" =~ ^[0-9]+$ ]] || [ "$PROBE_TIMEOUT_SECS" -eq 0 ]; then
    echo "::error::${label}: TRIPWIRE_PROBE_TIMEOUT_SECS='${PROBE_TIMEOUT_SECS}' is not a positive integer — refusing to run a probe with an uninterpretable bound"
    return 1
  fi

  local status=0
  probe_out=$(timeout -- "$PROBE_TIMEOUT_SECS" "$@" 2>&1) || status=$?
  case "$status" in
    0) return 0 ;;
    124 | 137)
      echo "::error::${label}: probe '$*' was killed after ${PROBE_TIMEOUT_SECS}s at the wall clock (status ${status}) — it reached NO verdict, refusing to fold a timeout into the no-violation result (AGENTS.md code-quality rule 1)"
      return 1
      ;;
    *) return "$status" ;;
  esac
}

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

# Scope: every cherry-pit-* package of the workspace, enumerated structurally
# via `cargo metadata --no-deps` rather than a literal list or a hand-rolled
# manifest parser — a literal silently drops coverage on a rename or a new
# crate, which is how a renamed crate went uncovered while the check stayed
# green (ghr-8602n), and a line-oriented parser both drops a same-line first
# member and picks up quoted strings out of comments. The name anchor is
# ^cherry-pit-, so pardosa-cherry-pit-test-support is out of scope.
# Fail-open guards: zero cherry-pit-* packages enumerated is a hard failure —
# a matcher that enumerates nothing would exit 0 forever and is worse than no
# check. A failing `cargo metadata` or `cargo tree` invocation is likewise a
# hard failure, never a silent skip and never degraded into the empty
# enumeration: the original defect piped the error stream into grep, making an
# unresolvable crate indistinguishable from a clean one.
# NO unchecked fallible stage may exist in this function: `|| true` is banned
# here outright, because it discards exactly the status this check exists to
# observe. Selection and ordering are a single checked jq rather than a
# `grep | sort` pipeline whose stage statuses are discarded — a sort that
# truncated the set to one name and exited 74 previously produced a narrowed
# CLEAN verdict. The surviving candidate count is cross-checked against a
# count derived independently from the same metadata, so a set narrowed in
# transit is an error rather than a smaller pass, and each candidate is
# re-anchored on cherry-pit- so a regressed selection cannot widen it. The
# match is a bash case glob, not `grep -q`: grep's exit 2 (I/O or binary
# fault) is indistinguishable from "no match" and would read as clean. Both
# cargo invocations are --locked: without it this read-only merge gate
# rewrote Cargo.lock and inspected a freshly resolved graph instead of the
# committed one. Both also carry --manifest-path, so the enumeration stage
# and the probe stage cannot inspect two different workspaces.
# Manifest overridable via ASYNC_TRAIT_MANIFEST for fixture-based proof runs.
check_async_trait() {
  local manifest="${ASYNC_TRAIT_MANIFEST:-$ROOT/Cargo.toml}"

  if [ ! -f "$manifest" ]; then
    echo "::error::async-trait: workspace manifest not found at $manifest (CHE-0025:R1+R2)"
    return 1
  fi

  local meta
  if ! run_probe "async-trait" cargo metadata --locked --no-deps --format-version 1 --manifest-path "$manifest"; then
    echo "::error::async-trait: cargo metadata --locked --no-deps --manifest-path ${manifest} FAILED — a failed workspace enumeration is an ERROR, not an empty member set, refusing to fold it into the no-violation verdict (CHE-0025:R1+R2)"
    printf '%s\n' "$probe_out"
    return 1
  fi
  meta=$probe_out

  local crates
  if ! crates=$(jq -r '[.packages[].name | select(startswith("cherry-pit-"))] | sort | .[]' <<< "$meta"); then
    echo "::error::async-trait: selecting and ordering cherry-pit-* package names from cargo metadata for ${manifest} FAILED — refusing to treat an unreadable enumeration as empty (CHE-0025:R1+R2)"
    return 1
  fi

  local declared_count
  if ! declared_count=$(jq -r '[.packages[].name | select(startswith("cherry-pit-"))] | length' <<< "$meta"); then
    echo "::error::async-trait: deriving the cherry-pit-* member count from cargo metadata for ${manifest} FAILED — refusing to probe an unverified candidate set (CHE-0025:R1+R2)"
    return 1
  fi
  if [[ ! "$declared_count" =~ ^[0-9]+$ ]]; then
    echo "::error::async-trait: cargo metadata yielded a non-numeric cherry-pit-* member count '${declared_count}' for ${manifest} — refusing to probe an unverified candidate set (CHE-0025:R1+R2)"
    return 1
  fi

  local -a crate_list=()
  local c
  while IFS= read -r c; do
    [ -z "$c" ] && continue
    if [[ "$c" != cherry-pit-* ]]; then
      echo "::error::async-trait: candidate '${c}' from ${manifest} does not carry the cherry-pit- name anchor — the selection stage is not producing what it claims, refusing to probe an unverified candidate set (CHE-0025:R1+R2)"
      return 1
    fi
    crate_list+=("$c")
  done <<< "$crates"

  local crate_count=${#crate_list[@]}
  if [ "$crate_count" -ne "$declared_count" ]; then
    echo "::error::async-trait: candidate set narrowed in transit for ${manifest} — cargo metadata declares ${declared_count} cherry-pit-* members but only ${crate_count} survived enumeration; a truncated candidate set is an ERROR, not a smaller clean verdict (CHE-0025:R1+R2)"
    return 1
  fi
  if [ "$crate_count" -eq 0 ]; then
    echo "::error::async-trait: enumerated ZERO cherry-pit-* workspace members from $manifest — fail-open guard tripped, refusing to pass silently (CHE-0025:R1+R2)"
    return 1
  fi

  local fail=0
  local tree
  for c in "${crate_list[@]}"; do
    if ! run_probe "async-trait" cargo tree --locked --manifest-path "$manifest" -p "$c" -e features; then
      echo "::error::async-trait: cargo tree --locked --manifest-path ${manifest} -p ${c} -e features FAILED — a probe error is not a clean result, refusing to fold it into the no-violation verdict (CHE-0025:R1+R2)"
      printf '%s\n' "$probe_out"
      fail=1
      continue
    fi
    tree=$probe_out
    case "$tree" in
      *async-trait*)
        echo "::error::$c transitively depends on async-trait (CHE-0025:R1+R2)"
        fail=1
        ;;
    esac
  done

  if [ "$fail" -eq 0 ]; then
    echo "async-trait: ${crate_count} cherry-pit-* workspace members enumerated from ${manifest} carry no async-trait edge (CHE-0025:R1+R2)"
  fi
  return $fail
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
  start2=$(grep -n '^async fn run_one_team_refresh_tick' "$FILE" | cut -d: -f1)
  end2=$(awk -v s="$start2" 'NR>s && /^(async )?fn /{print NR; exit}' "$FILE")
  fail=0
  check_window "$start1" "$end1" "spawn_collection_loop" || fail=1
  check_window "$start2" "$end2" "run_one_team_refresh_tick" || fail=1
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
# Workflow path overridable via GATE_CITATION_WORKFLOW for fixture-based
# proof runs: with the path hardcoded the fail-open guard above could not
# be exercised against a fixture and was itself asserted-but-unproven —
# the same fail-open shape it exists to close, one level down. Sibling
# checks parameterize for exactly this reason (DENY_TOML,
# FORBID_UNSAFE_MANIFEST). The override redirects EVERY workflow-consuming
# operation in this function — both the citation-token scan and the job
# enumeration — so a fixture cannot redirect one stage while another still
# reads the real workflow (the split-graph exposure of ghr-z9cho.4).
# A missing workflow is a hard failure, never an empty token set: the
# greps below carry `|| true` to tolerate a legitimate no-match, which
# would otherwise make an unreadable file indistinguishable from a clean
# one.
check_gate_citation() {
  local workflow="${GATE_CITATION_WORKFLOW:-$ROOT/.github/workflows/ci-reusable.yml}"

  if [ ! -f "$workflow" ]; then
    echo "::error::gate-citation: workflow not found at $workflow — an unreadable workflow is an ERROR, not a clean one (RST-0007:R7)"
    return 1
  fi

  local fail=0
  local tokens
  tokens=$( { grep -hoE -- '- name:.*' "$workflow" || true; \
              grep -hoE '::error::.*' "$workflow" tools/tripwires.sh || true; } \
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
  ' "$workflow")

  local job_count step_total
  job_count=$(printf '%s\n' "$job_report" | grep -c . || true)
  step_total=$(printf '%s\n' "$job_report" | awk -F'\t' '{s += $3} END {print s + 0}')
  if [ "$job_count" -eq 0 ] || [ "$step_total" -eq 0 ]; then
    echo "::error::gate-citation: enumerated ${job_count} job(s) and ${step_total} step name(s) from ${workflow} — fail-open guard tripped, refusing to pass silently (RST-0007:R7)"
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
# Scope: every workspace member, and for each member every COMPILATION root
# it owns (the lib/bin/cdylib/rlib/staticlib/proc-macro target src_paths),
# since #![forbid] is an inner attribute scoped to one root.
# Enumeration is `cargo metadata --locked --no-deps`, NOT a hand-rolled awk
# scan of [workspace].members. The awk parser this replaced carried a proven
# fail-OPEN: its `next` unconditionally skipped the line holding
# `members = [`, so a valid TOML spelling that puts the first member on that
# line dropped that member silently and the gate reported CLEAN — measured on
# a fixture whose dropped member lacked the attribute, exit 0. It also read
# quoted strings out of COMMENT lines inside the array (phantom member, hard
# fail on valid TOML), and ran past the array entirely when the closing `]`
# shared a line with the last member, harvesting `resolver = "2"` as a member.
# Targets are deduplicated by src_path: a target is emitted once per matching
# `kind` element, so gh-report-web-client (kind ["cdylib","rlib"]) would be
# counted twice and inflate coverage — `any(.kind[]; ...)` plus `unique`
# emits each root at most once.
# NO unchecked fallible stage may exist here: `|| true` is banned outright,
# because it discards exactly the status this check exists to observe. The
# root count is cross-checked against the count cargo metadata declares, so a
# set narrowed in transit is an ERROR rather than a smaller pass. The member
# count is cross-checked against a second, independently derived member count:
# the number of packages contributing AT LEAST ONE selected compilation root.
# A package that contributes none is named in an `::error::` and hard-fails —
# that is the guard against a package silently dropping out of root
# enumeration while the gate still reports clean. Zero members and zero roots
# are hard failures — a matcher that enumerates nothing would exit 0 forever
# and is worse than no check. The attribute probe distinguishes THREE
# outcomes, not two: grep's exit 2 (I/O or binary fault) is a probe ERROR and
# must not read as either a clean root or a missing attribute. cargo metadata
# is --locked: without it this read-only merge gate rewrites Cargo.lock and
# inspects a freshly resolved graph instead of the committed one.
# Manifest overridable via FORBID_UNSAFE_MANIFEST and paths displayed
# relative to FORBID_UNSAFE_ROOT for fixture-based proof runs.
check_forbid_unsafe_total() {
  local manifest="${FORBID_UNSAFE_MANIFEST:-$ROOT/Cargo.toml}"
  local base="${FORBID_UNSAFE_ROOT:-$ROOT}"
  local kindpred='any(.kind[]; . == "lib" or . == "bin" or . == "cdylib" or . == "rlib" or . == "staticlib" or . == "proc-macro")'
  local sel="[.packages[].targets[] | select(${kindpred}) | .src_path] | unique"
  local covsel="[.packages[] | select([.targets[] | select(${kindpred})] | length > 0)] | length"
  local uncovsel="[.packages[] | select([.targets[] | select(${kindpred})] | length == 0) | .name] | .[]"

  if [ ! -f "$manifest" ]; then
    echo "::error::forbid-unsafe-total: workspace manifest not found at $manifest (RST-0005:R1)"
    return 1
  fi

  local meta
  if ! run_probe "forbid-unsafe-total" cargo metadata --locked --no-deps --format-version 1 --manifest-path "$manifest"; then
    echo "::error::forbid-unsafe-total: cargo metadata --locked --no-deps --manifest-path ${manifest} FAILED — a failed workspace enumeration is an ERROR, not an empty member set, refusing to fold it into the no-violation verdict (RST-0005:R1)"
    printf '%s\n' "$probe_out"
    return 1
  fi
  meta=$probe_out

  local member_count
  if ! member_count=$(jq -r '.packages | length' <<< "$meta"); then
    echo "::error::forbid-unsafe-total: deriving the workspace member count from cargo metadata for ${manifest} FAILED — refusing to treat an unreadable enumeration as empty (RST-0005:R1)"
    return 1
  fi
  if [[ ! "$member_count" =~ ^[0-9]+$ ]]; then
    echo "::error::forbid-unsafe-total: cargo metadata yielded a non-numeric workspace member count '${member_count}' for ${manifest} — refusing to probe an unverified candidate set (RST-0005:R1)"
    return 1
  fi
  if [ "$member_count" -eq 0 ]; then
    echo "::error::forbid-unsafe-total: enumerated ZERO workspace members from $manifest — fail-open guard tripped, refusing to pass silently (RST-0005:R1)"
    return 1
  fi

  local roots
  if ! roots=$(jq -r "${sel} | .[]" <<< "$meta"); then
    echo "::error::forbid-unsafe-total: selecting compilation-root src_paths from cargo metadata for ${manifest} FAILED — refusing to treat an unreadable enumeration as empty (RST-0005:R1)"
    return 1
  fi

  local declared_roots
  if ! declared_roots=$(jq -r "${sel} | length" <<< "$meta"); then
    echo "::error::forbid-unsafe-total: deriving the compilation-root count from cargo metadata for ${manifest} FAILED — refusing to probe an unverified candidate set (RST-0005:R1)"
    return 1
  fi
  if [[ ! "$declared_roots" =~ ^[0-9]+$ ]]; then
    echo "::error::forbid-unsafe-total: cargo metadata yielded a non-numeric compilation-root count '${declared_roots}' for ${manifest} — refusing to probe an unverified candidate set (RST-0005:R1)"
    return 1
  fi

  local -a root_list=()
  local r
  while IFS= read -r r; do
    [ -z "$r" ] && continue
    if [[ "$r" != /* || "$r" != *.rs ]]; then
      echo "::error::forbid-unsafe-total: candidate root '${r}' from ${manifest} is not an absolute path to a .rs file — the selection stage is not producing what it claims, refusing to probe an unverified candidate set (RST-0005:R1)"
      return 1
    fi
    if [ ! -f "$r" ]; then
      echo "::error::forbid-unsafe-total: candidate root '${r}' from ${manifest} does not exist on disk — a missing compilation root is an ERROR, not a clean one (RST-0005:R1)"
      return 1
    fi
    root_list+=("$r")
  done <<< "$roots"

  local root_count=${#root_list[@]}
  if [ "$root_count" -ne "$declared_roots" ]; then
    echo "::error::forbid-unsafe-total: candidate set narrowed in transit for ${manifest} — cargo metadata declares ${declared_roots} compilation roots but only ${root_count} survived enumeration; a truncated candidate set is an ERROR, not a smaller clean verdict (RST-0005:R1)"
    return 1
  fi
  if [ "$root_count" -eq 0 ]; then
    echo "::error::forbid-unsafe-total: discovered ZERO crate roots across ${member_count} workspace members from $manifest — fail-open guard tripped, refusing to pass silently (RST-0005:R1)"
    return 1
  fi

  local covered_count
  if ! covered_count=$(jq -r "${covsel}" <<< "$meta"); then
    echo "::error::forbid-unsafe-total: deriving the per-package coverage count from cargo metadata for ${manifest} FAILED — refusing to treat an unreadable enumeration as full coverage (RST-0005:R1)"
    return 1
  fi
  if [[ ! "$covered_count" =~ ^[0-9]+$ ]]; then
    echo "::error::forbid-unsafe-total: cargo metadata yielded a non-numeric per-package coverage count '${covered_count}' for ${manifest} — refusing to probe an unverified candidate set (RST-0005:R1)"
    return 1
  fi
  if [ "$covered_count" -ne "$member_count" ]; then
    local uncovered
    if ! uncovered=$(jq -r "${uncovsel}" <<< "$meta"); then
      echo "::error::forbid-unsafe-total: naming the packages contributing zero compilation roots for ${manifest} FAILED (RST-0005:R1)"
      return 1
    fi
    local u
    while IFS= read -r u; do
      [ -z "$u" ] && continue
      echo "::error::forbid-unsafe-total: workspace package '${u}' contributes ZERO selected compilation roots — a package invisible to root enumeration is an ERROR, not a clean package (RST-0005:R1)"
    done <<< "$uncovered"
    echo "::error::forbid-unsafe-total: per-package coverage FAILED for ${manifest} — ${covered_count} of ${member_count} workspace packages contribute at least one selected compilation root (RST-0005:R1)"
    return 1
  fi

  local fail=0
  local root_file
  local probe
  for root_file in "${root_list[@]}"; do
    probe=0
    grep -qE '^#!\[forbid\(unsafe_code\)\]' "$root_file" || probe=$?
    case "$probe" in
      0) ;;
      1)
        echo "::error::forbid-unsafe-total: ${root_file#"$base"/} lacks #![forbid(unsafe_code)] at the crate root (RST-0005:R1)"
        fail=1
        ;;
      *)
        echo "::error::forbid-unsafe-total: probing ${root_file#"$base"/} for #![forbid(unsafe_code)] FAILED with grep status ${probe} — a probe error is not a clean result and is not a missing attribute, refusing to fold it into either verdict (RST-0005:R1)"
        fail=1
        ;;
    esac
  done

  if [ "$fail" -eq 0 ]; then
    echo "forbid-unsafe-total: ${root_count} crate roots across ${member_count} workspace members all carry #![forbid(unsafe_code)] (RST-0005:R1)"
  fi
  return $fail
}

run_check() {
  case "$1" in
    projection-lock) check_projection_lock ;;
    async-trait) check_async_trait ;;
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
