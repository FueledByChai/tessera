#!/usr/bin/env bash
# The loop's hand-off through pull requests (HK-09). A ticket's branch on origin is
# `ticket/<id>`; its presence is the claim that scripts/backlog-status.sh --next honours, and
# the pull request from it is what the owner merges. The loop never pushes the default branch.
#
#   scripts/open-ticket-pr.sh <id> --claim          push HEAD as origin/ticket/<id> before work
#                                                   starts (refuses if the branch already exists:
#                                                   someone else holds the claim)
#   scripts/open-ticket-pr.sh <id> [--body-file f] [--draft]
#                                                   push HEAD to origin/ticket/<id> and open the
#                                                   PR against the default branch when there is
#                                                   none: title from HEAD's subject (which must
#                                                   start with `<id>:`), body from the file or
#                                                   HEAD's message body, plus a footer naming
#                                                   the checks. Then the merge policy (HK-10):
#                                                   the PR is set to auto-merge (rebase) once CI
#                                                   is green on a branch up to date with the
#                                                   default branch, unless it touches one of the
#                                                   review paths (the changes the checks cannot
#                                                   judge, such as a frozen baseline), which
#                                                   labels it needs-review for the owner
#   scripts/open-ticket-pr.sh <id> --update         rebase the PR onto the default branch after
#                                                   it moved (the rules want checks on the exact
#                                                   result), so CI reruns and auto-merge can fire
#   scripts/open-ticket-pr.sh <id> --status         the PR's url, state, merge state, and checks
#   scripts/open-ticket-pr.sh <id> --release        delete origin/ticket/<id> after the PR merged
#                                                   or the claim is abandoned
#   scripts/open-ticket-pr.sh --self-test           a fixture repo with a stub `gh` proves the
#                                                   claim, the trailer rule, and the review-path
#                                                   policy
#
# Settings come from .loop.toml through scripts/loop-config.sh (HK-14): `default_branch`,
# `review_paths` (globs or path prefixes), `trailer_required`, `check`. LOOP_ROOT points at
# another checkout (the self-test's fixture). Needs `gh` installed and authenticated.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
ID="${1:-}"
[ -n "$ID" ] || { echo "usage: scripts/open-ticket-pr.sh <id> [--claim|--status|--update|--release|--body-file f|--draft] | --self-test" >&2; exit 2; }
shift
MODE=open
BODY_FILE=""
DRAFT=0
[ "$ID" = "--self-test" ] && MODE=selftest
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

remote_has_branch() { git ls-remote --exit-code --heads origin "refs/heads/$BRANCH" >/dev/null 2>&1; }
need_gh() { command -v gh >/dev/null || { echo "gh is not installed (brew install gh && gh auth login)" >&2; exit 1; }; }

