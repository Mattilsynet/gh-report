#!/usr/bin/env sh
set -eu

# Local BOUNDARY entrypoint (single host owner of the full-workspace verify).
# Prerequisites:
#   - GNU `timeout` on PATH (coreutils; not the macOS BSD `timeout`).
#   - Canonical Cargo aliases `atest` and `aclippy` in ~/.cargo/config.toml,
#     as documented by fleet authority (opencode AGENTS.md, "Cargo command
#     noise"): atest = test --quiet --no-fail-fast; aclippy = clippy --quiet
#     --message-format=short.
# The exact ledger and stale-prerequisite failure modes are exercised by
# tools/test_verify.py — an attended fixture that transiently replaces
# tools/tripwires.sh with a recording stub and must therefore run
# exclusively, never overlapping a real verification run.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "==> Running tripwires (tools/tripwires.sh all)..."
tools/tripwires.sh all

echo "==> Running cargo build..."
cargo build --workspace --all-features --locked

echo "==> Running workspace tests (timeout 900, no-fail-fast)..."
timeout 900 cargo atest --workspace --all-features --locked

echo "==> Running workspace clippy (all-targets)..."
cargo aclippy --workspace --all-targets --all-features --locked -- -D warnings

echo "==> Running cargo fmt check..."
cargo fmt --all -- --check

echo "==> All gh-report verification checks passed."
