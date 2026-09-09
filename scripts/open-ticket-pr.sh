#!/usr/bin/env bash
# The loop's hand-off through pull requests (HK-09). A ticket's branch on origin is
# `ticket/<id>`; its presence is the claim that scripts/backlog-status.sh --next honours, and
# the pull request from it is what the owner merges. The loop never pushes main.
#
#   scripts/open-ticket-pr.sh <id> --claim          push HEAD as origin/ticket/<id> before work
#                                                   starts (refuses if the branch already exists:
#                                                   someone else holds the claim)
#   scripts/open-ticket-pr.sh <id> [--body-file f] [--draft]
#                                                   push HEAD to origin/ticket/<id> and open the
#                                                   PR against main when there is none: title
#                                                   from HEAD's subject (which must start with
#                                                   `<id>:`), body from the file or HEAD's message
#                                                   body, plus a footer naming the checks
#                                                   Then the merge policy (HK-10): the PR is set
#                                                   to auto-merge (rebase) once CI is green on a
#                                                   branch up to date with main, unless it touches
#                                                   examples/expected (a parity baseline
#                                                   refresh), which waits for the owner's review
#   scripts/open-ticket-pr.sh <id> --update         rebase the PR onto main after main moved
#                                                   (the rules want checks on the exact result),
#                                                   so CI reruns and auto-merge can fire
#   scripts/open-ticket-pr.sh <id> --status         the PR's url, state, merge state, and checks
#   scripts/open-ticket-pr.sh <id> --release        delete origin/ticket/<id> after the PR merged
#                                                   or the claim is abandoned
#
# Needs `gh` installed and authenticated (`gh auth login`).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ID="${1:-}"
[ -n "$ID" ] || { echo "usage: scripts/open-ticket-pr.sh <id> [--claim|--status|--release|--body-file f|--draft]" >&2; exit 2; }
shift
MODE=open
BODY_FILE=""
DRAFT=0
while [ $# -gt 0 ]; do
  case "$1" in
    --claim) MODE=claim ;;
    --status) MODE=status ;;
    --update) MODE=update ;;
    --release) MODE=release ;;
    --body-file) BODY_FILE="$2"; shift ;;
    --draft) DRAFT=1 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done
BRANCH="ticket/$ID"
cd "$ROOT"

remote_has_branch() { git ls-remote --exit-code --heads origin "refs/heads/$BRANCH" >/dev/null 2>&1; }

# The merge policy (HK-10): a green PR that is up to date with main merges on its own; a PR
# that changes the parity baseline (examples/expected) is the one kind that waits for the
# owner's review.
apply_merge_policy() {
  local url="$1"
  if git diff --name-only "origin/main...HEAD" 2>/dev/null | grep -q '^examples/expected/'; then
    gh pr edit "$url" --add-label "needs-review" >/dev/null 2>&1 || true
    echo "merge policy: examples/expected changed, so this PR waits for the owner's review (no auto-merge)"
  else
    if gh pr merge "$url" --auto --rebase >/dev/null 2>&1; then
      echo "merge policy: auto-merge on; it merges once CI is green on a branch up to date with main (--update after main moves)"
    else
      echo "merge policy: could not enable auto-merge (no rules on main?); merge it by hand when green"
    fi
  fi
}
need_gh() { command -v gh >/dev/null || { echo "gh is not installed (brew install gh && gh auth login)" >&2; exit 1; }; }

case "$MODE" in
  claim)
    if remote_has_branch; then
      echo "claim refused: origin/$BRANCH already exists; someone holds $ID" >&2
      exit 1
    fi
    git push -q origin "HEAD:refs/heads/$BRANCH"
    echo "claimed $ID: origin/$BRANCH at $(git rev-parse --short HEAD)"
    ;;
  open)
    need_gh
    subject="$(git log -1 --format=%s)"
    case "$subject" in
      "$ID: "*) ;;
      *) echo "HEAD's subject must start with '$ID: ' (it is: $subject)" >&2; exit 1 ;;
    esac
    git push -q origin "HEAD:refs/heads/$BRANCH"
    echo "pushed origin/$BRANCH at $(git rev-parse --short HEAD)"
    existing="$(gh pr list --head "$BRANCH" --base main --state open --json url --jq '.[0].url' 2>/dev/null || true)"
    if [ -n "$existing" ]; then
      echo "pull request already open: $existing"
      exit 0
    fi
    body="$(mktemp "${TMPDIR:-/tmp}/tessera-pr-body.XXXXXX")"
    trap 'rm -f "$body"' EXIT
    if [ -n "$BODY_FILE" ]; then
      cat "$BODY_FILE" > "$body"
    else
      git log -1 --format=%b | sed '/^Co-Authored-By:/d' > "$body"
    fi
    {
      printf '\n---\n'
      printf 'Ticket %s. Checks on this PR: the CI engine job (`scripts/check.sh --quick`: fmt, tests, build, example parity) and the web job (`scripts/check.sh --web-only` against a scratch console: typecheck, lint, theme, layout, and chart checks). The private checks ran locally before the push.\n' "$ID"
      printf '\n🤖 Generated with [Claude Code](https://claude.com/claude-code)\n'
    } >> "$body"
    args=(--base main --head "$BRANCH" --title "$subject" --body-file "$body")
    [ "$DRAFT" = 1 ] && args+=(--draft)
    url="$(gh pr create "${args[@]}")"
    echo "$url"
    apply_merge_policy "$url"
    ;;
  update)
    need_gh
    gh pr update-branch "$BRANCH" --rebase && echo "rebased origin/$BRANCH onto main; CI reruns and auto-merge fires when green"
    ;;
  status)
    need_gh
    gh pr list --head "$BRANCH" --base main --state all --json url,state,title,mergeStateStatus,autoMergeRequest --jq '.[] | "\(.state)\t\(.mergeStateStatus)\tauto-merge \(if .autoMergeRequest then "on" else "off" end)\t\(.url)\t\(.title)"' | head -3
    gh pr checks "$BRANCH" 2>/dev/null || true
    ;;
  release)
    if remote_has_branch; then
      git push -q origin --delete "$BRANCH"
      echo "released $ID: origin/$BRANCH deleted"
    else
      echo "nothing to release: origin/$BRANCH does not exist"
    fi
    ;;
esac
