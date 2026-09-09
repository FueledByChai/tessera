#!/usr/bin/env bash
# The local deploy loop (HK-11): brings the console on this machine up to origin/main. Pulls
# main fast-forward, rebuilds what the new commits touched (the engine for src/, Cargo, build.rs,
# or strategy changes; the bundle for web/ changes), and restarts the service only when an
# engine change needs it and nothing is running in the catalog. Meant to run every few minutes
# from a schedule; every run is one line in data/ui/deploy.log.
#
#   scripts/deploy-local.sh                 deploy if origin/main moved (exit 0; 3 when refused)
#   scripts/deploy-local.sh --dry-run       say what a run would do, change nothing
#   scripts/deploy-local.sh --launchd       print a LaunchAgent plist that runs it every 5 min
#   scripts/deploy-local.sh --self-test     a fixture origin and a stubbed service prove the
#                                           decisions: nothing new, engine change, web change,
#                                           docs change, and a running job
#
# Decisions, in order: not on a clean main → refuse. origin/main not ahead → nothing new, exit
# at once. A job or study running → refuse before pulling, so the checkout stays what the
# service runs. Otherwise pull, build what changed, restart when the engine changed (the pid in
# data/ui/api.pid, checked against the port's listener), wait for /api/health, print the pid.
set -euo pipefail
# TESSERA_DEPLOY_ROOT deploys another checkout (the self-test's fixture); default: this one.
ROOT="${TESSERA_DEPLOY_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
MODE=deploy
for arg in "$@"; do
  case "$arg" in
    --dry-run) MODE=dry ;;
    --launchd) MODE=launchd ;;
    --self-test) MODE=selftest ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# The service, as the loop sees it. TESSERA_DEPLOY_STUB=<dir> replaces every call with files
# in that directory (busy, pid, built, restarted), which is how the self-test runs without a
# console or a compiler.
ADDR="${TESSERA_ADDR:-127.0.0.1:8787}"
PORT="${ADDR##*:}"
STUB="${TESSERA_DEPLOY_STUB:-}"

