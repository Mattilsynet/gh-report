#!/usr/bin/env bash
# DEFERRED WIP — NOT ACTIVE (ghr-ktvy5 checkpoint 1c).
# These fixtures copy the five-group [workspace.lints.clippy] table out of the
# root Cargo.toml. That table was removed; the active baseline is pedantic only,
# so this harness no longer describes the current root and its assertions are
# vacuous or misleading until that table returns. No enforcement is added here.

# Proof fixtures for the native five-group Clippy policy.
#
# Test orchestration only. The fixture workspace copies the real
# [workspace.lints.clippy] table text out of the root Cargo.toml and lets
# native Cargo resolve inheritance, levels and priorities via
# `[lints] workspace = true`; no lint level, priority or flag is
# reconstructed here. Configuration is pinned explicitly with CLIPPY_CONF_DIR
# so the run does not depend on the caller's working directory.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
here="$root/tools/clippy-fixtures"
manifest="$root/Cargo.toml"
toolchain="$(awk -F'"' '/^channel/ {print $2}' "$root/rust-toolchain.toml")"
out="$root/.ooda/tmp/clippy-native-proof-09"
status=0

lint_table() {
  awk '
    /^\[workspace\.lints\.clippy\]/ { inside = 1; print; next }
    /^\[/ { inside = 0 }
    inside { print }
  ' "$manifest"
}

table="$(lint_table)"
if [ "$(printf '%s\n' "$table" | grep -c '=')" -lt 5 ]; then
  echo "FAIL harness: copied only $(printf '%s\n' "$table" | grep -c '=') lint entries from $manifest" >&2
  exit 1
fi

make_member() {
  local ws="$1" name="$2" src="$3"
  mkdir -p "$ws/$name/src"
  cp "$src" "$ws/$name/src/lib.rs"
  cat >"$ws/$name/Cargo.toml" <<EOF
[package]
name = "$name"
version = "0.0.0"
edition = "2024"

[lib]
path = "src/lib.rs"

[lints]
workspace = true
EOF
}

make_workspace() {
  local ws="$1" members="$2" drop="$3"
  rm -rf "$ws"
  mkdir -p "$ws"
  {
    echo "[workspace]"
    echo "resolver = \"3\""
    printf 'members = ['
    for m in $members; do printf '"%s", ' "$m"; done
    echo "]"
    echo
    if [ -n "$drop" ]; then
      printf '%s\n' "$table" | grep -Ev "^($drop) *=" 
    else
      printf '%s\n' "$table"
    fi
  } >"$ws/Cargo.toml"
  for m in $members; do
    make_member "$ws" "$m" "$here/$m.rs"
  done
  cp "$root/clippy.toml" "$ws/clippy.toml"
}

run_clippy() {
  local ws="$1" conf="$2" raw="$3"
  set +e
  CLIPPY_CONF_DIR="$conf" cargo "+$toolchain" clippy \
    --manifest-path "$ws/Cargo.toml" --all-targets --quiet \
    --target-dir "$ws/target" --message-format=json >"$raw" 2>"$raw.err"
  local rc=$?
  set -e
  if [ "$rc" -ne 0 ]; then
    echo "FAIL harness: cargo clippy exited $rc for $ws" >&2
    sed -n '1,20p' "$raw.err" >&2
    return 1
  fi
  if ! codes="$("$here/codes.sh" <"$raw")"; then
    echo "FAIL harness: diagnostic decoding failed for $ws" >&2
    return 1
  fi
  printf '%s\n' "$codes"
}

sites() {
  jq -r 'select(.reason == "compiler-message") | .message
         | select(.code != null)
         | . as $m | (.spans[]? | select(.is_primary))
         | [$m.code.code, .file_name, .line_start] | @tsv' "$1" | sort -u
}

excluded_lints="implicit_return exhaustive_structs exhaustive_enums
missing_const_for_fn arithmetic_side_effects float_arithmetic
absolute_paths str_to_string std_instead_of_core
missing_docs_in_private_items question_mark_used struct_excessive_bools"

retained_lints="panic_in_result_fn unwrap_in_result let_underscore_must_use
unused_result_ok map_err_ignore wildcard_enum_match_arm
indexing_slicing expect_used unwrap_used panic"

ws_main="$out/ws-main"
make_workspace "$ws_main" "excluded retained tests_ctl" ""
codes_main="$(run_clippy "$ws_main" "$ws_main" "$out/main.json")"
sites_main="$(sites "$out/main.json")"

for lint in $excluded_lints; do
  if grep -qx "clippy::$lint" <<<"$codes_main"; then
    echo "FAIL excluded: clippy::$lint fired but is allowed at priority 0" >&2
    status=1
  else
    echo "ok   excluded: clippy::$lint silent under the real inherited table"
  fi
done

for lint in $retained_lints; do
  if grep -qx "clippy::$lint" <<<"$codes_main"; then
    echo "ok   retained: clippy::$lint fires"
  else
    echo "FAIL retained: clippy::$lint did not fire on its fixture" >&2
    status=1
  fi
done

drop_alt="$(printf '%s ' $excluded_lints | sed 's/ $//' | tr ' ' '|')"
ws_ctl="$out/ws-control"
make_workspace "$ws_ctl" "excluded" "$drop_alt"
codes_ctl="$(run_clippy "$ws_ctl" "$ws_ctl" "$out/control.json")"

for lint in $excluded_lints; do
  if grep -qx "clippy::$lint" <<<"$codes_ctl"; then
    echo "ok   control: clippy::$lint fires once its allow entry is removed"
  else
    echo "FAIL control: clippy::$lint has no live witness (silence was vacuous)" >&2
    status=1
  fi
done

site_line() { printf 'clippy::%s\ttests_ctl/src/lib.rs\t%s' "$1" "$2"; }
ctl_line() { awk -v pat="$1" 'index($0, pat) {print NR; exit}' "$here/tests_ctl.rs"; }

conf_off="$out/conf-off"
mkdir -p "$conf_off"
sed 's/= true/= false/' "$root/clippy.toml" >"$conf_off/clippy.toml"
run_clippy "$ws_main" "$conf_off" "$out/off.json" >/dev/null
sites_off="$(sites "$out/off.json")"

ctl_specs=$(cat <<'SPECS'
unwrap_used|value.unwrap() }|let taken = value.unwrap();
expect_used|value.expect("helper")|value.expect("test")
panic|panic!("helper")|panic!("test")
indexing_slicing|values[0]|numbers[0]
SPECS
)

