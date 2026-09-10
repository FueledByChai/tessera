#!/usr/bin/env bash
# Line coverage of the crate as one percentage on stdout (HK-23), the `coverage` command the
# loop's ratchet reads (.loop.toml, scripts/coverage-ratchet.sh). cargo-llvm-cov runs the test
# suite instrumented in its own target directory (target/llvm-cov-target), so it does not
# disturb the release build the rest of the check uses. Files outside this checkout (the
# private strategies compiled in through local.toml, the registry, the toolchain) are left out
# of the measurement; everything under src/, the service binary included, is in.
#
#   scripts/coverage.sh           print the line percentage (e.g. 71.4)
#   scripts/coverage.sh --report  the per-file table instead, for finding what to test
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  cat >&2 <<'EOF'
coverage: cargo-llvm-cov is not installed. Install it once (about two minutes):
  rustup component add llvm-tools-preview
  cargo install cargo-llvm-cov
CI installs it itself; locally the check fails until it is there.
EOF
  exit 1
fi
IGNORE='(/Tessera-private/|/\.cargo/registry/|/rustc/|/\.rustup/)'
if [ "${1:-}" = "--report" ]; then
  cargo llvm-cov --release --summary-only --ignore-filename-regex "$IGNORE"
  exit 0
fi
cargo llvm-cov --release --json --summary-only --ignore-filename-regex "$IGNORE" 2>/dev/null \
  | python3 -c 'import json, sys; print("%.1f" % json.load(sys.stdin)["data"][0]["totals"]["lines"]["percent"])'
