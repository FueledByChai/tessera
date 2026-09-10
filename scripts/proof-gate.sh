#!/usr/bin/env bash
# The proof gate: a change to code must come with a change to a proof. Every ticket's done
# line names a test, fixture, or check; this is what makes a pull request that changed code
# without touching one fail instead of merging on green.
#
#   scripts/proof-gate.sh               judge origin/<default branch>...HEAD (exit 1 when a
#                                       code file changed and nothing counts as proof)
#   scripts/proof-gate.sh --self-test   a fixture repository proves every verdict
#
# Settings, from .loop.toml through scripts/loop-config.sh:
#   code_paths     globs (a file, a prefix, or a shell pattern) whose change needs a proof
#   proof_paths    globs that count as proof when changed
#   proof_pattern  a regex (grep -E); an added line matching it in any changed file counts as
#                  proof, so a new test function beside the code it tests qualifies
# With code_paths empty, or both proof_paths and proof_pattern empty, the gate is off.
#
# A commit body line `No new test: <reason>` anywhere in the range lets the change through;
# the reason is printed so it is visible in the check's output and in the PR.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
MODE=gate
case "${1:-}" in
  --self-test) MODE=selftest ;;
  "") ;;
  *) echo "usage: scripts/proof-gate.sh [--self-test]" >&2; exit 2 ;;
esac

read_globs() {  # $1 key; prints one glob per line, skipping blanks
  "$CONFIG" "$1" | sed '/^$/d'
}

