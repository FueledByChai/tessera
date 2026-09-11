#!/usr/bin/env bash
# Edits the sprint list in .loop.toml (HK-40): `sprint = [...]`, the tickets chosen for now in
# the order to work them, which scripts/backlog-status.sh --next takes before file order.
# Every command rewrites the one line (or appends it under [loop]) and prints the new list;
# committing the change is the owner's, as with any edit to .loop.toml.
#
#   scripts/sprint.sh                          the sprint's tickets with their states
#                                              (scripts/backlog-status.sh --sprint)
#   scripts/sprint.sh add <id> [--before <id> | --after <id>]
#                                              add a ticket (at the end by default); it must be
#                                              a heading in the backlog file and not done
#   scripts/sprint.sh remove <id>              take a ticket out
#   scripts/sprint.sh set <id> [<id> ...]      replace the whole list
#   scripts/sprint.sh clear                    empty it
#   scripts/sprint.sh --self-test
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CONFIG="$ROOT/scripts/loop-config.sh"
FILE="${LOOP_CONFIG:-${LOOP_ROOT:-$ROOT}/.loop.toml}"

current() { "$CONFIG" sprint; }

# The backlog file, relative to the config's root.
backlog_file() {
  local root; root="$(dirname "$FILE")"
  echo "$root/$("$CONFIG" backlog)"
}

# A ticket id must be a heading in the backlog file, and not already done.
check_id() {
  local id="$1" backlog; backlog="$(backlog_file)"
  [[ "$id" =~ ^[A-Z]+-[0-9]+$ ]] || { echo "not a ticket id: $id" >&2; return 1; }
  grep -q "^### $id " "$backlog" 2>/dev/null || { echo "$id is not a ticket in $backlog" >&2; return 1; }
  local state
  state="$(cd "$(dirname "$backlog")" && "$ROOT/scripts/backlog-status.sh" --backlog "$backlog" --local 2>/dev/null | awk -v id="$id" '$1 == id { print $2 }')"
  [ "$state" != "done" ] || { echo "$id is already done" >&2; return 1; }
  return 0
}

