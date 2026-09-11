#!/usr/bin/env bash
# The proof gate: a change to code must come with a change to a proof. Every ticket's done
# line names a test, fixture, or check; this is what makes a pull request that changed code
# without touching one fail instead of merging on green.
#
#   scripts/proof-gate.sh               judge what is about to be committed: the working tree
#                                       (uncommitted and untracked files included) against
#                                       the merge base with origin/<default branch>; exit 1
#                                       when a code file changed and nothing counts as proof
#   scripts/proof-gate.sh --code-changed
#                                       the query alone: exit 0 listing the changed code
#                                       files, exit 1 with "no code change" when none is
#                                       under code_paths (exit 0 with "code_paths unset"
#                                       when the gate is off, so a caller assumes a change);
#                                       a check uses it to skip work that only code can move
#   scripts/proof-gate.sh --self-test   a fixture repository proves every verdict
#
# Settings, from .loop.toml through scripts/loop-config.sh:
#   code_paths     globs (a file, a prefix, or a shell pattern) whose change needs a proof
#   proof_paths    globs that count as proof when changed
#   proof_pattern  a regex (grep -E); an added line matching it in a changed code file counts
#                  as proof, so a new test function beside the code it tests qualifies (a
#                  docs or config file quoting the pattern does not)
# With code_paths empty, or both proof_paths and proof_pattern empty, the gate is off.
#
# A commit body line `No new test: <reason>` anywhere in the range lets the change through;
# the reason is printed so it is visible in the check's output and in the PR. Judging the
# working tree rather than the commits means a check run before the commit, the usual
# order, exercises the gate too; CI, with everything committed, sees the same answer.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
MODE=gate
case "${1:-}" in
  --self-test) MODE=selftest ;;
  --code-changed) MODE=query ;;
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

# The base a change is judged against: origin's default branch when it exists, else the
# local one.
base_ref() {
  local branch; branch="$("$CONFIG" default_branch)"
  if git rev-parse -q --verify "refs/remotes/origin/$branch" >/dev/null 2>&1; then echo "origin/$branch"; else echo "$branch"; fi
}

# The commit the working tree is compared with: where this branch left the base, so the
# base moving on does not count as our change.
merge_base() { git merge-base "$1" HEAD 2>/dev/null || echo "$1"; }

# Every file that differs from the merge base in the working tree: committed on this branch,
# modified but not committed, or untracked and not ignored.
changed_files() {
  { git diff --name-only "$1" 2>/dev/null; git ls-files --others --exclude-standard; } | sed '/^$/d' | sort -u
}

# The lines a file gained against the merge base; every line of an untracked file.
added_lines() {
  local mb="$1" file="$2"
  if git ls-files --error-unmatch -- "$file" >/dev/null 2>&1; then
    git diff "$mb" -- "$file" | grep -v '^+++' | grep '^+' || true
  else
    cat "$file" 2>/dev/null || true
  fi
}

# The query: did the range touch a code path? Prints the code files; exit 1 when none.
code_changed() {
  cd "$ROOT"
  local base mb glob file
  base="$(base_ref)"
  mb="$(merge_base "$base")"
  local -a code_globs=() code=()
  while IFS= read -r glob; do code_globs+=("$glob"); done < <(read_globs code_paths)
  if [ "${#code_globs[@]}" = 0 ]; then echo "code_paths unset: assuming a code change"; return 0; fi
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    if matches_any "$file" "${code_globs[@]}"; then code+=("$file"); fi
  done < <(changed_files "$mb")
  if [ "${#code[@]}" = 0 ]; then echo "no code change against $base"; return 1; fi
  printf '%s\n' "${code[@]}"
}

