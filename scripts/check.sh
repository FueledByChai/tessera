#!/usr/bin/env bash
# The definition of done, as a command. Exits non-zero on the first failure.
#
#   scripts/check.sh                  everything: fmt, tests, build, parity, web, private checks
#   scripts/check.sh --no-web         skip the web typecheck/lint/build and the coverage
#                                     ratchet (the fast check while iterating)
#   scripts/check.sh --quick          skip web and the private checks (the CI engine job);
#                                     the coverage ratchet still runs
#   scripts/check.sh --web-only       only the web step (the CI web job); the headless layout
#                                     and chart checks skip themselves without a Chromium
#   scripts/check.sh --private-only   only the private checks (the deploy loop's post-merge
#                                     guard, HK-18); fails when no private checkout is there
#   scripts/check.sh --refresh-baseline
#                                     rewrite examples/expected from the current engine; only after
#                                     an intentional results change, and say so in the commit
#   scripts/check.sh --resolve        print what a run here would use (main checkout, private
#                                     checkout, local.toml, node_modules) and exit
#
# The proof gate (scripts/proof-gate.sh) runs in every mode but --web-only and
# --private-only: a change under src/ must bring a test, fixture, or check, or say why. The
# coverage ratchet (scripts/coverage-ratchet.sh over scripts/coverage.sh) runs in the full
# check and --quick: line coverage must not drop below coverage-floor.txt (HK-23). On a
# branch that touched no code path it skips its instrumented build, since coverage cannot
# have moved (HK-28); on the default branch itself it always runs.
#
# Worktrees: from .claude/worktrees/<name> the script finds the main checkout through the shared
# git dir, takes the private checkout beside it (or TESSERA_PRIVATE_ROOT), writes a local.toml
# from the main one with its relative paths made absolute when the worktree has none, links
# web/node_modules to the main checkout's when missing, and builds into one cargo target
# directory shared by every worktree, <main checkout>/target-worktrees (HK-27), so the
# dependency crates compile once for all of them instead of from cold per ticket. It is kept
# apart from the main checkout's own target/, whose release binaries the deploy loop and the
# console run: a worktree build must never overwrite those. TESSERA_TARGET_DIR overrides it.
# The private checks build the private legacy crate against this checkout's engine
# (TESSERA_ENGINE_ROOT), not the main one.
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
PRIVATE_ONLY=0
RATCHET=1
REFRESH=0
RESOLVE=0
for arg in "$@"; do
  case "$arg" in
    --no-web) NO_WEB=1; RATCHET=0 ;;
    --quick) QUICK=1; NO_WEB=1 ;;
    --web-only) WEB_ONLY=1; QUICK=1; RATCHET=0 ;;
    --private-only) PRIVATE_ONLY=1; NO_WEB=1; RATCHET=0 ;;
    --refresh-baseline) REFRESH=1 ;;
    --resolve) RESOLVE=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done
if [ "$WEB_ONLY" = 1 ] && [ "$NO_WEB" = 1 ]; then
  echo "--web-only and --no-web/--quick/--private-only exclude each other" >&2
  exit 2
fi
if [ "$PRIVATE_ONLY" = 1 ] && [ "$QUICK" = 1 ]; then
  echo "--private-only and --quick exclude each other" >&2
  exit 2
fi

# The main checkout: the parent of the shared git dir (`.git` here, an absolute path from a
# worktree). The private checkout sits beside it unless TESSERA_PRIVATE_ROOT says otherwise.
MAIN_ROOT="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
PRIVATE_ROOT="${TESSERA_PRIVATE_ROOT:-$MAIN_ROOT/../Tessera-private}"
if [ -d "$PRIVATE_ROOT" ]; then PRIVATE_ROOT="$(cd "$PRIVATE_ROOT" && pwd)"; fi
LOCAL_TOML_SOURCE="local.toml"
NODE_MODULES_SOURCE="web/node_modules"
TARGET_DIR="$ROOT/target"
TARGET_SOURCE="target (this checkout's own)"
if [ "$ROOT" != "$MAIN_ROOT" ]; then
  TARGET_DIR="${TESSERA_TARGET_DIR:-$MAIN_ROOT/target-worktrees}"
  export CARGO_TARGET_DIR="$TARGET_DIR"
  TARGET_SOURCE="$TARGET_DIR (shared by every worktree; the main checkout keeps its own target/)"
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
  echo "target dir:    $TARGET_SOURCE"
  exit 0
fi

step() { printf '\n== %s\n' "$1"; }
started=$(date +%s)

