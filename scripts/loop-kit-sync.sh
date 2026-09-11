#!/usr/bin/env bash
# Keeps this checkout's copy of the loop kit in step with the kit (HK-16). The kit is the
# source of truth for the loop scripts and prompts; a project holds copies. `kit` in
# .loop.toml names it: a directory (relative to the root) or a git URL, with `kit_ref` the tag
# or branch to take from a URL.
#
#   scripts/loop-kit-sync.sh            copy the kit's scripts/*.sh into scripts/, its
#                                       prompts/*.md into loop/prompts/, and its templates/*.md
#                                       into loop/templates/, and say what changed
#   scripts/loop-kit-sync.sh --check    exit 1 with a list when any copy differs from the kit
#   scripts/loop-kit-sync.sh --self-test
#                                       a fixture kit and checkout prove both modes
#
# A directory is used as it stands (no tag); a URL, file:// included, is cloned at kit_ref.
# Each file is written beside its target and renamed into place, this script's own copy is
# replaced last, and when it was replaced the new copy is run once more (HK-35): a sync that
# updates loop-kit-sync.sh itself would otherwise overwrite the file the running shell is
# still reading and stop with an error, and the files only the new copy knows to list would
# wait for a second run.
# When the kit is a directory inside this checkout (the unpublished kit lives at loop/), the
# prompts are the kit's own files and only the scripts are compared.
set -euo pipefail
SCRIPT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ROOT="${LOOP_ROOT:-$SCRIPT_ROOT}"
CONFIG="$SCRIPT_ROOT/scripts/loop-config.sh"
MODE=sync
case "${1:-}" in
  --check) MODE=check ;;
  --self-test) MODE=selftest ;;
  "") ;;
  *) echo "usage: scripts/loop-kit-sync.sh [--check|--self-test]" >&2; exit 2 ;;
esac

# Prints the kit's directory, cloning a URL at kit_ref into a scratch directory first.
resolve_kit() {
  local kit ref
  kit="$("$CONFIG" kit)"
  ref="$("$CONFIG" kit_ref)"
  [ -n "$kit" ] || { echo "no kit configured: set kit (a directory or a git URL) in .loop.toml" >&2; return 1; }
  if [ -d "$ROOT/$kit" ]; then echo "$ROOT/$kit"; return 0; fi
  if [ -d "$kit" ]; then echo "$(cd "$kit" && pwd)"; return 0; fi
  KIT_CLONE="$(mktemp -d "${TMPDIR:-/tmp}/loop-kit.XXXXXX")"
  if [ -n "$ref" ]; then
    git clone -q --depth 1 --branch "$ref" "$kit" "$KIT_CLONE/kit" 2>/dev/null || { echo "cannot clone $kit at $ref" >&2; return 1; }
  else
    git clone -q --depth 1 "$kit" "$KIT_CLONE/kit" 2>/dev/null || { echo "cannot clone $kit" >&2; return 1; }
  fi
  echo "$KIT_CLONE/kit"
}
KIT_CLONE=""
cleanup() { [ -n "$KIT_CLONE" ] && rm -rf "$KIT_CLONE"; return 0; }
trap cleanup EXIT

