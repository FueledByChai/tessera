#!/usr/bin/env bash
# The definition of done for a Go project, as a command: exits non-zero on the first failure.
# Written by grill-project from the kit's skeleton; every stack step is skipped with a note
# until go.mod exists, so this passes on an empty repository and starts failing as code
# arrives. Fill the TODO lines as the project decides them.
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

if [ -f go.mod ]; then
  step "format"
  test -z "$(gofmt -l .)" || { gofmt -l .; echo "gofmt: the files above are not formatted" >&2; exit 1; }
  step "vet"
  go vet ./...
  step "tests"
  go test ./...
  step "build"
  go build ./...
else
  step "stack"; echo "skipped: no go.mod yet"
fi

if [ "$FAST" = 0 ]; then
  # `coverage` in .loop.toml should print one percentage; for Go:
  #   go test ./... -coverprofile=/tmp/cover.out >/dev/null && go tool cover -func=/tmp/cover.out | tail -1 | grep -oE '[0-9.]+%'
  ratchet
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