gate() {
  cd "$ROOT"
  local base mb
  base="$(base_ref)"
  mb="$(merge_base "$base")"
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
  # The pattern is looked for in code files only; a docs or config file quoting it is not proof.
  local is_code
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    is_code=0
    if matches_any "$file" "${code_globs[@]}"; then code+=("$file"); is_code=1; fi
    if [ "${#proof_globs[@]}" -gt 0 ] && matches_any "$file" "${proof_globs[@]}"; then proof+=("$file (path)"); continue; fi
    if [ "$is_code" = 1 ] && [ -n "$pattern" ] && added_lines "$mb" "$file" | grep -qE -- "$pattern"; then proof+=("$file (adds a line matching the pattern)"); fi
  done < <(changed_files "$mb")
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
  fresh() { (cd "$dir/work" && git checkout -q main && git reset -q --hard origin/main && git clean -fdq && git checkout -q -B case); }
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
  # 3b. A docs file that quotes the pattern is not proof for a code change.
  fresh; (cd "$dir/work" && echo "fn e() {}" >> src/lib.rs && printf 'Write tests with #[test].\n' >> docs/README.md && git commit -q -am "AA-05: e, docs mention tests")
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'FAILED' || { echo "self-test: a docs file quoting the pattern must not count as proof (rc $rc):"; echo "$out"; exit 1; }
  # 4. Code with the override line in the body: passes and prints the reason.
  fresh; (cd "$dir/work" && echo "// comment" >> src/lib.rs && git commit -q -am "AA-03: comment" -m "No new test: a comment only")
  out="$("$me")" || { echo "self-test: the override line should let it through:"; echo "$out"; exit 1; }
  echo "$out" | grep -q 'skipped, the commit says why: No new test: a comment only' || { echo "self-test: the reason should be printed:"; echo "$out"; exit 1; }
  # 5. A change outside code_paths: ok, no code change.
  fresh; (cd "$dir/work" && echo "more" >> docs/README.md && git commit -q -am "HK-01: docs")
  out="$("$me")" && echo "$out" | grep -q 'ok (no code change' || { echo "self-test: a docs change should pass as no code change:"; echo "$out"; exit 1; }
  # 5a. Uncommitted work is judged too (the check runs before the commit): a code change in
  #     the working tree with no proof fails before any commit; an untracked test file passes
  #     it; the query sees the uncommitted change as well; a base that moved on after the
  #     branch left it is not counted as ours.
  fresh; (cd "$dir/work" && echo "fn u() {}" >> src/lib.rs)
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q '^  src/lib.rs$' || { echo "self-test: an uncommitted code change with no proof should fail (rc $rc):"; echo "$out"; exit 1; }
  out="$("$me" --code-changed)" && [ "$out" = "src/lib.rs" ] || { echo "self-test: --code-changed should see the uncommitted change:"; echo "$out"; exit 1; }
  (cd "$dir/work" && echo "fn u_works() {}" > tests/u.rs)
  out="$("$me")" && echo "$out" | grep -q 'tests/u.rs (path)' || { echo "self-test: an untracked test file should count as proof:"; echo "$out"; exit 1; }
  (cd "$dir/work" && rm tests/u.rs && printf '#[test]\nfn u_works() {}\n' >> src/lib.rs)
  out="$("$me")" && echo "$out" | grep -q 'src/lib.rs (adds a line matching the pattern)' || { echo "self-test: an uncommitted test line should count as proof:"; echo "$out"; exit 1; }
  fresh; (cd "$dir/work" && git commit -q --allow-empty -m "AA-07: nothing yet")
  (cd "$dir" && git clone -q -b main origin.git mover 2>/dev/null && cd mover && git config user.email m@example.com && git config user.name m && echo "fn moved() {}" >> src/lib.rs && git commit -q -am "AA-08: base moves on" && git push -q origin main)
  (cd "$dir/work" && git fetch -q origin)
  rc=0; out="$("$me" --code-changed)" || rc=$?
  [ "$rc" = 1 ] || { echo "self-test: a base that moved on must not count as our code change (rc $rc):"; echo "$out"; exit 1; }
  (cd "$dir" && rm -rf mover)
  # 5b. The query: a code change lists the files; a docs change says no; unset keys assume.
  fresh; (cd "$dir/work" && echo "fn q() {}" >> src/lib.rs && git commit -q -am "AA-06: q")
  out="$("$me" --code-changed)" && [ "$out" = "src/lib.rs" ] || { echo "self-test: --code-changed should list src/lib.rs:"; echo "$out"; exit 1; }
  fresh; (cd "$dir/work" && echo "more" >> docs/README.md && git commit -q -am "HK-02: docs")
  rc=0; out="$("$me" --code-changed)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q '^no code change' || { echo "self-test: --code-changed should say no on a docs change (rc $rc):"; echo "$out"; exit 1; }
  fresh
  rc=0; out="$("$me" --code-changed)" || rc=$?
  [ "$rc" = 1 ] || { echo "self-test: --code-changed on the base itself should say no (rc $rc):"; echo "$out"; exit 1; }
  # 6. Unset keys: the gate is off.
  fresh; (cd "$dir/work" && printf '[loop]\n' > .loop.toml && echo "fn d() {}" >> src/lib.rs && git commit -q -am "AA-04: no config")
  out="$("$me")" && echo "$out" | grep -q 'off (no code_paths' || { echo "self-test: unset keys should switch the gate off:"; echo "$out"; exit 1; }
  (cd "$dir/work" && printf '[loop]\ncode_paths = ["src/"]\n' > .loop.toml && git commit -q -am "AA-04: code paths only")
  out="$("$me")" && echo "$out" | grep -q 'off (no proof_paths or proof_pattern' || { echo "self-test: code_paths alone should switch the gate off:"; echo "$out"; exit 1; }
  (cd "$dir/work" && printf '[loop]\n' > .loop.toml && git commit -q -am "AA-04: nothing set")
  out="$("$me" --code-changed)" && echo "$out" | grep -q 'code_paths unset: assuming' || { echo "self-test: --code-changed with code_paths unset should assume a change:"; echo "$out"; exit 1; }
  unset LOOP_ROOT
  echo "proof-gate self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  query) code_changed ;;
  gate) gate ;;
esac
