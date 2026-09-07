#!/usr/bin/env bash
set -euo pipefail

# Committed regression harness pinning tools/tripwires.sh gate behaviour.
#
# The gates are merge gates, and until this file existed their correctness was
# evidenced only by bead prose plus ad-hoc plants reconstructed by hand. That is
# evidence the gates were right on the day, not a guard against reintroducing
# the fail-open they were written to close: CI exercises them only against the
# real, homogeneous, conforming tree, so a future edit can reintroduce an
# awk-class under-match with every check green.
#
# Every scenario below drives a gate against a synthetic workspace via the
# gate's own fixture overrides and asserts an exit code AND a message fragment.
# The message fragment matters as much as the code: a gate that fails for the
# wrong reason is not pinned by exit status alone.
#
# The harness doubles as the override-redirection self-test (ghr-z9cho.4). A
# fixture override that silently no-ops proves nothing about the operation
# under test — it is a fail-open in the proof harness rather than in the gate,
# the same class one level up. Each REDIRECT scenario therefore points a gate
# at a fixture that MUST produce a different verdict from the real tree, so a
# no-op override is observable as a false pass.

ROOT="$(python3 -c 'import pathlib, sys; print(pathlib.Path(sys.argv[1]).resolve(strict=True).parent.parent)' "${BASH_SOURCE[0]}")"
if [ ! -f "$ROOT/Cargo.toml" ] || ! grep -Eq '^\[workspace\][[:space:]]*$' "$ROOT/Cargo.toml" || [ ! -f "$ROOT/tools/tripwires.sh" ] || [ ! -f "$ROOT/tools/tripwire-regression.sh" ] || [ ! -f "$ROOT/crates/non-exhaustive-check/src/main.rs" ]; then
  printf '::error::invalid derived workspace root: %s\n' "$ROOT" >&2
  exit 1
fi
cd "$ROOT"

TRIPWIRES="$ROOT/tools/tripwires.sh"
SCRATCH="$ROOT/.ooda/tmp/tripwire-regression.$$"
PASS=0
FAIL=0

# The required scenario set, declared in code and keyed by label.
#
# A harness that only counts failures is fail-open to DELETION: remove an
# expect invocation and the run reports fewer passes and still exits 0, so a
# future edit can silently shed coverage while the gate suite stays green —
# the same false-clean class the gates themselves were written to close, one
# level up. Every label below must execute exactly once per run; a missing,
# duplicated, or unregistered label fails the harness independently of any
# gate verdict.
REQUIRED_SCENARIOS="
fut/baseline conforming
fut/under-match first-member-on-members-line
fut/over-match commented-out member
fut/dedup cdylib+rlib counted once
fut/zero members
fut/package contributing zero roots
fut/cargo metadata failure
fut/manifest absent
fut/probe error is neither clean nor missing
redirect/FORBID_UNSAFE_MANIFEST reaches enumeration and probe
redirect/ASYNC_TRAIT_MANIFEST reaches enumeration and tree probe
redirect/DENY_TOML reaches the ignore-block parser
redirect/GATE_CITATION_WORKFLOW trips the fail-open guard
redirect/GATE_CITATION_WORKFLOW enforces per-job citation
timeout/hung cargo probe fails closed
timeout/uninterpretable bound fails closed
"
EXECUTED=""

cleanup() {
  if [ "${TRIPWIRE_KEEP_SCRATCH:-0}" = 1 ]; then
    printf 'Retained regression scratch: %s\n' "$SCRATCH"
    return
  fi
  chmod -R u+rwX "$SCRATCH" 2>/dev/null || true
  rm -r "$SCRATCH" 2>/dev/null || true
}
trap cleanup EXIT

mkdir -p "$SCRATCH"

# Run one gate under a fixture and assert both its exit code and a fragment of
# its output. Usage: expect <label> <want_exit> <want_fragment> <check> [ENV=V ...]
expect() {
  local label=$1 want_exit=$2 want_frag=$3 check=$4
  shift 4

  EXECUTED="${EXECUTED}${label}
"

  local out status=0
  out=$(env "$@" bash "$TRIPWIRES" "$check" 2>&1) || status=$?

  if [ "$status" -ne "$want_exit" ]; then
    echo "FAIL ${label}: expected exit ${want_exit}, got ${status}"
    printf '%s\n' "$out" | sed 's/^/     | /'
    FAIL=$((FAIL + 1))
    return 0
  fi
  if ! printf '%s' "$out" | grep -qF -- "$want_frag"; then
    echo "FAIL ${label}: exit ${status} as expected, but output does not contain '${want_frag}'"
    printf '%s\n' "$out" | sed 's/^/     | /'
    FAIL=$((FAIL + 1))
    return 0
  fi
  echo "ok   ${label} (exit ${status})"
  PASS=$((PASS + 1))
}

