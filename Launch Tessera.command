#!/bin/sh
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
STATE_DIR="$ROOT/data/ui"
LOG_DIR="$STATE_DIR/logs"
API_URL="http://127.0.0.1:8787/api/health"
WEB_URL="http://127.0.0.1:3322/"
WEB_RUNTIME="$HOME/Library/Caches/Tessera/web-runtime"

mkdir -p "$LOG_DIR"

if ! curl -fsS "$API_URL" >/dev/null 2>&1; then
  if [ ! -x "$ROOT/target/release/tessera-ui" ]; then
    osascript -e 'display notification "Building the local service for the first launch" with title "Tessera"'
    (cd "$ROOT" && cargo build --release --bin tessera-ui --bin tessera) >"$LOG_DIR/build.log" 2>&1
  fi
  (cd "$ROOT" && TESSERA_ROOT="$ROOT" nohup "$ROOT/target/release/tessera-ui" >"$LOG_DIR/api.log" 2>&1 & echo $! >"$STATE_DIR/api.pid")
fi

if ! curl -fsS "$WEB_URL" >/dev/null 2>&1; then
  mkdir -p "$WEB_RUNTIME"
  rsync -a --delete \
    --exclude node_modules \
    --exclude dist \
    --exclude .next \
    --exclude .vinext \
    --exclude .package-lock.sha256 \
    "$ROOT/web/" "$WEB_RUNTIME/"

  lock_hash="$(shasum -a 256 "$ROOT/web/package-lock.json" | awk '{print $1}')"
  installed_hash="$(cat "$WEB_RUNTIME/.package-lock.sha256" 2>/dev/null || true)"
  if [ ! -x "$WEB_RUNTIME/node_modules/.bin/vinext" ] || [ "$lock_hash" != "$installed_hash" ]; then
    (cd "$WEB_RUNTIME" && npm ci --no-audit --no-fund) >"$LOG_DIR/npm-install.log" 2>&1
    printf '%s\n' "$lock_hash" >"$WEB_RUNTIME/.package-lock.sha256"
  fi
  (cd "$WEB_RUNTIME" && npm run build) >"$LOG_DIR/web-build.log" 2>&1
  (cd "$WEB_RUNTIME" && nohup "$WEB_RUNTIME/node_modules/.bin/vinext" start --host 127.0.0.1 --port 3322 </dev/null >"$LOG_DIR/web.log" 2>&1 & echo $! >"$STATE_DIR/web.pid")
fi

attempt=0
until curl -fsS "$API_URL" >/dev/null 2>&1 && curl -fsS "$WEB_URL" >/dev/null 2>&1; do
  attempt=$((attempt + 1))
  if [ "$attempt" -ge 60 ]; then
    osascript -e 'display alert "Tessera did not start" message "Check data/ui/logs/api.log and web.log." as critical'
    exit 1
  fi
  sleep 1
done

open "$WEB_URL"
