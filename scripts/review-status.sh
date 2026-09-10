#!/usr/bin/env bash
# The agent review's hand on GitHub: which pull requests still need a review, and the commit
# status that records the verdict. The review itself is prompts/review-prs.md; the ruleset
# requires the status, so auto-merge waits for it.
#
#   scripts/review-status.sh --pending                 open PRs whose head commit has no
#                                                      review status yet, one per line:
#                                                      <number> <sha> <branch> <title>
#   scripts/review-status.sh <sha> pass|fail "<text>" [--url <url>]
#                                                      post the status for that commit (the
#                                                      text is its description, cut to 140
#                                                      characters; the url, if any, is where
#                                                      the findings are)
#   scripts/review-status.sh --self-test               a stub gh proves both
#
# The status context comes from .loop.toml (`review_context`, default "Agent review"). The
# owner's override for a red verdict is the same command with a reason:
#   scripts/review-status.sh <sha> pass "override: <reason>"
# Needs `gh` authenticated as someone who can write statuses on the repository.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
MODE=post
case "${1:-}" in
  --pending) MODE=pending ;;
  --self-test) MODE=selftest ;;
  "") echo "usage: scripts/review-status.sh --pending | <sha> pass|fail \"<text>\" [--url <url>] | --self-test" >&2; exit 2 ;;
esac

context() { "$CONFIG" review_context; }

pending() {
  local ctx number sha branch title
  ctx="$(context)"
  cd "$ROOT"
  gh pr list --state open --json number,headRefOid,headRefName,title \
    --jq '.[] | "\(.number)\t\(.headRefOid)\t\(.headRefName)\t\(.title)"' \
  | while IFS=$'\t' read -r number sha branch title; do
      [ -n "$sha" ] || continue
      if [ -z "$(gh api "repos/{owner}/{repo}/commits/$sha/status" --jq ".statuses[] | select(.context == \"$ctx\") | .state" 2>/dev/null)" ]; then
        printf '%s %s %s %s\n' "$number" "$sha" "$branch" "$title"
      fi
    done
}

post() {
  local sha="$1" verdict="$2" text="$3" url="${4:-}" state
  case "$verdict" in
    pass) state=success ;;
    fail) state=failure ;;
    *) echo "verdict must be pass or fail, not '$verdict'" >&2; return 2 ;;
  esac
  [ "${#text}" -le 140 ] || text="${text:0:137}..."
  cd "$ROOT"
  local -a args=(-X POST "repos/{owner}/{repo}/statuses/$sha" -f "state=$state" -f "context=$(context)" -f "description=$text")
  [ -n "$url" ] && args+=(-f "target_url=$url")
  gh api "${args[@]}" --jq '"\(.context): \(.state) on " + (.url | split("/statuses/")[1] | .[0:7])' 2>/dev/null \
    || gh api "${args[@]}" >/dev/null
  echo "review status: $state for ${sha:0:7} ($text)"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/review-status.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/review-status.sh" out
  mkdir -p "$dir/bin" "$dir/work"
  cat > "$dir/bin/gh" <<'EOF'
#!/usr/bin/env bash
# Stub gh: two open PRs; the first head already carries an "Agent review" status.
echo "$*" >> "$GH_LOG"
case "$1 $2" in
  "pr list") printf '11\taaaaaaa111\tticket/AA-01\tAA-01: first\n12\tbbbbbbb222\tbacklog/ideas\tBacklog: ideas\n' ;;
  "api repos/{owner}/{repo}/commits/aaaaaaa111/status") echo "success" ;;
  "api repos/{owner}/{repo}/commits/bbbbbbb222/status") ;;
  "api -X") echo '{"context":"Agent review","state":"success","url":"https://api.example/repos/o/r/statuses/bbbbbbb222"}' ;;
esac
EOF
  chmod +x "$dir/bin/gh"
  export PATH="$dir/bin:$PATH" GH_LOG="$dir/gh.log" LOOP_ROOT="$dir/work"
  # --pending lists only the head without a status, with number, sha, branch, and title.
  out="$("$me" --pending)"
  [ "$out" = "12 bbbbbbb222 backlog/ideas Backlog: ideas" ] || { echo "self-test: --pending should list PR 12 alone, got:"; echo "$out"; exit 1; }
  grep -q 'select(.context == "Agent review")' "$GH_LOG" || { echo "self-test: the default context should be Agent review:"; cat "$GH_LOG"; exit 1; }
  # pass posts success with the description; fail posts failure; a url becomes target_url.
  : > "$GH_LOG"
  out="$("$me" bbbbbbb222 pass "proof present; done line met")"
  grep -q -- '-X POST repos/{owner}/{repo}/statuses/bbbbbbb222 -f state=success -f context=Agent review -f description=proof present; done line met' "$GH_LOG" \
    || { echo "self-test: pass should post a success status:"; cat "$GH_LOG"; exit 1; }
  echo "$out" | grep -q 'review status: success for bbbbbbb' || { echo "self-test: pass should report what it posted:"; echo "$out"; exit 1; }
  : > "$GH_LOG"
  "$me" bbbbbbb222 fail "no test for the done line" --url https://example.invalid/pull/12#issuecomment-1 >/dev/null
  grep -q -- '-f state=failure -f context=Agent review -f description=no test for the done line -f target_url=https://example.invalid/pull/12#issuecomment-1' "$GH_LOG" \
    || { echo "self-test: fail should post failure with the url:"; cat "$GH_LOG"; exit 1; }
  # A long description is cut to 140 characters; a wrong verdict is refused.
  : > "$GH_LOG"
  long="$(printf 'x%.0s' $(seq 1 200))"
  "$me" bbbbbbb222 pass "$long" >/dev/null
  grep -oE 'description=x+\.\.\.' "$GH_LOG" | awk '{ if (length($0) - length("description=") == 140) exit 0; else exit 1 }' || { echo "self-test: the description should be cut to 140 characters"; exit 1; }
  if "$me" bbbbbbb222 maybe "?" >/dev/null 2>&1; then echo "self-test: a verdict other than pass/fail must be refused"; exit 1; fi
  # The context is configurable.
  printf '[loop]\nreview_context = "Robot review"\n' > "$dir/work/.loop.toml"
  : > "$GH_LOG"
  "$me" --pending >/dev/null
  grep -q 'select(.context == "Robot review")' "$GH_LOG" || { echo "self-test: review_context should change the context:"; cat "$GH_LOG"; exit 1; }
  unset LOOP_ROOT GH_LOG
  echo "review-status self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  pending) pending ;;
  post)
    SHA="$1"; VERDICT="${2:-}"; TEXT="${3:-}"; URL=""
    [ -n "$VERDICT" ] && [ -n "$TEXT" ] || { echo "usage: scripts/review-status.sh <sha> pass|fail \"<text>\" [--url <url>]" >&2; exit 2; }
    if [ "${4:-}" = "--url" ]; then URL="${5:-}"; fi
    post "$SHA" "$VERDICT" "$TEXT" "$URL"
    ;;
esac