# Materialise a fixture workspace and resolve its lockfile, so the gates' own
# --locked probes have a committed graph to read rather than resolving a fresh
# one. A fixture whose lockfile cannot be produced is a harness fault, not a
# gate verdict, and aborts rather than reporting a gate result.
new_ws() {
  local name=$1
  local dir="$SCRATCH/$name"
  mkdir -p "$dir"
  printf '%s' "$dir"
}

lock_ws() {
  local dir=$1
  if ! cargo generate-lockfile --manifest-path "$dir/Cargo.toml" >/dev/null 2>&1; then
    echo "HARNESS FAULT: could not resolve a lockfile for fixture $dir" >&2
    exit 2
  fi
}

member_pkg() {
  local dir=$1 name=$2 attr=$3
  mkdir -p "$dir/$name/src"
  cat > "$dir/$name/Cargo.toml" <<EOF
[package]
name = "$name"
version = "0.0.0"
edition = "2024"

[lib]
path = "src/lib.rs"
EOF
  if [ "$attr" = "forbid" ]; then
    printf '#![forbid(unsafe_code)]\n' > "$dir/$name/src/lib.rs"
  else
    printf 'pub fn f() {}\n' > "$dir/$name/src/lib.rs"
  fi
}

echo "== forbid-unsafe-total =="

# Baseline: a conforming two-member workspace must pass, so every failing
# scenario below is attributable to the planted defect rather than to the
# fixture shape itself.
D=$(new_ws fut_clean)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = [
  "alpha",
  "beta",
]
EOF
member_pkg "$D" alpha forbid
member_pkg "$D" beta forbid
lock_ws "$D"
expect "fut/baseline conforming" 0 "2 crate roots across 2 workspace members" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 1, the fus-0905 money shot: the retired awk parser unconditionally
# skipped the line holding `members = [`, so a first member spelled on that
# line was dropped silently and the gate reported CLEAN. Here that dropped
# member is the one lacking the attribute.
D=$(new_ws fut_undermatch)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["alpha",
  "beta",
]
EOF
member_pkg "$D" alpha plain
member_pkg "$D" beta forbid
lock_ws "$D"
expect "fut/under-match first-member-on-members-line" 1 "alpha/src/lib.rs lacks #![forbid(unsafe_code)]" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 2: a quoted name inside a COMMENT within the array is not a member.
# The retired parser harvested it and hard-failed on valid TOML.
D=$(new_ws fut_overmatch)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = [
  "alpha",
  # "ghost",
]
EOF
member_pkg "$D" alpha forbid
lock_ws "$D"
expect "fut/over-match commented-out member" 0 "1 crate roots across 1 workspace members" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 3: cargo metadata emits a target once per matching kind, so a
# cdylib+rlib package double-counts under select(.kind[] | IN(...)) and
# inflates coverage. any(.kind[]; ...) plus unique must emit it once.
D=$(new_ws fut_dedup)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["alpha"]
EOF
mkdir -p "$D/alpha/src"
cat > "$D/alpha/Cargo.toml" <<'EOF'
[package]
name = "alpha"
version = "0.0.0"
edition = "2024"

[lib]
path = "src/lib.rs"
crate-type = ["cdylib", "rlib"]
EOF
printf '#![forbid(unsafe_code)]\n' > "$D/alpha/src/lib.rs"
lock_ws "$D"
expect "fut/dedup cdylib+rlib counted once" 0 "1 crate roots across 1 workspace members" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 4: a matcher that enumerates nothing exits 0 forever and is worse
# than no check.
D=$(new_ws fut_zero_members)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = []
EOF
lock_ws "$D"
expect "fut/zero members" 1 "enumerated ZERO workspace members" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 5: a package contributing no selected compilation root is invisible
# to root enumeration; the per-package coverage assertion must name it rather
# than pass on the remaining members.
D=$(new_ws fut_zero_roots)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["alpha", "beta"]
EOF
member_pkg "$D" alpha forbid
mkdir -p "$D/beta/examples"
cat > "$D/beta/Cargo.toml" <<'EOF'
[package]
name = "beta"
version = "0.0.0"
edition = "2024"
autolib = false

