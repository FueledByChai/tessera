#!/usr/bin/env bash
# A throwaway console on the bundled example data, for the headless browser checks (HK-07):
# a scratch TESSERA_ROOT under target/ that points at this checkout's examples, strategy
# sources, engine, and web bundle, one completed run of a bundled example strategy, and one
# completed daily study, so the layout check has a run page, a strategy page, and a study to
# open. CI starts it before `scripts/check.sh --web-only` and stops it afterwards; locally a
# second port keeps it clear of the real console on 8787.
#
#   scripts/scratch-console.sh start [--port 8787]    build the root, start, seed, wait
#   scripts/scratch-console.sh start --empty [--port N]
#                                                     an empty catalog instead: no engine link, so
#                                                     no strategies sync, and nothing seeded (the
#                                                     layout check's skip path, HK-08)
#   scripts/scratch-console.sh root  [--port 8787]    (re)build the root and print it, start
#                                                     nothing; it removes an existing root
#   scripts/scratch-console.sh stop  [--port 8787]    stop it and remove the root
#   scripts/scratch-console.sh url   [--port 8787]    print the console origin
#   scripts/scratch-console.sh --self-test            a fixture checkout proves the root's `target`
#                                                     link is the directory the binaries were
#                                                     built into (HK-43)
#
# Needs target/release/tessera and target/release/tessera-ui, from the checkout's own `target/`
# or the shared one `scripts/check.sh` points CARGO_TARGET_DIR at from a worktree (cargo build
# --release).
set -euo pipefail
SCRIPT="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
# The checkout the root is built from. --self-test points this at a fixture; nothing else sets it.
ROOT="${TESSERA_SCRATCH_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
PORT=8787
EMPTY=0
COMMAND="${1:-}"
shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift ;;
    --empty) EMPTY=1 ;;
    *) echo "unknown flag: $1" >&2; exit 2 ;;
  esac
  shift
done
# The binaries come from the checkout's own target/, or the shared worktree one when
# scripts/check.sh set CARGO_TARGET_DIR (HK-27); the scratch root is per checkout regardless.
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
SCRATCH="$ROOT/target/scratch-console-$PORT"
ORIGIN="http://127.0.0.1:$PORT"
API="$ORIGIN/api"

api() {  # method path [json]
  if [ $# -ge 3 ]; then
    curl -sS -X "$1" -H 'Content-Type: application/json' -d "$3" "$API$2"
  else
    curl -sS -X "$1" "$API$2"
  fi
}

wait_for() {  # a python3 predicate over the JSON at path, polled for up to 300 s
  local path="$1" predicate="$2" label="$3"
  for _ in $(seq 1 150); do
    if api GET "$path" | python3 -c "import sys, json; d = json.load(sys.stdin); sys.exit(0 if ($predicate) else 1)" 2>/dev/null; then
      return 0
    fi
    sleep 2
  done
  echo "scratch console: timed out waiting for $label" >&2
  return 1
}

# Builds the scratch root: every link points at the checkout, except `target`, which has to be
# the directory the binaries were actually built into. The service resolves the engine it syncs
# SDK strategies with at `<root>/target/release/tessera`, and scripts/check.sh builds a
# worktree's binaries into the shared CARGO_TARGET_DIR instead (HK-27) — so linking the
# checkout's own `target/` there found no engine at all, and `start` waited out its timeout for
# a catalog that could never sync (HK-43). The `--empty` root takes no engine link on purpose.
prepare_scratch() {
  rm -rf "$SCRATCH"
  mkdir -p "$SCRATCH/data/ui" "$SCRATCH/artifacts"
  local links="examples src target web" link
  [ "$EMPTY" = 1 ] && links="examples src web"
  for link in $links; do
    case "$link" in
      target) ln -s "$TARGET_DIR" "$SCRATCH/target" ;;
      *) ln -s "$ROOT/$link" "$SCRATCH/$link" ;;
    esac
  done
}

# A fixture checkout in the shape scripts/check.sh leaves a worktree in (HK-27): its own
# `target/` holds no binaries, the shared directory does. `root` builds the root and starts
# nothing, which is the step HK-43 was wrong in.
self_test() {
  [ -n "${TESSERA_SCRATCH_TRACE:-}" ] && set -x
  # A global, not a `local`: the EXIT trap fires after this function has returned, and `set -u`
  # would make the trap itself the failure.
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/tessera-scratch-console.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" binary root
  mkdir -p "$dir/checkout/target" "$dir/checkout/examples" "$dir/checkout/src" \
           "$dir/checkout/web" "$dir/shared/release"
  for binary in tessera tessera-ui; do
    printf '#!/bin/sh\nexit 0\n' > "$dir/shared/release/$binary"
    chmod +x "$dir/shared/release/$binary"
  done
  root_of() {  # <shared target dir, or "" for none> [extra flags] - prints the root it built
    local shared="$1"; shift
    if [ -n "$shared" ]; then
      TESSERA_SCRATCH_ROOT="$dir/checkout" CARGO_TARGET_DIR="$shared" \
        bash "$SCRIPT" root --port 8793 "$@"
    else
      # `env -u`: scripts/check.sh exports CARGO_TARGET_DIR for a worktree, and this case is the
      # main checkout's shape, where it is not set at all.
      env -u CARGO_TARGET_DIR TESSERA_SCRATCH_ROOT="$dir/checkout" \
        bash "$SCRIPT" root --port 8793 "$@"
    fi
  }

  # 1. A worktree with the shared target dir: the engine the service resolves is the built one.
  root="$(root_of "$dir/shared")"
  [ "$root" = "$dir/checkout/target/scratch-console-8793" ] || { echo "self-test: the root is $root"; exit 1; }
  [ "$(readlink "$root/target")" = "$dir/shared" ] || { echo "self-test: the root's target link is $(readlink "$root/target"), not the shared target directory the binaries were built into"; exit 1; }
  for binary in tessera tessera-ui; do
    [ -x "$root/target/release/$binary" ] || { echo "self-test: no engine at $root/target/release/$binary, which is where the service looks for it"; exit 1; }
  done
  [ "$(readlink "$root/examples")" = "$dir/checkout/examples" ] || { echo "self-test: the root's examples link left the checkout"; exit 1; }

  # 2. No CARGO_TARGET_DIR - the main checkout's shape, which is what CI starts: its own target.
  root="$(root_of "")"
  [ "$(readlink "$root/target")" = "$dir/checkout/target" ] || { echo "self-test: without CARGO_TARGET_DIR the root's target link is $(readlink "$root/target"), not the checkout's own target/"; exit 1; }

  # 3. --empty takes no engine link at all, so the skip path is unchanged (HK-08).
  root="$(root_of "$dir/shared" --empty)"
  [ ! -e "$root/target" ] || { echo "self-test: --empty linked an engine"; exit 1; }
  [ -L "$root/web" ] || { echo "self-test: --empty did not link the checkout"; exit 1; }

  echo "scratch-console self-test passed"
}

