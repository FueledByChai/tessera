#!/bin/sh
# Builds what is missing, starts the single Tessera process, and opens the console.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
STATE_DIR="$ROOT/data/ui"
LOG_DIR="$STATE_DIR/logs"
URL="http://127.0.0.1:8787/"

mkdir -p "$LOG_DIR"

if ! curl -fsS "$URL/api/health" >/dev/null 2>&1; then
  # Console bundle: build when missing or when the source is newer than the bundle.
  if [ ! -f "$ROOT/web/dist/index.html" ] || [ -n "$(find "$ROOT/web/app" "$ROOT/web/src" -newer "$ROOT/web/dist/index.html" 2>/dev/null | head -1)" ]; then
    osascript -e 'display notification "Building the console bundle" with title "Tessera"' >/dev/null 2>&1 || true
    if [ ! -x "$ROOT/web/node_modules/.bin/vite" ]; then
      (cd "$ROOT/web" && npm ci --no-audit --no-fund) >"$LOG_DIR/npm-install.log" 2>&1
    fi
    (cd "$ROOT/web" && npm run build) >"$LOG_DIR/web-build.log" 2>&1
  fi
  # Engine and service binaries.
  if [ ! -x "$ROOT/target/release/tessera-ui" ] || [ ! -x "$ROOT/target/release/tessera" ]; then
    osascript -e 'display notification "Building the engine (first launch takes a few minutes)" with title "Tessera"' >/dev/null 2>&1 || true
    (cd "$ROOT" && cargo build --release --bin tessera-ui --bin tessera) >"$LOG_DIR/build.log" 2>&1
  fi
  (cd "$ROOT" && TESSERA_ROOT="$ROOT" nohup "$ROOT/target/release/tessera-ui" >"$LOG_DIR/api.log" 2>&1 & echo $! >"$STATE_DIR/api.pid")
fi

attempt=0
until curl -fsS "$URL/api/health" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 60 ]; then
    osascript -e 'display alert "Tessera did not start" message "Check data/ui/logs/api.log." as critical'
    exit 1
  fi
  sleep 1
done

open "$URL"