while IFS='|' read -r lint hpat tpat; do
  [ -n "$lint" ] || continue
  hline="$(ctl_line "$hpat")"
  tline="$(ctl_line "$tpat")"
  if [ -z "$hline" ] || [ -z "$tline" ]; then
    echo "FAIL config: fixture markers missing for clippy::$lint" >&2
    status=1
    continue
  fi
  if grep -qxF "$(site_line "$lint" "$hline")" <<<"$sites_main"; then
    echo "ok   config: helper clippy::$lint fires at tests_ctl.rs:$hline"
  else
    echo "FAIL config: helper clippy::$lint did not fire at tests_ctl.rs:$hline" >&2
    status=1
  fi
  if grep -qxF "$(site_line "$lint" "$tline")" <<<"$sites_main"; then
    echo "FAIL config: recognized #[test] clippy::$lint fired despite its allow-in-tests setting" >&2
    status=1
  else
    echo "ok   config: recognized #[test] clippy::$lint silent under root clippy.toml"
  fi
  if grep -qxF "$(site_line "$lint" "$tline")" <<<"$sites_off"; then
    echo "ok   config-off: recognized #[test] clippy::$lint fires once the setting is false"
  else
    echo "FAIL config-off: clippy::$lint setting has no observable effect" >&2
    status=1
  fi
  if grep -qxF "$(site_line "$lint" "$hline")" <<<"$sites_off"; then
    echo "ok   config-off: helper clippy::$lint still fires"
  else
    echo "FAIL config-off: helper clippy::$lint lost" >&2
    status=1
  fi
