#!/usr/bin/env bash
# Local mirror of the CI checks (Format, Clippy, Test) from
# .github/workflows/ci.yml. Run this before every commit.
set -euo pipefail

step() {
    printf '==> %s\n' "$1"
}

step Format
cargo fmt --all -- --check

step Clippy
cargo clippy --all-targets --all-features -- -D warnings

step Test
cargo test --workspace

printf 'all checks passed\n'
