#!/usr/bin/env bash
# The definition of done for a project whose stack the kit has no skeleton for, as a command:
# exits non-zero on the first failure. Written by grill-project; the loop's own checks run,
# and every stack step is a TODO that the project's first ticket fills in.
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

step "format"; echo "TODO: the formatter's check command"
step "lint";   echo "TODO: the linter"
step "tests";  echo "TODO: the test runner (fails the check when a test fails)"
step "build";  echo "TODO: the build"

if [ "$FAST" = 0 ]; then
  # TODO: `coverage` in .loop.toml, a command that prints one percentage, switches the ratchet on.
  ratchet
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
