#!/usr/bin/env bash
# The definition of done for a Node or TypeScript project, as a command: exits non-zero on
# the first failure. Written by grill-project from the kit's skeleton; every stack step is
# skipped with a note until package.json exists, so this passes on an empty repository and
# starts failing as code arrives. Fill the TODO lines as the project decides them.
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

if [ -f package.json ]; then
  # The scripts below are the package.json names the project agrees to keep.
  step "typecheck"
  npm run --silent typecheck   # TODO: `tsc --noEmit` behind this name, or drop the step for plain JS
  step "lint"
  npm run --silent lint
  step "tests"
  npm test --silent
  step "build"
  npm run --silent build
else
  step "stack"; echo "skipped: no package.json yet"
fi

if [ "$FAST" = 0 ]; then
  # `coverage` in .loop.toml should print one percentage; with vitest:
  #   npx vitest run --coverage --coverage.reporter=text-summary 2>/dev/null | grep '^Lines' | grep -oE '[0-9.]+%'
  ratchet
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
