#!/usr/bin/env bash
# The definition of done, as a command. Exits non-zero on the first failure.
#
#   scripts/check.sh                  everything: fmt, tests, build, parity, web, private checks
#   scripts/check.sh --no-web         skip the web typecheck/lint/build (slow on iCloud checkouts)
#   scripts/check.sh --quick          skip web and the private checks (the CI engine job)
#   scripts/check.sh --web-only       only the web step (the CI web job); the headless layout
#                                     and chart checks skip themselves without a Chromium
#   scripts/check.sh --refresh-baseline
#                                     rewrite examples/expected from the current engine; only after
#                                     an intentional results change, and say so in the commit
#   scripts/check.sh --resolve        print what a run here would use (main checkout, private
#                                     checkout, local.toml, node_modules) and exit
#
# Worktrees: from .claude/worktrees/<name> the script finds the main checkout through the shared
# git dir, takes the private checkout beside it (or TESSERA_PRIVATE_ROOT), writes a local.toml
# from the main one with its relative paths made absolute when the worktree has none, and links
# web/node_modules to the main checkout's when missing. The private checks build the private
# legacy crate against this checkout's engine (TESSERA_ENGINE_ROOT), not the main one.
#
# Parity: the two bundled examples run against the synthetic data and their trades and daily
# equity must match examples/expected byte for byte. A behaviour change in the engine, the SDK,
# or the costs model shows up here before it shows up in a research result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

NO_WEB=0
QUICK=0
WEB_ONLY=0
REFRESH=0
RESOLVE=0
for arg in "$@"; do
  case "$arg" in
    --no-web) NO_WEB=1 ;;
    --quick) QUICK=1; NO_WEB=1 ;;
    --web-only) WEB_ONLY=1; QUICK=1 ;;
    --refresh-baseline) REFRESH=1 ;;
    --resolve) RESOLVE=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done
if [ "$WEB_ONLY" = 1 ] && [ "$NO_WEB" = 1 ]; then
  echo "--web-only and --no-web/--quick exclude each other" >&2
  exit 2
fi

# The main checkout: the parent of the shared git dir (`.git` here, an absolute path from a
# worktree). The private checkout sits beside it unless TESSERA_PRIVATE_ROOT says otherwise.
MAIN_ROOT="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
PRIVATE_ROOT="${TESSERA_PRIVATE_ROOT:-$MAIN_ROOT/../Tessera-private}"
if [ -d "$PRIVATE_ROOT" ]; then PRIVATE_ROOT="$(cd "$PRIVATE_ROOT" && pwd)"; fi
LOCAL_TOML_SOURCE="local.toml"
NODE_MODULES_SOURCE="web/node_modules"
if [ "$ROOT" != "$MAIN_ROOT" ]; then
  if [ ! -e local.toml ] && [ -f "$MAIN_ROOT/local.toml" ]; then
    # Relative paths in the main local.toml (`../Tessera-private/strategies`) mean nothing from
    # a worktree; every relative path that exists beside the main checkout becomes absolute.
    TESSERA_MAIN_ROOT="$MAIN_ROOT" perl -pe \
      's{"(\.\.?/[^"]*)"}{ -e "$ENV{TESSERA_MAIN_ROOT}/$1" ? "\"$ENV{TESSERA_MAIN_ROOT}/$1\"" : "\"$1\"" }ge' \
      "$MAIN_ROOT/local.toml" > local.toml
    LOCAL_TOML_SOURCE="$MAIN_ROOT/local.toml (copied, relative paths made absolute)"
    echo "worktree: local.toml written from $LOCAL_TOML_SOURCE"
  elif [ -e local.toml ]; then
    LOCAL_TOML_SOURCE="local.toml (already present)"
  else
    LOCAL_TOML_SOURCE="none (no local.toml in $MAIN_ROOT either)"
  fi
  if [ ! -e web/node_modules ] && [ -d "$MAIN_ROOT/web/node_modules" ]; then
    ln -s "$MAIN_ROOT/web/node_modules" web/node_modules
    NODE_MODULES_SOURCE="$MAIN_ROOT/web/node_modules (linked)"
    echo "worktree: web/node_modules linked to $MAIN_ROOT/web/node_modules"
  elif [ -L web/node_modules ]; then
    NODE_MODULES_SOURCE="$(readlink web/node_modules) (linked)"
  fi