# The pairs `<kit file> <checkout file>`, one per line.
pairs() {
  local kit="$1" f
  for f in "$kit"/scripts/*.sh; do [ -e "$f" ] && echo "$f $ROOT/scripts/$(basename "$f")"; done
  for f in "$kit"/prompts/*.md; do [ -e "$f" ] && echo "$f $ROOT/loop/prompts/$(basename "$f")"; done
  for f in "$kit"/templates/*.md; do [ -e "$f" ] && echo "$f $ROOT/loop/templates/$(basename "$f")"; done
  for f in "$kit"/templates/check/*.sh; do [ -e "$f" ] && echo "$f $ROOT/loop/templates/check/$(basename "$f")"; done
  for f in "$kit"/templates/ci/*.yml; do [ -e "$f" ] && echo "$f $ROOT/loop/templates/ci/$(basename "$f")"; done
  return 0
}

# Copies src over dst through a temporary file beside it and one rename, so a reader of the
# old file (this shell, when dst is this script) keeps the old inode to the end.
place() {
  mkdir -p "$(dirname "$2")"
  cp "$1" "$2.tmp"; chmod +x "$2.tmp" 2>/dev/null || true
  mv -f "$2.tmp" "$2"
}

run() {
  local kit mode="$1" src dst rel differ=0 same=0 changed=0 self_src="" self_dst=""
  kit="$(resolve_kit)"
  while read -r src dst; do
    [ -n "$src" ] || continue
    [ "$src" -ef "$dst" ] && { same=$((same + 1)); continue; }
    rel="${dst#"$ROOT"/}"
    if [ -e "$dst" ] && cmp -s "$src" "$dst"; then
      same=$((same + 1))
    elif [ "$mode" = check ]; then
      differ=$((differ + 1))
      if [ -e "$dst" ]; then echo "differs: $rel"; else echo "missing: $rel"; fi
    elif [ "$dst" = "$SCRIPT_ROOT/scripts/loop-kit-sync.sh" ]; then
      self_src="$src"; self_dst="$dst"
    else
      place "$src" "$dst"
      changed=$((changed + 1)); echo "updated: $rel"
    fi
  done < <(pairs "$kit")
  if [ -n "$self_dst" ]; then
    place "$self_src" "$self_dst"
    changed=$((changed + 1)); echo "updated: ${self_dst#"$ROOT"/}"
    if [ -z "${LOOP_KIT_SYNC_AGAIN:-}" ]; then
      echo "loop kit: loop-kit-sync.sh changed; running the new copy for the files it lists"
      cleanup; KIT_CLONE=""
      LOOP_KIT_SYNC_AGAIN=1 exec "$self_dst"
    fi
  fi
  local where; where="$("$CONFIG" kit)"; [ -n "$("$CONFIG" kit_ref)" ] && where="$where at $("$CONFIG" kit_ref)"
  if [ "$mode" = check ]; then
    if [ "$differ" -gt 0 ]; then echo "loop kit: $differ file(s) differ from $where; run scripts/loop-kit-sync.sh" >&2; return 1; fi
    echo "loop kit: in sync with $where ($same files)"
  else
    echo "loop kit: $changed file(s) updated, $same unchanged, from $where"
  fi
}

self_test() {
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/loop-kit-sync.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"; cleanup' EXIT
  local dir="$SELF_TEST_DIR" me="$SCRIPT_ROOT/scripts/loop-kit-sync.sh" out
  # A kit as a bare git repository with a tag, and a checkout that names it by URL.
  mkdir -p "$dir/kit-src/scripts" "$dir/kit-src/prompts" "$dir/work/scripts" "$dir/work/loop/prompts"
  cp "$SCRIPT_ROOT/scripts/loop-config.sh" "$dir/work/scripts/"
  printf '#!/usr/bin/env bash\necho tool v1\n' > "$dir/kit-src/scripts/tool.sh"
  printf 'prompt v1\n' > "$dir/kit-src/prompts/do.md"
  (cd "$dir/kit-src" && git init -q && git config user.email t@example.com && git config user.name t && git add -A && git commit -q -m "kit v1" && git tag v1)
  printf '[loop]\nkit = "file://%s"\nkit_ref = "v1"\n' "$dir/kit-src" > "$dir/work/.loop.toml"
  export LOOP_ROOT="$dir/work"
  # Nothing copied yet: --check lists the missing files and fails.
  if out="$("$me" --check 2>&1)"; then echo "self-test: --check must fail before the first sync"; exit 1; fi
  echo "$out" | grep -q '^missing: scripts/tool.sh' || { echo "self-test: --check should list the missing script:"; echo "$out"; exit 1; }
  echo "$out" | grep -q '^missing: loop/prompts/do.md' || { echo "self-test: --check should list the missing prompt:"; echo "$out"; exit 1; }
  # The sync copies them; --check then passes and names the ref.
  out="$("$me")"
  echo "$out" | grep -q '^loop kit: 2 file(s) updated' || { echo "self-test: the sync should copy two files:"; echo "$out"; exit 1; }
  [ -x "$dir/work/scripts/tool.sh" ] || { echo "self-test: the copied script must be executable"; exit 1; }
  out="$("$me" --check)"
  echo "$out" | grep -q "^loop kit: in sync with file://$dir/kit-src at v1 (2 files)" || { echo "self-test: --check should pass after the sync:"; echo "$out"; exit 1; }
  # Local drift fails --check; the kit moving on (a new tag) shows as a difference too.
  echo "edited" >> "$dir/work/scripts/tool.sh"
  if "$me" --check >/dev/null 2>&1; then echo "self-test: --check must fail on local drift"; exit 1; fi
  "$me" >/dev/null
  (cd "$dir/kit-src" && printf 'prompt v2\n' > prompts/do.md && git commit -q -am "kit v2" && git tag v2)
  "$me" --check >/dev/null || { echo "self-test: v1 is still what the config names, so --check must pass"; exit 1; }
  printf '[loop]\nkit = "file://%s"\nkit_ref = "v2"\n' "$dir/kit-src" > "$dir/work/.loop.toml"
  out="$("$me" --check 2>&1 || true)"
  echo "$out" | grep -q '^differs: loop/prompts/do.md' || { echo "self-test: moving kit_ref to v2 should show the prompt differing:"; echo "$out"; exit 1; }
  # A kit that is a directory inside the checkout: its prompts are the checkout's own files.
  mkdir -p "$dir/work/loop/scripts"
  cp "$dir/kit-src/scripts/tool.sh" "$dir/work/loop/scripts/tool.sh"
  printf '[loop]\nkit = "loop"\n' > "$dir/work/.loop.toml"
  out="$("$me" --check)"
  echo "$out" | grep -q '^loop kit: in sync with loop (2 files)' || { echo "self-test: an in-checkout kit should count its prompts as the same files:"; echo "$out"; exit 1; }
  # No kit configured: a clear message, not a silent pass.
  printf '[loop]\n' > "$dir/work/.loop.toml"
  if "$me" --check >/dev/null 2>&1; then echo "self-test: --check must fail with no kit configured"; exit 1; fi
  unset LOOP_ROOT
  # A sync that replaces loop-kit-sync.sh itself (HK-35): the checkout runs its own copy of
  # this script; the kit's copy differs in the middle (it lists one more kind of file,
  # extras/*.txt), so the running shell's offset into its own file would land in shifted
  # text if the file were overwritten in place. One run must finish without an error, copy
  # the files sorted after the script, replace the script's own copy byte for byte, and
  # then copy the file only the new copy knows to list.
  mkdir -p "$dir/self/scripts" "$dir/kit-src/templates/check" "$dir/kit-src/extras"
  cp "$me" "$dir/self/scripts/loop-kit-sync.sh"; cp "$SCRIPT_ROOT/scripts/loop-config.sh" "$dir/self/scripts/"
  awk '{print} /templates\/check\/\*\.sh/ && !done {print "  for f in \"$kit\"/extras/*.txt; do [ -e \"$f\" ] \&\& echo \"$f $ROOT/loop/extras/$(basename \"$f\")\"; done"; done=1}' "$me" > "$dir/kit-src/scripts/loop-kit-sync.sh"
  grep -q 'extras/\*\.txt' "$dir/kit-src/scripts/loop-kit-sync.sh" || { echo "self-test: the fixture kit's sync script should list extras"; exit 1; }
  printf '#!/usr/bin/env bash\necho zz\n' > "$dir/kit-src/scripts/zz-after.sh"
  printf 'skeleton\n' > "$dir/kit-src/templates/check/other.sh"
  printf 'only the new copy lists this\n' > "$dir/kit-src/extras/new.txt"
  printf '[loop]\nkit = "%s"\n' "$dir/kit-src" > "$dir/self/.loop.toml"
  out="$("$dir/self/scripts/loop-kit-sync.sh" 2>&1)" || { echo "self-test: a sync that replaces the script itself must finish (rc $?):"; echo "$out"; exit 1; }
  for f in scripts/zz-after.sh loop/templates/check/other.sh scripts/loop-kit-sync.sh loop/extras/new.txt; do
    echo "$out" | grep -q "^updated: $f\$" || { echo "self-test: one run should copy $f:"; echo "$out"; exit 1; }
  done
  n_zz="$(echo "$out" | grep -n '^updated: scripts/zz-after.sh$' | cut -d: -f1)"
  n_self="$(echo "$out" | grep -n '^updated: scripts/loop-kit-sync.sh$' | cut -d: -f1)"
  n_new="$(echo "$out" | grep -n '^updated: loop/extras/new.txt$' | cut -d: -f1)"
  [ "$n_zz" -lt "$n_self" ] && [ "$n_self" -lt "$n_new" ] || { echo "self-test: the script's own copy should be replaced after the old list and before the new list:"; echo "$out"; exit 1; }
  cmp -s "$dir/kit-src/scripts/loop-kit-sync.sh" "$dir/self/scripts/loop-kit-sync.sh" || { echo "self-test: the script's own copy should equal the kit's after one run"; exit 1; }
  [ -z "$(ls "$dir/self/scripts"/*.tmp 2>/dev/null)" ] || { echo "self-test: no temporary file may be left behind"; exit 1; }
  "$dir/self/scripts/loop-kit-sync.sh" --check >/dev/null || { echo "self-test: --check should pass after the self-replacing sync"; exit 1; }
  echo "loop-kit-sync self-test passed"
}

case "$MODE" in
  selftest) self_test ;;
  check) run check ;;
  sync) run sync ;;
esac