matches_any() {  # $1 file, then globs
  local file="$1" glob; shift
  for glob in "$@"; do
    # shellcheck disable=SC2254
    case "$file" in $glob|$glob*|${glob%/}/*) return 0 ;; esac
  done
  return 1
}

gate() {
  cd "$ROOT"
  local base branch
  branch="$("$CONFIG" default_branch)"
  if git rev-parse -q --verify "refs/remotes/origin/$branch" >/dev/null 2>&1; then base="origin/$branch"; else base="$branch"; fi
  local -a code_globs=() proof_globs=()
  local glob pattern
  while IFS= read -r glob; do code_globs+=("$glob"); done < <(read_globs code_paths)
  while IFS= read -r glob; do proof_globs+=("$glob"); done < <(read_globs proof_paths)
  pattern="$("$CONFIG" proof_pattern)"
  if [ "${#code_globs[@]}" = 0 ]; then echo "proof gate: off (no code_paths in .loop.toml)"; return 0; fi
  if [ "${#proof_globs[@]}" = 0 ] && [ -z "$pattern" ]; then echo "proof gate: off (no proof_paths or proof_pattern in .loop.toml)"; return 0; fi
  local -a code=() proof=()
  local file
  # A file can be both: code that gains a test function beside it counts as its own proof.
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    if matches_any "$file" "${code_globs[@]}"; then code+=("$file"); fi
    if [ "${#proof_globs[@]}" -gt 0 ] && matches_any "$file" "${proof_globs[@]}"; then proof+=("$file (path)"); continue; fi
    if [ -n "$pattern" ] && git diff "$base...HEAD" -- "$file" | grep -v '^+++' | grep '^+' | grep -qE -- "$pattern"; then proof+=("$file (adds a line matching the pattern)"); fi
  done < <(git diff --name-only "$base...HEAD" 2>/dev/null || true)
  if [ "${#code[@]}" = 0 ]; then echo "proof gate: ok (no code change against $base)"; return 0; fi
  if [ "${#proof[@]}" -gt 0 ]; then
    echo "proof gate: ok (${#code[@]} code file(s) with proof: $(printf '%s; ' "${proof[@]}" | sed 's/; $//'))"
    return 0
  fi
  local reason
  reason="$(git log "$base..HEAD" --format=%B 2>/dev/null | grep -m1 '^No new test: ' | sed 's/^No new test: *//' || true)"
  if [ -n "$reason" ]; then echo "proof gate: skipped, the commit says why: No new test: $reason"; return 0; fi
  echo "proof gate: FAILED. Code changed with nothing that counts as proof:" >&2
  printf '  %s\n' "${code[@]}" >&2
  echo "  proof paths: ${proof_globs[*]:-(none)}; pattern: ${pattern:-(none)}" >&2
  echo "  Add the test, fixture, or check that proves the ticket's done line, or a line" >&2
  echo "  'No new test: <reason>' in the commit body if none is needed." >&2
  return 1
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/proof-gate.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/proof-gate.sh" out rc
  (
    cd "$dir"
    git init -q --bare origin.git
    git clone -q origin.git work 2>/dev/null
    cd work
    git config user.email t@example.com; git config user.name t
    git checkout -q -b main
    mkdir -p src tests docs
    echo "fn a() {}" > src/lib.rs; echo "fn t() {}" > tests/a.rs; echo "# d" > docs/README.md
    printf '[loop]\ncode_paths = ["src/"]\nproof_paths = ["tests/"]\nproof_pattern = "#\\\\[test\\\\]"\n' > .loop.toml
    git add -A; git commit -q -m "Scaffold"; git push -q -u origin main 2>/dev/null
  )
  export LOOP_ROOT="$dir/work"
  fresh() { (cd "$dir/work" && git checkout -q main && git reset -q --hard origin/main && git checkout -q -B case); }
  # 1. Code without proof: fails and names the file.
  fresh; (cd "$dir/work" && echo "fn b() {}" >> src/lib.rs && git commit -q -am "AA-01: add b")
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'FAILED' && echo "$out" | grep -q '^  src/lib.rs$' || { echo "self-test: code without proof should fail naming src/lib.rs (rc $rc):"; echo "$out"; exit 1; }
  # 2. Code plus a proof path: passes.
  (cd "$dir/work" && echo "fn t2() {}" >> tests/a.rs && git commit -q -am "AA-01: with a test file")
  out="$("$me")" || { echo "self-test: code with a proof path should pass:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'ok (1 code file(s) with proof: tests/a.rs (path))' || { echo "self-test: pass line should name the proof:"; echo "$out"; exit 1; }
  # 3. Code that adds a line matching the pattern in the same file: passes.
  fresh; (cd "$dir/work" && printf 'fn c() {}\n#[test]\nfn c_works() {}\n' >> src/lib.rs && git commit -q -am "AA-02: c with its test")
  out="$("$me")" || { echo "self-test: an added #[test] line should count as proof:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'src/lib.rs (adds a line matching the pattern)' || { echo "self-test: pass line should name the pattern match:"; echo "$out"; exit 1; }
  # 4. Code with the override line in the body: passes and prints the reason.
  fresh; (cd "$dir/work" && echo "// comment" >> src/lib.rs && git commit -q -am "AA-03: comment" -m "No new test: a comment only")
  out="$("$me")" || { echo "self-test: the override line should let it through:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'skipped, the commit says why: No new test: a comment only' || { echo "self-test: the reason should be printed:"; echo "$out"; exit 1; }
  # 5. A change outside code_paths: ok, no code change.
  fresh; (cd "$dir/work" && echo "more" >> docs/README.md && git commit -q -am "HK-01: docs")
  out="$("$me")" && echo "$out" | grep -q 'ok (no code change' || { echo "self-test: a docs change should pass as no code change:"; echo "$out"; exit 1; }
  # 6. Unset keys: the gate is off.
  fresh; (cd "$dir/work" && printf '[loop]\n' > .loop.toml && echo "fn d() {}" >> src/lib.rs && git commit -q -am "AA-04: no config")
  out="$("$me")" && echo "$out" | grep -q 'off (no code_paths' || { echo "self-test: unset keys should switch the gate off:"; echo "$out"; exit 1; }
  (cd "$dir/work" && printf '[loop]\ncode_paths = ["src/"]\n' > .loop.toml && git commit -q -am "AA-04: code paths only")
  out="$("$me")" && echo "$out" | grep -q 'off (no proof_paths or proof_pattern' || { echo "self-test: code_paths alone should switch the gate off:"; echo "$out"; exit 1; }
  unset LOOP_ROOT
  echo "proof-gate self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  gate) gate ;;
esac