# Writes the list (one id per argument) as the sprint line.
write_list() {
  local items="" id
  for id in "$@"; do items="$items${items:+, }\"$id\""; done
  local line="sprint = [$items]"
  if [ -f "$FILE" ] && grep -q '^sprint *=' "$FILE"; then
    awk -v line="$line" '/^sprint *=/ && !done { print line; done = 1; next } { print }' "$FILE" > "$FILE.tmp" && mv -f "$FILE.tmp" "$FILE"
  elif [ -f "$FILE" ] && grep -q '^\[loop\]' "$FILE"; then
    printf '%s\n' "$line" >> "$FILE"
  else
    printf '[loop]\n%s\n' "$line" >> "$FILE"
  fi
  if [ $# -eq 0 ]; then echo "sprint: empty"; else echo "sprint: $*"; fi
}

cmd="${1:-}"
case "$cmd" in
  "")
    exec "$ROOT/scripts/backlog-status.sh" --sprint ;;
  add)
    id="${2:-}"; [ -n "$id" ] || { echo "usage: scripts/sprint.sh add <id> [--before <id> | --after <id>]" >&2; exit 2; }
    check_id "$id"
    where="${3:-}"; anchor="${4:-}"
    list=(); while IFS= read -r x; do [ -n "$x" ] && [ "$x" != "$id" ] && list+=("$x"); done < <(current)
    if [ -z "$where" ]; then
      list+=("$id")
    else
      [ "$where" = --before ] || [ "$where" = --after ] || { echo "unknown flag: $where" >&2; exit 2; }
      new=(); placed=0
      for x in "${list[@]}"; do
        [ "$where" = --before ] && [ "$x" = "$anchor" ] && { new+=("$id"); placed=1; }
        new+=("$x")
        [ "$where" = --after ] && [ "$x" = "$anchor" ] && { new+=("$id"); placed=1; }
      done
      [ "$placed" = 1 ] || { echo "$anchor is not in the sprint" >&2; exit 1; }
      list=("${new[@]}")
    fi
    write_list "${list[@]}" ;;
  remove)
    id="${2:-}"; [ -n "$id" ] || { echo "usage: scripts/sprint.sh remove <id>" >&2; exit 2; }
    list=(); found=0
    while IFS= read -r x; do [ -n "$x" ] || continue; if [ "$x" = "$id" ]; then found=1; else list+=("$x"); fi; done < <(current)
    [ "$found" = 1 ] || { echo "$id is not in the sprint" >&2; exit 1; }
    write_list "${list[@]+"${list[@]}"}" ;;
  set)
    shift; [ $# -gt 0 ] || { echo "usage: scripts/sprint.sh set <id> [<id> ...]" >&2; exit 2; }
    for id in "$@"; do check_id "$id"; done
    write_list "$@" ;;
  clear)
    write_list ;;
  --self-test)
    dir="$(mktemp -d "${TMPDIR:-/tmp}/sprint.XXXXXX")"; trap 'rm -rf "$dir"' EXIT
    me="$ROOT/scripts/sprint.sh"
    (cd "$dir" && git init -q && git config user.email t@example.com && git config user.name t
     printf '# Queue\n\n## Alpha\n\n### AA-01 First\n### AA-02 Second\n### AA-03 Third\n' > BACKLOG.md
     git add -A && git commit -q -m "Scaffold" && git branch -q -M main && git commit -q --allow-empty -m "AA-03: third landed")
    printf '[loop]\ndefault_branch = "main"\n' > "$dir/.loop.toml"
    export LOOP_CONFIG="$dir/.loop.toml"
    out="$("$me" add AA-01)"; [ "$out" = "sprint: AA-01" ] || { echo "self-test: add should start the list: $out"; exit 1; }
    grep -q '^sprint = \["AA-01"\]$' "$dir/.loop.toml" || { echo "self-test: the line should be appended under [loop]"; cat "$dir/.loop.toml"; exit 1; }
    out="$("$me" add AA-02 --before AA-01)"; [ "$out" = "sprint: AA-02 AA-01" ] || { echo "self-test: --before should place it first: $out"; exit 1; }
    out="$("$me" add AA-01)"; [ "$out" = "sprint: AA-02 AA-01" ] || { echo "self-test: adding an id already there should not duplicate it: $out"; exit 1; }
    if "$me" add AA-03 >/dev/null 2>&1; then echo "self-test: a done ticket must be refused"; exit 1; fi
    if "$me" add AA-09 >/dev/null 2>&1; then echo "self-test: an id not in the file must be refused"; exit 1; fi
    if "$me" add AA-01 --before AA-09 >/dev/null 2>&1; then echo "self-test: an anchor not in the sprint must be refused"; exit 1; fi
    [ "$("$CONFIG" sprint | tr '\n' ' ')" = "AA-02 AA-01 " ] || { echo "self-test: loop-config should read the list back"; exit 1; }
    [ "$(grep -c '^sprint' "$dir/.loop.toml")" = 1 ] || { echo "self-test: exactly one sprint line"; exit 1; }
    out="$("$me" remove AA-02)"; [ "$out" = "sprint: AA-01" ] || { echo "self-test: remove: $out"; exit 1; }
    out="$("$me" set AA-02 AA-01)"; [ "$out" = "sprint: AA-02 AA-01" ] || { echo "self-test: set: $out"; exit 1; }
    out="$("$me" clear)"; [ "$out" = "sprint: empty" ] || { echo "self-test: clear: $out"; exit 1; }
    grep -q '^sprint = \[\]$' "$dir/.loop.toml" || { echo "self-test: clear should leave an empty list"; exit 1; }
    unset LOOP_CONFIG
    echo "sprint self-test passed" ;;
  *) echo "usage: scripts/sprint.sh [add <id> [--before <id> | --after <id>] | remove <id> | set <id>... | clear | --self-test]" >&2; exit 2 ;;
esac