[[example]]
name = "demo"
path = "examples/demo.rs"
EOF
printf 'fn main() {}\n' > "$D/beta/examples/demo.rs"
lock_ws "$D"
expect "fut/package contributing zero roots" 1 "workspace package 'beta' contributes ZERO selected compilation roots" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 6: a failed enumeration is an ERROR, never an empty member set. The
# original defect piped the error stream into the consumer, making an
# unresolvable workspace indistinguishable from a clean one.
D=$(new_ws fut_metadata_fail)
printf '[workspace\nthis is not toml\n' > "$D/Cargo.toml"
expect "fut/cargo metadata failure" 1 "cargo metadata --locked --no-deps --manifest-path" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# Scenario 7: an absent manifest is an error, not an empty workspace.
expect "fut/manifest absent" 1 "workspace manifest not found" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$SCRATCH/nonexistent/Cargo.toml" "FORBID_UNSAFE_ROOT=$SCRATCH"

# Scenario 8: grep's exit 2 is an I/O or binary fault, indistinguishable from
# "no match" to a two-outcome probe. The gate must report it as neither a clean
# root nor a missing attribute.
#
# The fault is induced by a deterministic `grep` shim that returns status 2
# for the forbid-attribute probe and delegates every other invocation to the
# real grep. Permission-based induction was not deterministic — a privileged
# runner can still read a mode-000 file, which previously made this scenario
# skip silently and shed one of the three required verdicts with the summary
# still green. A shim reaches the probe on every runner, root included, so the
# scenario is mandatory and has no skip path. Making the root a directory does
# not work: the gate's own existence check rejects a non-regular root first,
# which is a different verdict from a probe fault.
D=$(new_ws fut_probe_error)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["alpha"]
EOF
member_pkg "$D" alpha forbid
lock_ws "$D"
mkdir -p "$D/bin"
cat > "$D/bin/grep" <<'EOF'
#!/usr/bin/env bash
for a in "$@"; do
  if [ "$a" = '^#!\[forbid\(unsafe_code\)\]' ]; then
    echo "grep: simulated I/O fault" >&2
    exit 2
  fi
done
exec /usr/bin/grep "$@"
EOF
chmod +x "$D/bin/grep"
expect "fut/probe error is neither clean nor missing" 1 "FAILED with grep status 2" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D" \
  "PATH=$D/bin:$PATH"

echo "== override redirection (a no-op override proves nothing) =="

# FORBID_UNSAFE_MANIFEST: the fixture verdict (1 root / 1 member) differs from
# the real tree's, so an override that failed to redirect would report the real
# counts and be caught here.
D=$(new_ws redirect_fut)
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["alpha"]
EOF
member_pkg "$D" alpha forbid
lock_ws "$D"
expect "redirect/FORBID_UNSAFE_MANIFEST reaches enumeration and probe" 0 "1 crate roots across 1 workspace members" \
  forbid-unsafe-total "FORBID_UNSAFE_MANIFEST=$D/Cargo.toml" "FORBID_UNSAFE_ROOT=$D"

# ASYNC_TRAIT_MANIFEST: the fixture's only cherry-pit member does not exist in
# the real workspace, so the cargo tree stage can only succeed if it honoured
# the override too. This is the split-graph exposure of ghr-z9cho.4 made
# observable: enumeration and probing must read the same workspace.
D=$(new_ws redirect_at)
mkdir -p "$D/src"
cat > "$D/Cargo.toml" <<'EOF'
[workspace]
members = ["."]
resolver = "3"

[package]
name = "cherry-pit-regression-fixture"
version = "0.0.0"
edition = "2024"

[lib]
path = "src/lib.rs"
EOF
printf '#![forbid(unsafe_code)]\n' > "$D/src/lib.rs"
lock_ws "$D"
expect "redirect/ASYNC_TRAIT_MANIFEST reaches enumeration and tree probe" 0 "1 cherry-pit-* workspace members enumerated" \
  async-trait "ASYNC_TRAIT_MANIFEST=$D/Cargo.toml"

# DENY_TOML: bare-string ignore form is rejected, a verdict the real deny.toml
# does not produce.
D=$(new_ws redirect_deny)
mkdir -p "$D"
cat > "$D/deny.toml" <<'EOF'
[advisories]
unused-ignored-advisory = "warn"
ignore = [
  "RUSTSEC-0000-0000",
]
EOF
expect "redirect/DENY_TOML reaches the ignore-block parser" 1 "is bare-string form" \
  deny-ignore-lifecycle "DENY_TOML=$D/deny.toml"