fi
if [ "$RESOLVE" = 1 ]; then
  echo "checkout:      $ROOT"
  echo "main checkout: $MAIN_ROOT$( [ "$ROOT" = "$MAIN_ROOT" ] && echo ' (this is the main checkout)' || echo ' (this is a worktree)')"
  if [ -x "$PRIVATE_ROOT/scripts/check.sh" ]; then
    echo "private:       $PRIVATE_ROOT (checks will run)"
  else
    echo "private:       $PRIVATE_ROOT (absent: checks will be skipped; set TESSERA_PRIVATE_ROOT)"
  fi
  echo "local.toml:    $LOCAL_TOML_SOURCE"
  echo "node_modules:  $NODE_MODULES_SOURCE"
  exit 0
fi

step() { printf '\n== %s\n' "$1"; }
started=$(date +%s)

if [ "$WEB_ONLY" = 0 ]; then
step "loop self-tests: config, backlog status, ticket PRs, release notes, kit sync, deploy"
scripts/loop-config.sh --self-test
scripts/backlog-status.sh --self-test
scripts/open-ticket-pr.sh --self-test
scripts/release-notes.sh --self-test
scripts/loop-kit-sync.sh --self-test
scripts/deploy-local.sh --self-test

# The loop scripts and prompts are copies from the kit's tagged release (HK-16, HK-19);
# drift fails the check.
step "loop kit: scripts/ and loop/prompts/ match the kit at its tag"
scripts/loop-kit-sync.sh --check

# The loop prompts belong to every project that adopts the loop (HK-15): nothing in loop/
# may name this project, its build tools, a harness, or its review path. (.loop.toml is
# exempt: it holds this project's own values.)
step "loop prompts name no project, build tool, or harness"
if grep -rniE 'tessera|cargo|npm|claude|examples/expected' loop/; then
  echo "loop/ must stay generic: the lines above name the project, a build tool, or a harness" >&2
  exit 1
fi
echo "loop/ is generic"

step "cargo fmt --check"
cargo fmt --all --check

step "cargo test --release"
cargo test --release --quiet 2>&1 | grep -E "test result|FAILED|panicked|error(\[|:)" || true
cargo test --release --quiet >/dev/null 2>&1

step "cargo build --release (tessera, tessera-ui)"
cargo build --release --quiet --bin tessera --bin tessera-ui

step "parity against examples/expected"
for strategy in rsi_mean_reversion moving_average_cross; do
  out="target/check_$strategy"
  rm -rf "$out"
  ./target/release/tessera run-strategy \
    --config "examples/configs/$strategy.toml" \
    --start 2019-01-01 --end 2025-12-31 --output-dir "$out" >/dev/null 2>&1
  if [ "$REFRESH" = 1 ]; then
    mkdir -p "examples/expected/$strategy"
    cp "$out/trades.csv" "$out/daily_returns.csv" "examples/expected/$strategy/"
    echo "baseline refreshed: $strategy"
    continue
  fi
  for file in trades.csv daily_returns.csv; do
    if ! cmp -s "$out/$file" "examples/expected/$strategy/$file"; then
      echo "PARITY BROKEN: $strategy/$file differs from examples/expected"
      diff "examples/expected/$strategy/$file" "$out/$file" | head -20
      echo "If the change is intended, rerun with --refresh-baseline and explain it in the commit."
      exit 1
    fi
  done
  echo "parity ok: $strategy"
done
fi

if [ "$NO_WEB" = 0 ]; then
  step "web typecheck, lint, theme check, build, layout check"
  (cd web && npm run --silent typecheck && npm run --silent lint && npm run --silent theme-check && npm run --silent build >/dev/null && npm run --silent layout-check && npm run --silent chart-check)
fi

if [ "$QUICK" = 0 ]; then
  if [ -x "$PRIVATE_ROOT/scripts/check.sh" ]; then
    step "private checks ($PRIVATE_ROOT, engine from $ROOT)"
    TESSERA_ENGINE_ROOT="$ROOT" "$PRIVATE_ROOT/scripts/check.sh"
  else
    step "private checks"
    echo "skipped: no private checkout at $PRIVATE_ROOT (set TESSERA_PRIVATE_ROOT to point at one)"
  fi
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