case "$COMMAND" in
  --self-test) self_test ;;
  url) echo "$ORIGIN/" ;;
  root) prepare_scratch; echo "$SCRATCH" ;;
  start)
    for binary in tessera tessera-ui; do
      [ -x "$TARGET_DIR/release/$binary" ] || { echo "scratch console: build $TARGET_DIR/release/$binary first" >&2; exit 1; }
    done
    if curl -sf "$API/health" >/dev/null 2>&1; then
      echo "scratch console: something already answers on $ORIGIN" >&2
      exit 1
    fi
    prepare_scratch
    (
      cd "$SCRATCH"
      TESSERA_ROOT="$SCRATCH" TESSERA_ADDR="127.0.0.1:$PORT" nohup "$TARGET_DIR/release/tessera-ui" > "$SCRATCH/api.log" 2>&1 &
      echo $! > "$SCRATCH/api.pid"
    )
    for _ in $(seq 1 60); do curl -sf "$API/health" >/dev/null 2>&1 && break; sleep 1; done
    curl -sf "$API/health" >/dev/null || { echo "scratch console: did not come up; log:" >&2; cat "$SCRATCH/api.log" >&2; exit 1; }
    echo "scratch console: up on $ORIGIN (pid $(cat "$SCRATCH/api.pid"), root $SCRATCH)"
    if [ "$EMPTY" = 1 ]; then
      echo "scratch console: empty catalog (no strategies, no runs, no studies); LAYOUT_CONSOLE=$ORIGIN/"
      exit 0
    fi
    # The service syncs the compiled strategies into its catalog in the background after it
    # starts answering; a job for one is refused until that has happened.
    wait_for /dashboard "any(s['id'] == 'moving_average_cross' for s in d['strategies'])" "the strategy catalog"
    # One run of a bundled example on the synthetic daily data.
    job="$(api POST /jobs '{"strategy_id":"moving_average_cross","name":"scratch: moving average cross","start_date":"2024-01-02","end_date":"2024-06-28","parameters":{"symbols":["DEMO.US"],"resolution":"daily"}}')"
    job_id="$(echo "$job" | python3 -c 'import sys, json; print(json.load(sys.stdin)["id"])')" || { echo "scratch console: job not accepted: $job" >&2; exit 1; }
    wait_for /jobs "any(j['id'] == '$job_id' and j['status'] not in ('queued', 'running') for j in d)" "the seed run"
    api GET /jobs | python3 -c "import sys, json; j = [x for x in json.load(sys.stdin) if x['id'] == '$job_id'][0]; print('scratch console: run', j['status'], j.get('error') or ''); sys.exit(0 if j['status'] == 'complete' else 1)"
    # One daily study on the same data.
    study="$(api POST /studies '{"name":"scratch: DEMO.US daily","start_date":"2019-01-02","end_date":"2025-12-31","symbols":["DEMO.US"],"step_secs":1,"resolution":"daily","features":["return_1","range_bps"],"horizons":[1,5,21],"decision_delay_bars":1,"accepted":[]}')"
    study_id="$(echo "$study" | python3 -c 'import sys, json; print(json.load(sys.stdin)["id"])')" || { echo "scratch console: study not accepted: $study" >&2; exit 1; }
    wait_for /studies "any(s['id'] == '$study_id' and s['status'] != 'running' for s in d)" "the seed study"
    api GET /studies | python3 -c "import sys, json; s = [x for x in json.load(sys.stdin) if x['id'] == '$study_id'][0]; print('scratch console: study', s['status'], s.get('error') or ''); sys.exit(0 if s['status'] == 'complete' else 1)"
    echo "scratch console: seeded; LAYOUT_CONSOLE=$ORIGIN/"
    ;;
  stop)
    if [ -f "$SCRATCH/api.pid" ]; then
      kill "$(cat "$SCRATCH/api.pid")" 2>/dev/null || true
      sleep 1
    fi
    rm -rf "$SCRATCH"
    echo "scratch console: stopped and removed"
    ;;
  *) echo "usage: scripts/scratch-console.sh start|stop|url [--port N]" >&2; exit 2 ;;
esac
