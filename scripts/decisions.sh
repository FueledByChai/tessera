#!/usr/bin/env bash
# Decision records: one file per decision, numbered, never edited in place, with an index.
# A decision that changes is a new record that supersedes the old one, so the reason for
# every choice survives. Agents and people cite a record by its number.
#
#   scripts/decisions.sh new "<title>" [--supersedes NNNN]
#                                       create the next numbered record from the template
#                                       (NNNN-<slug>.md), mark NNNN superseded by it when
#                                       given, rewrite the index, and print the new path
#   scripts/decisions.sh index          rewrite the index (README.md in the directory) from
#                                       the records: number, title, status, one line each
#   scripts/decisions.sh --check        exit 1 when a record lacks a section or a status,
#                                       when the index does not list exactly the records, or
#                                       when a superseded record points at a number that
#                                       does not exist; a missing directory is nothing to check
#   scripts/decisions.sh --self-test    a fixture proves new, supersede, index, and --check
#
# The directory comes from .loop.toml (`decisions`, default docs/decisions). The template is
# templates/decision.md in the kit, or loop/templates/decision.md in a project.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
MODE="${1:-}"
[ -n "$MODE" ] || { echo "usage: scripts/decisions.sh new \"<title>\" [--supersedes NNNN] | index | --check | --self-test" >&2; exit 2; }

SECTIONS=("## Context" "## Decision" "## Alternatives" "## Consequences" "## What would show this was wrong")

dir() { echo "$ROOT/$("$CONFIG" decisions)"; }

template() {
  if [ -f "$ROOT/loop/templates/decision.md" ]; then echo "$ROOT/loop/templates/decision.md"
  elif [ -f "$SCRIPT_ROOT/templates/decision.md" ]; then echo "$SCRIPT_ROOT/templates/decision.md"
  elif [ -f "$SCRIPT_ROOT/loop/templates/decision.md" ]; then echo "$SCRIPT_ROOT/loop/templates/decision.md"
  else echo "decisions: no template found (templates/decision.md or loop/templates/decision.md)" >&2; return 1; fi
}

records() {  # the record files, numbered order
  local d; d="$(dir)"
  [ -d "$d" ] || return 0
  find "$d" -maxdepth 1 -name '[0-9][0-9][0-9][0-9]-*.md' | sort
}

title_of() { sed -n '1s/^# [0-9]* — //p' "$1"; }
status_of() { sed -n 's/^Status: *//p' "$1" | head -1; }
number_of() { basename "$1" | cut -c1-4; }

slug() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//' | cut -c1-60
}

write_index() {
  local d; d="$(dir)"
  mkdir -p "$d"
  {
    echo "# Decisions"
    echo
    echo "One file per decision, never edited in place: a change is a new record that supersedes"
    echo "the old one. Cite a decision by its number. \`scripts/decisions.sh new \"<title>\"\` adds one."
    echo
    local f
    while IFS= read -r f; do
      [ -n "$f" ] || continue
      echo "- [$(number_of "$f")]($(basename "$f")) $(title_of "$f") — $(status_of "$f")"
    done < <(records)
  } > "$d/README.md"
}

