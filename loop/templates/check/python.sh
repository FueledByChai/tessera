#!/usr/bin/env bash
# The definition of done for a Python project, as a command: exits non-zero on the first
# failure. Written by grill-project from the kit's skeleton; every stack step is skipped
# with a note until pyproject.toml exists, so this passes on an empty repository and starts
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

if [ -f pyproject.toml ]; then
  # TODO: activate the project's environment first if the tools are not on the PATH.
  step "format"
  ruff format --check .
  step "lint"
  ruff check .
  step "types"
  # TODO: mypy or pyright, once the project chooses one.
  echo "skipped: no type checker chosen yet"
  step "tests"
  pytest -q
else
  step "stack"; echo "skipped: no pyproject.toml yet"
fi

if [ "$FAST" = 0 ]; then
  # `coverage` in .loop.toml should print one percentage; for Python:
  #   pytest -q --cov --cov-report=term 2>/dev/null | grep '^TOTAL' | awk '{print $NF}'
  ratchet
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