# The changed files (against origin/<default branch>) that fall under a review path, one per
# line as `<glob>: <file>`; empty when none does. A review path matches a file it names, a file
# under it (a prefix), or a file its shell pattern matches.
review_hits() {
  local base="$1" glob file
  local -a globs=()
  while IFS= read -r glob; do [ -n "$glob" ] && globs+=("$glob"); done < <("$CONFIG" review_paths)
  [ "${#globs[@]}" -gt 0 ] || return 0
  while IFS= read -r file; do
    for glob in "${globs[@]}"; do
      # shellcheck disable=SC2254
      case "$file" in $glob|$glob*|${glob%/}/*) echo "$glob: $file"; break ;; esac
    done
  done < <(git diff --name-only "origin/$base...HEAD" 2>/dev/null || true)
}

# The merge policy (HK-10): a green PR that is up to date with the default branch merges on its
# own; a PR that touches a review path is the one kind that waits for the owner's review.
apply_merge_policy() {
  local url="$1" base="$2" hits
  hits="$(review_hits "$base")"
  if [ -n "$hits" ]; then
    gh pr edit "$url" --add-label "needs-review" >/dev/null 2>&1 || true
    echo "merge policy: a review path changed, so this PR waits for the owner's review (no auto-merge):"
    echo "$hits" | sed 's/^/  /'
  else
    if gh pr merge "$url" --auto --rebase >/dev/null 2>&1; then
      echo "merge policy: auto-merge on; it merges once CI is green on a branch up to date with $base (--update after $base moves)"
    else
      echo "merge policy: could not enable auto-merge (no rules on $base?); merge it by hand when green"
    fi
  fi
}

# The commit's agent trailer (`Co-Authored-By: <name> <email>`), or nothing.
head_trailer() { git log -1 --format=%B | { grep -i '^Co-Authored-By:' || true; } | head -1 | sed 's/^[Cc]o-[Aa]uthored-[Bb]y: *//'; }

run() {
  local base
  cd "$ROOT"
  base="$("$CONFIG" default_branch)"
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
      local subject trailer existing url
      subject="$(git log -1 --format=%s)"
      case "$subject" in
        "$ID: "*) ;;
        *) echo "HEAD's subject must start with '$ID: ' (it is: $subject)" >&2; exit 1 ;;
      esac
      trailer="$(head_trailer)"
      if [ "$("$CONFIG" trailer_required)" = "true" ] && [ -z "$trailer" ]; then
        echo "HEAD's message must end with a 'Co-Authored-By: <agent> <email>' trailer naming the agent that did the work (.loop.toml trailer_required)" >&2
        exit 1
      fi
      git push -q origin "HEAD:refs/heads/$BRANCH"
      echo "pushed origin/$BRANCH at $(git rev-parse --short HEAD)"
      existing="$(gh pr list --head "$BRANCH" --base "$base" --state open --json url --jq '.[0].url' 2>/dev/null || true)"
      if [ -n "$existing" ]; then
        echo "pull request already open: $existing"
        exit 0
      fi
      BODY_TMP="$(mktemp "${TMPDIR:-/tmp}/loop-pr-body.XXXXXX")"
      trap 'rm -f "${BODY_TMP:-}"' EXIT
      local body="$BODY_TMP"
      if [ -n "$BODY_FILE" ]; then
        cat "$BODY_FILE" > "$body"
      else
        git log -1 --format=%b | sed '/^[Cc]o-[Aa]uthored-[Bb]y:/d' > "$body"
      fi
      {
        printf '\n---\n'
        printf 'Ticket %s. The checks on this PR are the CI jobs required on `%s`; `%s` ran locally before the push.\n' "$ID" "$base" "$("$CONFIG" check)"
        if [ -n "$trailer" ]; then printf '\nWorked by %s.\n' "$(echo "$trailer" | sed 's/ *<.*//')"; fi
      } >> "$body"
      local -a args=(--base "$base" --head "$BRANCH" --title "$subject" --body-file "$body")
      [ "$DRAFT" = 1 ] && args+=(--draft)
      url="$(gh pr create "${args[@]}")"
      echo "$url"
      apply_merge_policy "$url" "$base"
      ;;
    update)
      need_gh
      gh pr update-branch "$BRANCH" --rebase && echo "rebased origin/$BRANCH onto $base; CI reruns and auto-merge fires when green"
      ;;
    status)
      need_gh
      gh pr list --head "$BRANCH" --base "$base" --state all --json url,state,title,mergeStateStatus,autoMergeRequest --jq '.[] | "\(.state)\t\(.mergeStateStatus)\tauto-merge \(if .autoMergeRequest then "on" else "off" end)\t\(.url)\t\(.title)"' | head -3
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
}

# A fixture: a bare origin, a clone with a .loop.toml whose review path is fixtures/expected/,
# and a stub `gh` on PATH that records every call. Proves: --claim pushes and then refuses; a
# commit without the trailer is refused while trailer_required is true and accepted when it is
# false; a PR that touches the review path is labelled needs-review and not auto-merged; one
# that does not is set to auto-merge.
self_test() {
  # LOOP_TRACE=1 traces the fixture steps when one fails silently.
  [ -n "${LOOP_TRACE:-}" ] && set -x
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/open-ticket-pr.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/open-ticket-pr.sh" out
  mkdir -p "$dir/bin"
  cat > "$dir/bin/gh" <<'EOF'
#!/usr/bin/env bash
# Stub gh: records its arguments; `pr create` answers with a URL, `pr list` with nothing.
echo "$*" >> "$GH_LOG"
case "$1 $2" in
  "pr create") echo "https://example.invalid/pull/1" ;;
  "pr list") ;;
