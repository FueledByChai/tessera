#!/usr/bin/env bash
# Installs the loop kit into a checkout (HK-16): the scripts into scripts/, the prompts into
# loop/prompts/, AGENTS.md and .loop.toml when absent, the workflow skeleton when there is no
# workflow, and the two wrappers when a command directory is given. Then it says what the
# project still has to supply.
#
#   install.sh <checkout> [--commands <dir>]    install into <checkout>
#   install.sh --self-test                      a fresh repository with a stub check proves the
#                                               install and the installed self-tests
set -euo pipefail
KIT="$(cd "$(dirname "$0")" && pwd)"
TARGET="${1:-}"
COMMANDS=""
[ -n "$TARGET" ] || { echo "usage: install.sh <checkout> [--commands <dir>] | --self-test" >&2; exit 2; }
shift
while [ $# -gt 0 ]; do
  case "$1" in
    --commands) COMMANDS="$2"; shift ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done

install_into() {
  local target="$1" commands="$2" f
  [ -d "$target" ] || { echo "no such directory: $target" >&2; return 1; }
  target="$(cd "$target" && pwd)"
  mkdir -p "$target/scripts" "$target/loop/prompts"
  for f in "$KIT"/scripts/*.sh; do cp "$f" "$target/scripts/"; chmod +x "$target/scripts/$(basename "$f")"; done
  for f in "$KIT"/prompts/*.md; do cp "$f" "$target/loop/prompts/"; done
  echo "installed: scripts/{$(cd "$KIT/scripts" && ls *.sh | sed 's/\.sh$//' | tr '\n' ',' | sed 's/,$//')}.sh and loop/prompts/*.md"
  if [ -e "$target/AGENTS.md" ]; then
    echo "kept: AGENTS.md (already present; compare its loop section with the kit's when you update)"
  else
    cp "$KIT/AGENTS.md" "$target/AGENTS.md"; echo "installed: AGENTS.md (fill in the Project rules)"
  fi
  if [ -e "$target/.loop.toml" ]; then
    echo "kept: .loop.toml (already present)"
  else
    cp "$KIT/loop.toml.example" "$target/.loop.toml"; echo "installed: .loop.toml (from the example; set kit to the kit's URL and kit_ref to a tag)"
  fi
  if [ -d "$target/.github/workflows" ] && [ -n "$(ls "$target/.github/workflows" 2>/dev/null)" ]; then
    echo "kept: .github/workflows (a workflow exists; compare with the kit's ci/workflow.yml)"
  else
    mkdir -p "$target/.github/workflows"; cp "$KIT/ci/workflow.yml" "$target/.github/workflows/loop.yml"
    echo "installed: .github/workflows/loop.yml (one job per check command; its job names are the required status checks)"
  fi
  if [ -n "$commands" ]; then
    mkdir -p "$target/$commands"
    for f in "$KIT"/commands/*.md; do cp "$f" "$target/$commands/"; done
    echo "installed: $commands/{next-ticket,grill-me}.md wrappers"
  fi
  if [ ! -e "$target/$(basename "$(cd "$target" && "$target/scripts/loop-config.sh" backlog)")" ]; then
    echo "note: the backlog file ($(cd "$target" && "$target/scripts/loop-config.sh" backlog)) does not exist yet; create it with a heading per section and a ticket per '### <ID> <title>'"
  fi
  cat <<EOF

Still to supply:
  1. scripts/check.sh: the definition of done for this project (exit non-zero on anything not
     shippable; run the loop self-tests from it: scripts/loop-config.sh --self-test,
     scripts/backlog-status.sh --self-test, scripts/open-ticket-pr.sh --self-test,
     scripts/release-notes.sh --self-test, scripts/loop-kit-sync.sh --check).
     $( [ -x "$target/scripts/check.sh" ] && echo "(present)" || echo "(missing)" )
  2. Optionally a deploy script, if a merged PR should reach a running service on its own.
  3. The repository settings and branch ruleset, once, with gh (see README.md and ci/ruleset.json).
  4. The Project rules section of AGENTS.md.
EOF
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/loop-install.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR/fresh" out
  mkdir -p "$dir/scripts"
  (cd "$dir" && git init -q && git config user.email t@example.com && git config user.name t)
  printf '#!/usr/bin/env bash\necho stub check\n' > "$dir/scripts/check.sh"; chmod +x "$dir/scripts/check.sh"
  out="$("$KIT/install.sh" "$dir" --commands .agent/commands)"
  echo "$out" | grep -q '^installed: AGENTS.md' || { echo "self-test: AGENTS.md should be installed:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^installed: .loop.toml' || { echo "self-test: .loop.toml should be installed:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^installed: .github/workflows/loop.yml' || { echo "self-test: the workflow should be installed:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^installed: .agent/commands/' || { echo "self-test: the wrappers should be installed:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '(present)' || { echo "self-test: the stub check should be reported present:"; echo "$out"; exit 1; }
  for f in loop-config backlog-status open-ticket-pr release-notes loop-kit-sync; do
    [ -x "$dir/scripts/$f.sh" ] || { echo "self-test: scripts/$f.sh missing or not executable"; exit 1; }
  done
  [ -e "$dir/loop/prompts/next-ticket.md" ] && [ -e "$dir/loop/prompts/grill-me.md" ] || { echo "self-test: prompts missing"; exit 1; }
  grep -q '^## Project rules' "$dir/AGENTS.md" || { echo "self-test: AGENTS.md lacks the Project rules heading"; exit 1; }
  # The installed scripts prove themselves from the fresh repository.
  (cd "$dir" && scripts/loop-config.sh --self-test | grep -q 'self-test passed') || { echo "self-test: installed loop-config self-test failed"; exit 1; }
  (cd "$dir" && scripts/backlog-status.sh --self-test | grep -q 'self-test passed') || { echo "self-test: installed backlog-status self-test failed"; exit 1; }
  (cd "$dir" && scripts/release-notes.sh --self-test | grep -q 'self-test passed') || { echo "self-test: installed release-notes self-test failed"; exit 1; }
  (cd "$dir" && scripts/open-ticket-pr.sh --self-test | grep -q 'self-test passed') || { echo "self-test: installed open-ticket-pr self-test failed"; exit 1; }
  # Installing again keeps what exists.
  out="$("$KIT/install.sh" "$dir")"
  echo "$out" | grep -q '^kept: AGENTS.md' || { echo "self-test: a second install must keep AGENTS.md:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^kept: .loop.toml' || { echo "self-test: a second install must keep .loop.toml:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^kept: .github/workflows' || { echo "self-test: a second install must keep the workflow:"; echo "$out"; exit 1; }
  echo "install self-test passed"
}

case "$TARGET" in
  --self-test) self_test ;;
  *) install_into "$TARGET" "$COMMANDS" ;;
esac
