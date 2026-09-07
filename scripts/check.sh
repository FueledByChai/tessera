#!/usr/bin/env bash
# The definition of done, as a command. Exits non-zero on the first failure.
#
#   scripts/check.sh                  everything: fmt, tests, build, parity, web, private checks
#   scripts/check.sh --no-web         skip the web typecheck/lint/build (slow on iCloud checkouts)
#   scripts/check.sh --quick          skip web and the private checks
#   scripts/check.sh --refresh-baseline
#                                     rewrite examples/expected from the current engine; only after
#                                     an intentional results change, and say so in the commit
#
# Parity: the two bundled examples run against the synthetic data and their trades and daily
# equity must match examples/expected byte for byte. A behaviour change in the engine, the SDK,
# or the costs model shows up here before it shows up in a research result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

NO_WEB=0
QUICK=0
REFRESH=0
for arg in "$@"; do
  case "$arg" in
    --no-web) NO_WEB=1 ;;
    --quick) QUICK=1; NO_WEB=1 ;;
    --refresh-baseline) REFRESH=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

step() { printf '\n== %s\n' "$1"; }
started=$(date +%s)

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

if [ "$NO_WEB" = 0 ]; then
  step "web typecheck, lint, theme check, build"
  (cd web && npm run --silent typecheck && npm run --silent lint && npm run --silent theme-check && npm run --silent build >/dev/null)
fi

if [ "$QUICK" = 0 ] && [ -x "$ROOT/../Tessera-private/scripts/check.sh" ]; then
  step "private checks"
  "$ROOT/../Tessera-private/scripts/check.sh"
fi

printf '\nALL CHECKS PASSED (%ss)\n' "$(( $(date +%s) - started ))"