esac
EOF
  chmod +x "$dir/bin/gh"
  (
    cd "$dir"
    export PATH="$dir/bin:$PATH" GH_LOG="$dir/gh.log" LOOP_ROOT="$dir/work"
    git init -q --bare origin.git
    git clone -q origin.git work 2>/dev/null
    cd work
    git config user.email "self-test@example.com"; git config user.name "self-test"
    git checkout -q -b trunk
    printf '[loop]\ndefault_branch = "trunk"\nreview_paths = ["fixtures/expected/"]\ncheck = "make check"\n' > .loop.toml
    mkdir -p fixtures/expected src
    echo baseline > fixtures/expected/out.csv; echo code > src/a.txt
    git add -A; git commit -q -m "Scaffold"; git push -q -u origin trunk 2>/dev/null
    # The claim: once, then refused.
    "$me" AA-01 --claim | grep -q '^claimed AA-01' || { echo "self-test: claim should succeed"; exit 1; }
    if "$me" AA-01 --claim 2>/dev/null; then echo "self-test: a second claim must be refused"; exit 1; fi
    # A commit without the trailer is refused while trailer_required defaults to true.
    echo more > src/a.txt; git commit -q -am "AA-01: code change without a trailer"
    if "$me" AA-01 >/dev/null 2>&1; then echo "self-test: a commit without the trailer must be refused"; exit 1; fi
    git commit -q --amend -m "AA-01: code change" -m "Body." -m "Co-Authored-By: Test Agent <agent@example.com>"
    out="$("$me" AA-01)"
    echo "$out" | grep -q '^merge policy: auto-merge on' || { echo "self-test: a change outside the review paths should auto-merge:"; echo "$out"; exit 1; }
    grep -q '^pr create --base trunk --head ticket/AA-01 --title AA-01: code change' "$GH_LOG" || { echo "self-test: pr create should target trunk:"; cat "$GH_LOG"; exit 1; }
    grep -q '^pr merge https://example.invalid/pull/1 --auto --rebase' "$GH_LOG" || { echo "self-test: auto-merge not requested:"; cat "$GH_LOG"; exit 1; }
    grep -q 'needs-review' "$GH_LOG" && { echo "self-test: needs-review must not be applied to a code change"; exit 1; }
    # A change under the review path: labelled, not auto-merged.
    : > "$GH_LOG"
    echo changed > fixtures/expected/out.csv
    git commit -q -am "AA-02: refresh the baseline" -m "Co-Authored-By: Test Agent <agent@example.com>"
    out="$("$me" AA-02)"
    echo "$out" | grep -q '^merge policy: a review path changed' || { echo "self-test: a baseline change should wait for review:"; echo "$out"; exit 1; }
    echo "$out" | grep -q '  fixtures/expected/: fixtures/expected/out.csv' || { echo "self-test: the hit should name the glob and file:"; echo "$out"; exit 1; }
    grep -q '^pr edit https://example.invalid/pull/1 --add-label needs-review' "$GH_LOG" || { echo "self-test: needs-review label not applied:"; cat "$GH_LOG"; exit 1; }
    grep -q '^pr merge' "$GH_LOG" && { echo "self-test: auto-merge must not be requested for a baseline change"; exit 1; }
    # A shell-pattern review path matches too; and with trailer_required false a bare commit opens.
    printf '[loop]\ndefault_branch = "trunk"\nreview_paths = ["docs/*.md"]\ntrailer_required = false\n' > .loop.toml
    mkdir -p docs; echo notes > docs/NOTES.md
    git add -A; git commit -q -m "AA-03: docs without a trailer"
    : > "$GH_LOG"
    out="$("$me" AA-03)"
    echo "$out" | grep -q '  docs/\*.md: docs/NOTES.md' || { echo "self-test: the pattern should match docs/NOTES.md:"; echo "$out"; exit 1; }
    # --release drops the claim.
    "$me" AA-01 --release | grep -q '^released AA-01' || { echo "self-test: release should delete the claim branch"; exit 1; }
    "$me" AA-01 --release | grep -q '^nothing to release' || { echo "self-test: a second release finds nothing"; exit 1; }
  )
  echo "open-ticket-pr self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  *) run ;;
esac