log() { printf '%s %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$*" | tee -a "$ROOT/data/ui/deploy.log" >&2; }

running_work() {  # prints "N job(s), M study(ies)" when something runs, nothing otherwise
  if [ -n "$STUB" ]; then
    [ "$(cat "$STUB/busy" 2>/dev/null || echo 0)" = 1 ] && echo "1 job(s) (stub)"
    return 0
  fi
  local jobs studies
  jobs="$(curl -sf "http://$ADDR/api/jobs" | python3 -c 'import sys,json; print(sum(1 for x in json.load(sys.stdin) if x.get("status")=="running"))' 2>/dev/null || echo 0)"
  studies="$(curl -sf "http://$ADDR/api/studies" | python3 -c 'import sys,json; print(sum(1 for x in json.load(sys.stdin) if x.get("status")=="running"))' 2>/dev/null || echo 0)"
  [ "$jobs" = 0 ] && [ "$studies" = 0 ] || echo "$jobs job(s), $studies study(ies)"
}

listener_pid() {
  if [ -n "$STUB" ]; then cat "$STUB/pid" 2>/dev/null || true; return 0; fi
  lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null | head -1 || true
}

build_engine() {
  if [ -n "$STUB" ]; then echo engine >> "$STUB/built"; return 0; fi
  (cd "$ROOT" && cargo build --release --quiet --bin tessera --bin tessera-ui)
}

build_web() {
  if [ -n "$STUB" ]; then echo web >> "$STUB/built"; return 0; fi
  (cd "$ROOT/web" && npm run --silent build >/dev/null)
}

restart_service() {  # prints the new pid
  if [ -n "$STUB" ]; then
    local old; old="$(listener_pid)"
    echo "$((${old:-1000} + 1))" > "$STUB/pid"
    echo "restarted from ${old:-none}" >> "$STUB/restarted"
    cat "$STUB/pid"
    return 0
  fi
  local pid recorded
  pid="$(listener_pid)"
  recorded="$(cat "$ROOT/data/ui/api.pid" 2>/dev/null || true)"
  if [ -n "$recorded" ] && [ "$recorded" != "$pid" ]; then
    log "note: data/ui/api.pid says $recorded but the listener on $PORT is ${pid:-nobody}; using the listener"
  fi
  if [ -n "$pid" ]; then
    kill "$pid" 2>/dev/null || true
    for _ in $(seq 1 30); do [ -z "$(listener_pid)" ] && break; sleep 1; done
    [ -z "$(listener_pid)" ] || { log "the old service ($pid) did not stop"; return 1; }
  fi
  (cd "$ROOT" && TESSERA_ROOT="$ROOT" nohup ./target/release/tessera-ui > data/ui/api.log 2>&1 &
   echo $! > data/ui/api.pid)
  for _ in $(seq 1 60); do curl -sf "http://$ADDR/api/health" >/dev/null 2>&1 && break; sleep 1; done
  curl -sf "http://$ADDR/api/health" >/dev/null 2>&1 || { log "the new service did not answer on $ADDR; see data/ui/api.log"; return 1; }
  cat "$ROOT/data/ui/api.pid"
}

# Which kinds of files a commit range touched: engine, web, other.
touched() {  # $1 old $2 new
  local engine=0 web=0 other=0
  while IFS= read -r file; do
    case "$file" in
      src/*|Cargo.toml|Cargo.lock|build.rs|strategies/*) engine=1 ;;
      web/app/*|web/src/*|web/index.html|web/package.json|web/package-lock.json|web/vite.config.*|web/tsconfig*.json) web=1 ;;
      *) other=1 ;;
    esac
  done < <(git -C "$ROOT" diff --name-only "$1" "$2")
  echo "$engine $web $other"
}

deploy() {  # $1: dry (1) or real (0)
  local dry="$1"
  cd "$ROOT"
  mkdir -p data/ui
  local branch; branch="$(git rev-parse --abbrev-ref HEAD)"
  if [ "$branch" != "main" ]; then log "refusing: checkout is on $branch, not main"; return 3; fi
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then log "refusing: main has local changes"; return 3; fi
  git fetch -q origin main
  local old new
  old="$(git rev-parse HEAD)"; new="$(git rev-parse origin/main)"
  if [ "$old" = "$new" ]; then log "nothing new: main is at $(git rev-parse --short "$old")"; return 0; fi
  if ! git merge-base --is-ancestor "$old" "$new"; then log "refusing: origin/main ($(git rev-parse --short "$new")) is not a fast-forward of main ($(git rev-parse --short "$old"))"; return 3; fi
  local busy; busy="$(running_work)"
  if [ -n "$busy" ]; then log "refusing: $busy running; $(git rev-list --count "$old..$new") new commit(s) wait for the next run"; return 3; fi
  read -r engine web other <<< "$(touched "$old" "$new")"
  local plan="pull $(git rev-parse --short "$old")..$(git rev-parse --short "$new")"
  [ "$engine" = 1 ] && plan="$plan, build engine, restart"
  [ "$web" = 1 ] && plan="$plan, build web"
  [ "$engine" = 0 ] && [ "$web" = 0 ] && plan="$plan, nothing to build (docs or scripts only)"
  if [ "$dry" = 1 ]; then log "dry run: would $plan"; return 0; fi
  git pull -q --ff-only origin main
  log "pulled $(git rev-parse --short "$old")..$(git rev-parse --short "$new"): $(git log -1 --format=%s)"
  [ "$engine" = 1 ] && { build_engine; log "built the engine"; }
  [ "$web" = 1 ] && { build_web; log "built the web bundle"; }
  if [ "$engine" = 1 ]; then
    local pid; pid="$(restart_service)"
    log "restarted the service: pid $pid"
    echo "$pid"
  else
    log "no restart needed"
  fi
  return 0
}

launchd_plist() {
  cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.tessera.deploy-local</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$ROOT/scripts/deploy-local.sh</string></array>
  <key>StartInterval</key><integer>300</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>$ROOT/data/ui/deploy-launchd.log</string>
  <key>StandardErrorPath</key><string>$ROOT/data/ui/deploy-launchd.log</string>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$HOME/.cargo/bin</string></dict>
</dict>
</plist>
EOF
  cat >&2 <<EOF

Install: save this as ~/Library/LaunchAgents/com.tessera.deploy-local.plist, then
  launchctl bootstrap gui/\$(id -u) ~/Library/LaunchAgents/com.tessera.deploy-local.plist
Remove:
  launchctl bootout gui/\$(id -u)/com.tessera.deploy-local
EOF
}

self_test() {
  # TESSERA_DEPLOY_TRACE=1 traces the fixture steps when a step fails silently.
  [ -n "${TESSERA_DEPLOY_TRACE:-}" ] && set -x
  SELF_TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/tessera-deploy-local.XXXXXX")"
  trap 'rm -rf "$SELF_TEST_DIR"' EXIT
  local dir="$SELF_TEST_DIR" script="$ROOT/scripts/deploy-local.sh"
  (
    cd "$dir"
    git init -q --bare origin.git
    git clone -q origin.git upstream 2>/dev/null
    cd upstream
    git config user.email "self-test@example.com"; git config user.name "self-test"
    git checkout -q -b main
    mkdir -p src web/app docs scripts
    echo "fn main() {}" > src/main.rs; echo "body {}" > web/app/globals.css; echo "# docs" > docs/README.md
    git add -A; git commit -q -m "Scaffold"; git push -q -u origin main
    cd "$dir"
    git clone -q -b main origin.git deploy
    mkdir -p deploy/data/ui stub
    echo 0 > stub/busy; echo 4242 > stub/pid
    export TESSERA_DEPLOY_STUB="$dir/stub"
    export TESSERA_DEPLOY_ROOT="$dir/deploy"
    run() { (cd "$dir/deploy" && bash "$script" "$@" 2>"$dir/last.err"); }
    # 1. Nothing new: exits 0 at once, touching nothing.
    rc=0; run || rc=$?
    [ "$rc" = 0 ] && grep -q 'nothing new' "$dir/last.err" || { echo "self-test: nothing-new run: rc $rc"; cat "$dir/last.err"; exit 1; }
    [ ! -e stub/built ] && [ ! -e stub/restarted ] || { echo "self-test: nothing-new run touched the service"; exit 1; }
    # 2. An engine change on origin: pull, build the engine, restart, print the new pid.
    (cd upstream && echo "fn main() { println!(\"hi\"); }" > src/main.rs && git commit -q -am "WB-99: engine change" && git push -q origin main)
    rc=0; pid="$(run)" || rc=$?
    [ "$rc" = 0 ] && [ "$pid" = 4243 ] || { echo "self-test: engine change: rc $rc pid '$pid'"; cat "$dir/last.err"; exit 1; }
    grep -q '^engine$' stub/built && grep -q 'restarted from 4242' stub/restarted || { echo "self-test: engine change did not build and restart"; exit 1; }
    [ "$(git -C deploy rev-parse HEAD)" = "$(git -C upstream rev-parse HEAD)" ] || { echo "self-test: deploy checkout did not pull"; exit 1; }
    rm -f stub/built stub/restarted
    # 3. A web-only change: pull, build the bundle, no restart.
    (cd upstream && echo "body { color: red }" > web/app/globals.css && git commit -q -am "UI-99: web change" && git push -q origin main)
    rc=0; run || rc=$?
    [ "$rc" = 0 ] && grep -q '^web$' stub/built && [ ! -e stub/restarted ] && grep -q 'no restart needed' "$dir/last.err" || { echo "self-test: web change: rc $rc"; cat "$dir/last.err"; ls stub; exit 1; }
    grep -q '^engine$' stub/built && { echo "self-test: web change rebuilt the engine"; exit 1; }
    rm -f stub/built
    # 4. Docs only: pull, nothing to build, no restart.
    (cd upstream && echo "more" >> docs/README.md && git commit -q -am "HK-99: docs" && git push -q origin main)
    rc=0; run || rc=$?
    [ "$rc" = 0 ] && [ ! -e stub/built ] && [ ! -e stub/restarted ] && grep -q 'no restart needed' "$dir/last.err" || { echo "self-test: docs change: rc $rc"; cat "$dir/last.err"; exit 1; }
    # 5. A running job: refuse before pulling, say why, exit 3; the next run (idle) deploys.
    (cd upstream && echo "fn main() { println!(\"again\"); }" > src/main.rs && git commit -q -am "WB-98: another engine change" && git push -q origin main)
    echo 1 > stub/busy
    before="$(git -C deploy rev-parse HEAD)"
    rc=0; run || rc=$?
    [ "$rc" = 3 ] && grep -q 'refusing: 1 job(s) (stub) running' "$dir/last.err" || { echo "self-test: busy run: rc $rc"; cat "$dir/last.err"; exit 1; }
    [ "$(git -C deploy rev-parse HEAD)" = "$before" ] && [ ! -e stub/restarted ] || { echo "self-test: busy run pulled or restarted"; exit 1; }
    echo 0 > stub/busy
    rc=0; pid="$(run)" || rc=$?
    [ "$rc" = 0 ] && [ "$pid" = 4244 ] || { echo "self-test: idle run after busy: rc $rc pid '$pid'"; cat "$dir/last.err"; exit 1; }
    # 6. A dry run reports the plan and changes nothing.
    (cd upstream && echo "fn main() { println!(\"dry\"); }" > src/main.rs && git commit -q -am "WB-97: dry" && git push -q origin main)
    rm -f stub/built stub/restarted
    rc=0; run --dry-run || rc=$?
    [ "$rc" = 0 ] && grep -q 'dry run: would pull .*build engine, restart' "$dir/last.err" && [ ! -e stub/built ] || { echo "self-test: dry run: rc $rc"; cat "$dir/last.err"; exit 1; }
    # 7. Not on main, or dirty: refuse.
    (cd deploy && git checkout -q -b elsewhere) ; rc=0; run || rc=$?
    [ "$rc" = 3 ] && grep -q 'not main' "$dir/last.err" || { echo "self-test: off-main run: rc $rc"; cat "$dir/last.err"; exit 1; }
    (cd deploy && git checkout -q main && echo "local" >> src/main.rs); rc=0; run || rc=$?
    [ "$rc" = 3 ] && grep -q 'local changes' "$dir/last.err" || { echo "self-test: dirty run: rc $rc"; cat "$dir/last.err"; exit 1; }
  )
  echo "deploy-local self-test passed"
}

case "$MODE" in
  deploy) deploy 0 ;;
  dry) deploy 1 ;;
  launchd) launchd_plist ;;
  selftest) self_test ;;
esac