if [ "$WEB_ONLY" = 0 ] && [ "$PRIVATE_ONLY" = 0 ]; then
step "loop self-tests: config, backlog status, ticket PRs, release notes, kit sync, proof gate, coverage ratchet, review status, decisions, prompt check, sprint, deploy"
scripts/loop-config.sh --self-test
scripts/backlog-status.sh --self-test
scripts/open-ticket-pr.sh --self-test
scripts/release-notes.sh --self-test
scripts/loop-kit-sync.sh --self-test
scripts/proof-gate.sh --self-test
scripts/coverage-ratchet.sh --self-test
scripts/review-status.sh --self-test
scripts/decisions.sh --self-test
scripts/prompt-check.sh --self-test
scripts/sprint.sh --self-test
scripts/deploy-local.sh --self-test

# The loop scripts and prompts are copies from the kit's tagged release (HK-16, HK-19);
# drift fails the check.
step "loop kit: scripts/ and loop/prompts/ match the kit at its tag"
scripts/loop-kit-sync.sh --check

# The prompts carry their rules (HK-31): a phrase that states a rule may not be edited away;
# and the decision records, when there are any, keep their sections and their index.
step "loop prompts carry their rules; decision records are in step"
scripts/prompt-check.sh
scripts/decisions.sh --check

# The loop prompts belong to every project that adopts the loop (HK-15): nothing in loop/
# may name this project, its build tools, a harness, or its review path. (.loop.toml is
# exempt: it holds this project's own values.) The first-day interview, the check
# skeletons, and the CI toolchain steps (HK-32, HK-38) name every stack's tools on purpose,
# so only the project and the harness are forbidden there.
step "loop prompts name no project, build tool, or harness"
if grep -rniE 'tessera|claude|examples/expected' loop/ \
   || grep -rniE 'cargo|npm' loop/ --exclude-dir=check --exclude-dir=ci --exclude=grill-project.md; then
  echo "loop/ must stay generic: the lines above name the project, a build tool, or a harness" >&2
  exit 1
fi
echo "loop/ is generic"

# A change under src/ must bring a test, fixture, or check (HK-23; the paths and the pattern
# are in .loop.toml). Judged against origin/main, so CI fetches the history it needs.
step "proof gate: code changes bring a proof"
scripts/proof-gate.sh

step "cargo fmt --check"
cargo fmt --all --check

step "cargo test --release"
cargo test --release --quiet 2>&1 | grep -E "test result|FAILED|panicked|error(\[|:)" || true
cargo test --release --quiet >/dev/null 2>&1

step "cargo build --release (tessera, tessera-ui)"
cargo build --release --quiet --bin tessera --bin tessera-ui

step "parity against examples/expected"
# Per-run outputs stay under this checkout's own target/: the shared directory holds the
# build cache only, so two worktrees checking at once cannot delete each other's outputs.
for strategy in rsi_mean_reversion moving_average_cross; do
  out="$ROOT/target/check_$strategy"
  rm -rf "$out"
  "$TARGET_DIR/release/tessera" run-strategy \
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

if [ "$RATCHET" = 1 ]; then
  step "coverage ratchet: line coverage against coverage-floor.txt"
  # Coverage moves only when code does: a branch with no change under a code path skips the
  # instrumented build (HK-28). The default branch itself (HEAD is origin/<default>) and any
  # checkout without an origin always measure.
  DEFAULT_BRANCH="$(scripts/loop-config.sh default_branch)"
  ORIGIN_HEAD="$(git rev-parse -q --verify "origin/$DEFAULT_BRANCH" 2>/dev/null || echo none)"
  if [ "$(git rev-parse HEAD)" = "$ORIGIN_HEAD" ] || [ "$ORIGIN_HEAD" = none ] || scripts/proof-gate.sh --code-changed >/dev/null; then
    scripts/coverage-ratchet.sh
  else
    echo "coverage ratchet: skipped, no code change against origin/$DEFAULT_BRANCH"
  fi
fi

if [ "$NO_WEB" = 0 ]; then
  step "web typecheck, lint, theme check, build, layout check"
  (cd web && npm run --silent typecheck && npm run --silent lint && npm run --silent theme-check && npm run --silent build >/dev/null && npm run --silent layout-check && npm run --silent chart-check)
fi

if [ "$QUICK" = 0 ]; then
  if [ -x "$PRIVATE_ROOT/scripts/check.sh" ]; then
    step "private checks ($PRIVATE_ROOT, engine from $ROOT)"
    TESSERA_ENGINE_ROOT="$ROOT" "$PRIVATE_ROOT/scripts/check.sh"
  elif [ "$PRIVATE_ONLY" = 1 ]; then
    step "private checks"
    echo "no private checkout at $PRIVATE_ROOT (set TESSERA_PRIVATE_ROOT to point at one)" >&2
    exit 1
  else
    step "private checks"
    echo "skipped: no private checkout at $PRIVATE_ROOT (set TESSERA_PRIVATE_ROOT to point at one)"
  fi
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
