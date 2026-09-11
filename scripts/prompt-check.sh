#!/usr/bin/env bash
# A prompt cannot be unit-tested, but the rules it carries can be asserted: this check fails
# when a prompt no longer contains a phrase that states one of its rules, so an edit that
# drops a rule fails the check instead of silently changing how agents behave. The other
# half of a prompt's proof is a recorded real run in the pull request that changed it.
#
#   scripts/prompt-check.sh             check every prompt in loop/prompts (a project) or
#                                       prompts (the kit itself)
#   scripts/prompt-check.sh --self-test a copy with a phrase removed fails; intact copies pass
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"

# prompt file<TAB>phrase, one rule per line. A phrase is matched case-insensitively as a
# fixed string with the prompt's whitespace collapsed, so a line break inside it is fine.
RULES='next-ticket.md	Done when
next-ticket.md	--claim
next-ticket.md	Never push the default branch
grill-me.md	at least three rounds
grill-me.md	proofs and edge cases
grill-me.md	docs/decisions
grill-me.md	superseding
grill-me.md	without a named proof
grill-me.md	wireframe
grill-me.md	80 columns
review-prs.md	four questions
review-prs.md	one status per head commit
grill-project.md	in a sentence
grill-project.md	Who and where
grill-project.md	The data
grill-project.md	Runtime and deploy
grill-project.md	The UI
grill-project.md	Non-negotiables
grill-project.md	dated deferral
grill-project.md	rust.sh
grill-project.md	python.sh
grill-project.md	node.sh
grill-project.md	java.sh
grill-project.md	go.sh
grill-project.md	wireframe'

prompts_dir() {
  if [ -d "$ROOT/loop/prompts" ]; then echo "$ROOT/loop/prompts"
  elif [ -d "$ROOT/prompts" ]; then echo "$ROOT/prompts"
  else echo "prompt-check: no prompts directory under $ROOT (loop/prompts or prompts)" >&2; return 1; fi
}

check() {
  local d file phrase failed=0 checked=0
  d="$(prompts_dir)"
  while IFS=$'\t' read -r file phrase; do
    [ -n "$file" ] || continue
    if [ ! -f "$d/$file" ]; then echo "prompt-check: $file is missing from ${d#"$ROOT"/}" >&2; failed=1; continue; fi
    if tr -s '[:space:]' ' ' < "$d/$file" | grep -qiF -- "$phrase"; then checked=$((checked + 1)); else echo "prompt-check: $file no longer says \"$phrase\"" >&2; failed=1; fi
  done <<< "$RULES"
  [ "$failed" = 0 ] || { echo "prompt-check: a prompt lost a rule it must carry (see above)" >&2; return 1; }
  echo "prompt-check: $checked rule(s) present in ${d#"$ROOT"/}"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/prompt-check.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/prompt-check.sh" src out rc
  src="$(prompts_dir)"
  mkdir -p "$dir/loop/prompts"
  cp "$src"/*.md "$dir/loop/prompts/"
  export LOOP_ROOT="$dir"
  out="$("$me")" && echo "$out" | grep -q 'rule(s) present' || { echo "self-test: intact copies should pass:"; echo "$out"; exit 1; }
  # Remove one phrase: the check fails and names the file and the phrase.
  sed -i.bak 's/at least three rounds/some rounds/' "$dir/loop/prompts/grill-me.md"; rm -f "$dir/loop/prompts/grill-me.md.bak"
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'grill-me.md no longer says "at least three rounds"' || { echo "self-test: a removed phrase should fail naming it (rc $rc):"; echo "$out"; exit 1; }
  # A missing prompt file fails too.
  cp "$src/grill-me.md" "$dir/loop/prompts/grill-me.md"; rm "$dir/loop/prompts/review-prs.md"
  rc=0; out="$("$me" 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'review-prs.md is missing' || { echo "self-test: a missing prompt should fail (rc $rc):"; echo "$out"; exit 1; }
  unset LOOP_ROOT
  echo "prompt-check self-test passed"
}

case "${1:-}" in
  --self-test) self_test ;;
  "") check ;;
  *) echo "usage: scripts/prompt-check.sh [--self-test]" >&2; exit 2 ;;
esac