new_record() {
  local title="$1" supersedes="${2:-}" d t next num path old
  title="$(printf '%s' "$title" | sed 's/^ *//; s/ *$//')"
  [ -n "$title" ] || { echo "decisions: the record needs a title" >&2; return 2; }
  d="$(dir)"; t="$(template)"
  mkdir -p "$d"
  next=1
  while IFS= read -r old; do [ -n "$old" ] && next=$((10#$(number_of "$old") + 1)); done < <(records)
  num="$(printf '%04d' "$next")"
  path="$d/$num-$(slug "$title").md"
  if [ -n "$supersedes" ]; then
    old="$(find "$d" -maxdepth 1 -name "$supersedes-*.md" | head -1)"
    [ -n "$old" ] || { echo "decisions: no record $supersedes to supersede" >&2; return 2; }
  fi
  sed -e "s/^# NNNN — TITLE\$/# $num — $(printf '%s' "$title" | sed 's/[&/\\]/\\&/g')/" \
      -e "s/^Date: DATE\$/Date: $(date +%Y-%m-%d)/" \
      -e "s/^Status: accepted\$/Status: accepted${supersedes:+, supersedes $supersedes}/" "$t" > "$path"
  if [ -n "$supersedes" ]; then
    sed -i.bak "s/^Status: .*\$/Status: superseded by $num/" "$old" && rm -f "$old.bak"
  fi
  write_index
  echo "$path"
}

check() {
  local d f n missing=0 listed
  d="$(dir)"
  if [ ! -d "$d" ]; then echo "decisions: no directory at ${d#"$ROOT"/}; nothing to check"; return 0; fi
  n=0
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    n=$((n + 1))
    [ -n "$(status_of "$f")" ] || { echo "decisions: $(basename "$f") has no Status line" >&2; missing=1; }
    [ -n "$(title_of "$f")" ] || { echo "decisions: $(basename "$f") has no '# NNNN — title' heading" >&2; missing=1; }
    local s
    for s in "${SECTIONS[@]}"; do grep -qxF "$s" "$f" || { echo "decisions: $(basename "$f") lacks '$s'" >&2; missing=1; }; done
    case "$(status_of "$f")" in
      "superseded by "*)
        local by; by="$(status_of "$f" | sed 's/^superseded by //')"
        [ -n "$(find "$d" -maxdepth 1 -name "$by-*.md" | head -1)" ] || { echo "decisions: $(basename "$f") is superseded by $by, which does not exist" >&2; missing=1; } ;;
    esac
    if [ -f "$d/README.md" ]; then
      grep -qF "]($(basename "$f"))" "$d/README.md" || { echo "decisions: the index does not list $(basename "$f")" >&2; missing=1; }
    fi
  done < <(records)
  if [ "$n" = 0 ]; then echo "decisions: no records in ${d#"$ROOT"/}"; return 0; fi
  [ -f "$d/README.md" ] || { echo "decisions: no index (${d#"$ROOT"/}/README.md); run scripts/decisions.sh index" >&2; return 1; }
  listed="$(grep -c '^- \[[0-9]\{4\}\](' "$d/README.md" || true)"
  [ "$listed" = "$n" ] || { echo "decisions: the index lists $listed record(s), the directory holds $n; run scripts/decisions.sh index" >&2; missing=1; }
  [ "$missing" = 0 ] || return 1
  echo "decisions: $n record(s) in ${d#"$ROOT"/}, index in step"
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/decisions.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/decisions.sh" out rc
  export LOOP_ROOT="$dir"
  printf '[loop]\n' > "$dir/.loop.toml"
  # No directory yet: nothing to check.
  out="$("$me" --check)" && echo "$out" | grep -q 'no directory' || { echo "self-test: a missing directory should be nothing to check:"; echo "$out"; exit 1; }
  # The first record: numbered 0001, from the template, with the title, a date, and a status.
  out="$("$me" new "Use Postgres for the catalog")"
  [ "$out" = "$dir/docs/decisions/0001-use-postgres-for-the-catalog.md" ] || { echo "self-test: the first record should be 0001 with a slug, got '$out'"; exit 1; }
  grep -q '^# 0001 — Use Postgres for the catalog$' "$out" && grep -q '^Status: accepted$' "$out" && grep -qE '^Date: [0-9]{4}-[0-9]{2}-[0-9]{2}$' "$out" || { echo "self-test: the record's heading, status, or date is wrong:"; head -4 "$out"; exit 1; }
  grep -q '^## What would show this was wrong$' "$out" || { echo "self-test: the template's sections should be in the record"; exit 1; }
  grep -q '\[0001\](0001-use-postgres-for-the-catalog.md) Use Postgres for the catalog — accepted' "$dir/docs/decisions/README.md" || { echo "self-test: the index should list 0001:"; cat "$dir/docs/decisions/README.md"; exit 1; }
  # A second record supersedes the first: the old one is marked, the new one says what it supersedes.
  out="$("$me" new "Use SQLite; one user, one machine" --supersedes 0001)"
  [ "$(basename "$out")" = "0002-use-sqlite-one-user-one-machine.md" ] || { echo "self-test: the second record should be 0002, got '$out'"; exit 1; }
  grep -q '^Status: accepted, supersedes 0001$' "$out" || { echo "self-test: the new record should say what it supersedes:"; head -4 "$out"; exit 1; }
  grep -q '^Status: superseded by 0002$' "$dir/docs/decisions/0001-use-postgres-for-the-catalog.md" || { echo "self-test: 0001 should be marked superseded by 0002"; exit 1; }
  grep -q '0001.*— superseded by 0002' "$dir/docs/decisions/README.md" && grep -q '0002.*— accepted, supersedes 0001' "$dir/docs/decisions/README.md" || { echo "self-test: the index should carry both statuses:"; cat "$dir/docs/decisions/README.md"; exit 1; }
  out="$("$me" --check)" && echo "$out" | grep -q '2 record(s)' || { echo "self-test: --check should pass with two records and the index in step:"; echo "$out"; exit 1; }
  # Superseding a record that does not exist is refused; an empty title is refused.
  if "$me" new "Nothing" --supersedes 0042 >/dev/null 2>&1; then echo "self-test: superseding a missing record must be refused"; exit 1; fi
  if "$me" new "   " >/dev/null 2>&1; then echo "self-test: an empty title must be refused"; exit 1; fi
  # An index out of step fails --check and `index` repairs it; a record missing a section fails.
  sed -i.bak '/0002/d' "$dir/docs/decisions/README.md"; rm -f "$dir/docs/decisions/README.md.bak"
  rc=0; out="$("$me" --check 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q 'does not list 0002' || { echo "self-test: an index missing a record should fail --check (rc $rc):"; echo "$out"; exit 1; }
  "$me" index >/dev/null; "$me" --check >/dev/null || { echo "self-test: index should repair the index"; exit 1; }
  sed -i.bak '/^## Consequences$/d' "$dir/docs/decisions/0002-use-sqlite-one-user-one-machine.md"; rm -f "$dir/docs/decisions/"*.bak
  rc=0; out="$("$me" --check 2>&1)" || rc=$?
  [ "$rc" = 1 ] && echo "$out" | grep -q "lacks '## Consequences'" || { echo "self-test: a record without a section should fail --check (rc $rc):"; echo "$out"; exit 1; }
  # Another directory name from the config.
  printf '[loop]\ndecisions = "adr"\n' > "$dir/.loop.toml"
  out="$("$me" new "Keep the adr name")"; [ "$out" = "$dir/adr/0001-keep-the-adr-name.md" ] || { echo "self-test: the decisions key should move the directory, got '$out'"; exit 1; }
  unset LOOP_ROOT
  echo "decisions self-test passed"
}

case "$MODE" in
  --self-test) self_test ;;
  --check) check ;;
  index) write_index; echo "decisions: index written to $("$CONFIG" decisions)/README.md" ;;
  new)
    TITLE="${2:-}"; SUPERSEDES=""
    if [ "${3:-}" = "--supersedes" ]; then SUPERSEDES="${4:-}"; fi
    new_record "$TITLE" "$SUPERSEDES"
    ;;
  *) echo "unknown mode: $MODE" >&2; exit 2 ;;
esac
