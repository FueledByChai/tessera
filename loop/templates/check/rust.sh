#!/usr/bin/env bash
# The definition of done for a Rust project, as a command: exits non-zero on the first
# failure. Written by grill-project from the kit's skeleton; every stack step is skipped
# with a note until Cargo.toml exists, so this passes on an empty repository and starts
# failing as code arrives. Fill the TODO lines as the project decides them.
#
#   scripts/check.sh            everything
#   scripts/check.sh --fast     skip the coverage ratchet (the check to run while iterating)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
FAST=0
for arg in "$@"; do case "$arg" in --fast) FAST=1 ;; *) echo "unknown flag: $arg" >&2; exit 2 ;; esac; done
step() { printf '\n== %s\n' "$1"; }
started=$(date +%s)
. loop/templates/check/common.sh
loop_checks

if [ -f Cargo.toml ]; then
  step "format"
  cargo fmt --all --check
  step "lint"
  cargo clippy --all-targets --quiet -- -D warnings
  step "tests"
  cargo test --quiet
  step "build"
  cargo build --release --quiet
  # TODO: a parity or golden-output step, if results must not drift (see the kit README).
else
  step "stack"; echo "skipped: no Cargo.toml yet"
fi

if [ "$FAST" = 0 ]; then
  # `coverage` in .loop.toml should print one percentage; for Rust:
  #   cargo llvm-cov --json --summary-only | python3 -c 'import json,sys; print("%.1f" % json.load(sys.stdin)["data"][0]["totals"]["lines"]["percent"])'
  # (needs `rustup component add llvm-tools-preview` and `cargo install cargo-llvm-cov`).
  ratchet
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
