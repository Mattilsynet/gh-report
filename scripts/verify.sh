#!/usr/bin/env sh
set -eu

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
