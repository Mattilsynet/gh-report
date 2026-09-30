#!/usr/bin/env sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

echo "==> Running tripwires (tools/tripwires.sh all)..."
tools/tripwires.sh all

echo "==> Running cargo test..."
cargo test --workspace --all-features --locked

echo "==> Running cargo clippy..."
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

echo "==> Running cargo fmt check..."
cargo fmt --all -- --check

echo "==> All gh-report verification checks passed."
