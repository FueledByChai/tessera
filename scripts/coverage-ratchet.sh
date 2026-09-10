#!/usr/bin/env bash
# The coverage ratchet: test coverage may rise and may not fall. The project's own tool
# measures it; this script only compares the number with a checked-in floor.
#
#   scripts/coverage-ratchet.sh             run the coverage command and compare with the
#                                           floor (exit 1 below it; a pass above it names the
#                                           new floor to record)
#   scripts/coverage-ratchet.sh --set       run the command and write its number as the floor
#   scripts/coverage-ratchet.sh --self-test a fixture with a stub command proves every verdict
#
# Settings, from .loop.toml through scripts/loop-config.sh:
#   coverage        a command, run from the root, whose output ends in one percentage (the
#                   project's tool wrapped so that it prints the figure: cargo-llvm-cov,
#                   JaCoCo, coverage.py, whichever); empty: the ratchet is off
#   coverage_floor  the floor file (default coverage-floor.txt), one number, committed
#
# The last number in the command's output is the measurement, a `%` after it is ignored, and
# the comparison is exact: a ticket that raises coverage raises the floor in the same commit
# (`--set`), so the floor only ever moves up.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
MODE=check
case "${1:-}" in
  --set) MODE=set ;;
  --self-test) MODE=selftest ;;
  "") ;;
  *) echo "usage: scripts/coverage-ratchet.sh [--set|--self-test]" >&2; exit 2 ;;
esac

measure() {  # prints the last number the coverage command printed, or nothing
  local cmd="$1" out
  out="$(cd "$ROOT" && bash -c "$cmd" 2>&1)" || { echo "$out" >&2; return 1; }
  echo "$out" | { grep -oE '[0-9]+(\.[0-9]+)?[[:space:]]*%?' || true; } | tail -1 | tr -d '% \t'
}

ratchet() {
  local mode="$1" cmd floor_file measured floor
  cmd="$("$CONFIG" coverage)"
  floor_file="$ROOT/$("$CONFIG" coverage_floor)"
  if [ -z "$cmd" ]; then echo "coverage ratchet: off (no coverage command in .loop.toml)"; return 0; fi
  measured="$(measure "$cmd")" || { echo "coverage ratchet: the coverage command failed: $cmd" >&2; return 1; }
  if [ -z "$measured" ]; then echo "coverage ratchet: the coverage command printed no number: $cmd" >&2; return 1; fi
  if [ "$mode" = set ]; then
    echo "$measured" > "$floor_file"
    echo "coverage floor set to $measured% in ${floor_file#"$ROOT"/}"
    return 0
  fi
  if [ ! -f "$floor_file" ]; then
    echo "coverage ratchet: no floor yet (${floor_file#"$ROOT"/}); coverage is $measured%. Record it: scripts/coverage-ratchet.sh --set" >&2
    return 1
  fi
  floor="$(tr -d '% \n' < "$floor_file")"
  case "$(awk -v m="$measured" -v f="$floor" 'BEGIN { if (m < f) print "below"; else if (m > f) print "above"; else print "equal" }')" in
    below)
      echo "coverage ratchet: FAILED: coverage $measured% < floor $floor% (${floor_file#"$ROOT"/}). Add tests for what this change touched; the floor only moves up." >&2
      return 1 ;;
    above)
      echo "coverage $measured% >= floor $floor%; raise the floor to $measured: scripts/coverage-ratchet.sh --set" ;;
    equal)
      echo "coverage $measured% >= floor $floor%" ;;
  esac
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/coverage-ratchet.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/coverage-ratchet.sh" out rc
  export LOOP_ROOT="$dir"
  # The stub tool prints a report whose last line carries the figure.
  printf '[loop]\ncoverage = "cat report.txt"\n' > "$dir/.loop.toml"
  printf 'lines: 812/1140\nTOTAL 71.2%%\n' > "$dir/report.txt"
  # No floor yet: fails, names the number, says how to record it.
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'no floor yet (coverage-floor.txt); coverage is 71.2%' || { echo "self-test: a missing floor should fail with the hint (rc $rc):"; echo "$out"; exit 1; }
  # --set records it.
  out="$("$me" --set)"; [ "$(cat "$dir/coverage-floor.txt")" = "71.2" ] && echo "$out" | grep -q 'floor set to 71.2%' || { echo "self-test: --set should write the floor:"; echo "$out"; exit 1; }
  # Equal passes.
  out="$("$me")" && [ "$out" = "coverage 71.2% >= floor 71.2%" ] || { echo "self-test: equal should pass plainly:"; echo "$out"; exit 1; }
  # Above passes and names the new floor.
  printf 'TOTAL 73.0%%\n' > "$dir/report.txt"
  out="$("$me")" && echo "$out" | grep -q 'coverage 73.0% >= floor 71.2%; raise the floor to 73.0' || { echo "self-test: above should pass and name the new floor:"; echo "$out"; exit 1; }
  # Below fails naming both numbers.
  printf 'TOTAL 70.9%%\n' > "$dir/report.txt"
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'FAILED: coverage 70.9% < floor 71.2%' || { echo "self-test: below should fail naming both (rc $rc):"; echo "$out"; exit 1; }
  # A floor file with a percent sign and a newline still reads.
  printf '70.5%%\n' > "$dir/coverage-floor.txt"
  out="$("$me")" && echo "$out" | grep -q 'coverage 70.9% >= floor 70.5%' || { echo "self-test: a floor written with a percent sign should read:"; echo "$out"; exit 1; }
  # A command that prints no number fails with a message.
  printf '[loop]\ncoverage = "echo done"\n' > "$dir/.loop.toml"
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'printed no number' || { echo "self-test: no number should fail (rc $rc):"; echo "$out"; exit 1; }
  # A failing command fails, showing its output.
  printf '[loop]\ncoverage = "echo tool missing >&2; false"\n' > "$dir/.loop.toml"
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'tool missing' && echo "$out" | grep -q 'command failed' || { echo "self-test: a failing command should fail showing its output (rc $rc):"; echo "$out"; exit 1; }
  # Another floor file name; and no command at all means off.
  printf '[loop]\ncoverage = "cat report.txt"\ncoverage_floor = "ci/floor.txt"\n' > "$dir/.loop.toml"
  mkdir -p "$dir/ci"; "$me" --set >/dev/null; [ "$(cat "$dir/ci/floor.txt")" = "70.9" ] || { echo "self-test: coverage_floor should name the file"; exit 1; }
  printf '[loop]\n' > "$dir/.loop.toml"
  out="$("$me")" && echo "$out" | grep -q 'off (no coverage command' || { echo "self-test: no command should report off:"; echo "$out"; exit 1; }
  unset LOOP_ROOT
  echo "coverage-ratchet self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  set) ratchet set ;;
  check) ratchet check ;;
esac