done <<<"$ctl_specs"

real_cargo="$(command -v cargo)"
mkdir -p "$out/fakebin"
inject_probe() {
  local label="$1" tail="$2"
  {
    printf '#!/usr/bin/env bash\n'
    printf '"%s" "$@"\n' "$real_cargo"
    if [ -n "$tail" ]; then
      printf "printf '%%s' '%s'\n" "$tail"
    fi
    printf 'exit 101\n'
  } >"$out/fakebin/cargo"
  chmod +x "$out/fakebin/cargo"
  if ( PATH="$out/fakebin:$PATH"; run_clippy "$ws_main" "$ws_main" "$out/inject-$label.json" ) >/dev/null 2>&1; then
    echo "FAIL injection($label): harness accepted a cargo run that exited 101" >&2
    status=1
  else
    echo "ok   injection($label): cargo exit 101 is rejected"
  fi
}
inject_probe bare ''
inject_probe error-appended '{"reason":"compiler-message","message":{"level":"error","code":{"code":"E0425"},"spans":[],"children":[],"rendered":"error"}}
{"reason":"build-finished","success":false}
'
if "$here/codes.sh" <"$out/inject-error-appended.json" >/dev/null 2>&1; then
  echo "FAIL injection: decoder accepted an injected compiler error and failed build" >&2
  status=1
else
  echo "ok   injection: decoder rejects injected E0425 plus build-finished success:false"
fi

if printf 'not json at all\n' | "$here/codes.sh" >/dev/null 2>&1; then
  echo "FAIL decoder: malformed diagnostics decoded as silence" >&2
  status=1
else
  echo "ok   decoder: malformed diagnostics are an error, not silence"
fi
if printf '{"reason":"compiler-message","message":"wrong shape"}\n' | "$here/codes.sh" >/dev/null 2>&1; then
  echo "FAIL decoder: wrong-shaped compiler-message decoded as silence" >&2
  status=1
else
  echo "ok   decoder: wrong-shaped compiler-message is an error"
fi

evo="$out/evolution"
rm -rf "$evo"
mkdir -p "$evo/src"
cat >"$evo/Cargo.toml" <<'EOF'
[package]
name = "evolution"
version = "0.0.0"
edition = "2024"

[workspace]
EOF
evo_source() {
  local extra_variant="$1" extra_arm="$2"
  {
    printf 'pub enum Outcome {\n    Accepted,\n    Rejected,\n'
    [ -n "$extra_variant" ] && printf '    %s,\n' "$extra_variant"
    printf '}\n\npub fn describe(outcome: &Outcome) -> &'"'"'static str {\n    match outcome {\n'
    printf '        Outcome::Accepted => "accepted",\n        Outcome::Rejected => "rejected",\n'
    [ -n "$extra_arm" ] && printf '        Outcome::%s => "deferred",\n' "$extra_arm"
    printf '    }\n}\n'
  } >"$evo/src/lib.rs"
}
evo_source '' ''
evo_check() {
  set +e
  cargo "+$toolchain" check --manifest-path "$evo/Cargo.toml" --quiet \
    --target-dir "$evo/target" --message-format=json >"$evo/check.json" 2>/dev/null
  local rc=$?
  set -e
  echo "$rc"
}
if [ "$(evo_check)" -ne 0 ]; then
  echo "FAIL evolution: base consumer does not compile" >&2
  status=1
else
  echo "ok   evolution: base exhaustive consumer compiles"
fi
evo_source Deferred ''
if [ "$(evo_check)" -ne 0 ] && grep -q '"code":"E0004"' "$evo/check.json"; then
  echo "ok   evolution: added variant breaks the exhaustive consumer with E0004"
else
  echo "FAIL evolution: added enum variant did not produce E0004" >&2
  status=1
fi
evo_source Deferred Deferred
if [ "$(evo_check)" -eq 0 ]; then
  echo "ok   evolution: repaired consumer compiles again"
else
  echo "FAIL evolution: repaired consumer still fails" >&2
  status=1
fi

exit "$status"