# GATE_CITATION_WORKFLOW: the fail-open job-enumeration guard is unreachable
# against the real workflow, which always has jobs. This is the scenario the
# hardcoded path made unprovable (ghr-ox4zq).
D=$(new_ws redirect_gate)
mkdir -p "$D"
printf 'name: fixture\non:\n  workflow_call:\njobs:\n' > "$D/zerojob.yml"
expect "redirect/GATE_CITATION_WORKFLOW trips the fail-open guard" 1 "fail-open guard tripped" \
  gate-citation "GATE_CITATION_WORKFLOW=$D/zerojob.yml"

printf 'name: fixture\non:\n  workflow_call:\njobs:\n  uncited:\n    runs-on: ubuntu-latest\n    steps:\n      - name: a step naming no rule id\n        run: true\n' > "$D/uncited.yml"
expect "redirect/GATE_CITATION_WORKFLOW enforces per-job citation" 1 "cites no ADR rule id" \
  gate-citation "GATE_CITATION_WORKFLOW=$D/uncited.yml"

echo "== probe timeout (a timeout reaches no verdict) =="

# A probe killed at the wall clock must not read as "no violation". The bound
# is driven to a value the shim cannot meet, so the timeout path is exercised
# rather than asserted.
D=$(new_ws probe_timeout)
mkdir -p "$D/bin"
printf '#!/usr/bin/env bash\nsleep 300\n' > "$D/bin/cargo"
chmod +x "$D/bin/cargo"
expect "timeout/hung cargo probe fails closed" 1 "reached NO verdict" \
  async-trait "PATH=$D/bin:$PATH" "TRIPWIRE_PROBE_TIMEOUT_SECS=2"

expect "timeout/uninterpretable bound fails closed" 1 "is not a positive integer" \
  async-trait "TRIPWIRE_PROBE_TIMEOUT_SECS=not-a-number"

echo
echo "== scenario coverage (a harness that cannot detect its own deletion is fail-open) =="

COVERAGE_FAIL=0

while IFS= read -r want; do
  [ -n "$want" ] || continue
  seen=$(printf '%s' "$EXECUTED" | grep -cxF -- "$want" || true)
  if [ "$seen" -eq 0 ]; then
    echo "::error::tripwire-regression: required scenario '${want}' did NOT execute — a pinned gate scenario was deleted, renamed, or skipped"
    COVERAGE_FAIL=$((COVERAGE_FAIL + 1))
  elif [ "$seen" -ne 1 ]; then
    echo "::error::tripwire-regression: required scenario '${want}' executed ${seen} times — duplicate registration inflates the pass count without adding coverage"
    COVERAGE_FAIL=$((COVERAGE_FAIL + 1))
  fi
done <<EOF
$REQUIRED_SCENARIOS
EOF

while IFS= read -r ran; do
  [ -n "$ran" ] || continue
  if ! printf '%s' "$REQUIRED_SCENARIOS" | grep -qxF -- "$ran"; then
    echo "::error::tripwire-regression: scenario '${ran}' executed but is not in the required set — register it in REQUIRED_SCENARIOS so its deletion is detectable"
    COVERAGE_FAIL=$((COVERAGE_FAIL + 1))
  fi
done <<EOF
$EXECUTED
EOF

REQUIRED_COUNT=$(printf '%s' "$REQUIRED_SCENARIOS" | grep -c . || true)
if [ "$((PASS + FAIL))" -ne "$REQUIRED_COUNT" ]; then
  echo "::error::tripwire-regression: ${REQUIRED_COUNT} scenarios required, $((PASS + FAIL)) results recorded"
  COVERAGE_FAIL=$((COVERAGE_FAIL + 1))
fi

if [ "$COVERAGE_FAIL" -eq 0 ]; then
  echo "ok   all ${REQUIRED_COUNT} required scenarios executed exactly once"
fi

echo
echo "tripwire-regression: ${PASS} passed, ${FAIL} failed, ${COVERAGE_FAIL} coverage fault(s)"
if [ "$FAIL" -ne 0 ]; then
  echo "::error::tripwire-regression: ${FAIL} pinned gate scenario(s) no longer hold — a merge gate changed behaviour"
fi
if [ "$FAIL" -ne 0 ] || [ "$COVERAGE_FAIL" -ne 0 ]; then
  exit 1
fi
