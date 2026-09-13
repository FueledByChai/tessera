#!/usr/bin/env bash
# The local deploy loop (HK-11): brings the console on this machine up to origin/main. Pulls
# main fast-forward, rebuilds what the commits since the last build touched (the engine for
# src/, Cargo, build.rs, or strategy changes; the bundle for web/ changes), runs the private
# checks against the new engine (HK-18: public CI cannot, so this is the post-merge guard), and
# restarts the service only when an engine change needs it, the private checks pass, and
# nothing is running in the catalog. What to rebuild is keyed on data/ui/deployed, the sha each
# artifact was last built from, and not on whether this run did the pull (HK-44): a hand pull in
# the main checkout advances HEAD, so the pull delta reads as "nothing new" while the artifacts
# on disk are a build behind. Meant to run every few minutes from a schedule; every run is one
# line in data/ui/deploy.log.
#
#   scripts/deploy-local.sh                 deploy if origin/main moved (exit 0; 3 when refused;
#                                           4 when the private checks failed or the restart
#                                           did not take, and the service keeps its previous
#                                           build)
#   scripts/deploy-local.sh --dry-run       say what a run would do, change nothing
#   scripts/deploy-local.sh --launchd       print a LaunchAgent plist that runs it every 5 min
#   scripts/deploy-local.sh --self-test     a fixture origin and a stubbed service prove the
#                                           decisions: nothing new, engine change, web change,
#                                           docs change, a running job, the private checks
#                                           failing (no restart) and passing (restart), and a
#                                           restart that does not take because the old
#                                           service still holds the port, a start_service
#                                           that returns from inside $(...) with the pid
#                                           file naming the live process, and a LaunchAgent
#                                           that abandons its process group (HK-47); and the
#                                           marker the decisions are keyed on — matching
#                                           origin/main with nothing rebuilt, and left behind
#                                           by a hand pull with the engine rebuilt and the
#                                           service restarted anyway (HK-44)
#   scripts/deploy-local.sh --start-service internal, the self-test's: start_service alone
#                                           against $TESSERA_ADDR (refuses a port in use)
#
# TESSERA_DEPLOY_HEALTH_WAIT is the seconds start_service waits for /api/health (default 60).
#
# Decisions, in order: not on a clean main → refuse. Nothing to build and nothing to pull →
# nothing new, exit at once (saying so if the last engine build failed its private checks or its
# restart did not take, and the service is still on the build before). A job or study running →
# refuse before pulling, so the checkout stays what the service runs. Otherwise pull when main
# is behind, then build each artifact whose marker is behind — an artifact whose marker already
# matches origin/main is not rebuilt, and one whose marker is behind is rebuilt even when this
# run did not pull — and for an engine change run `scripts/check.sh --private-only` (output in
# data/ui/private-check.log) and, only when it passes, restart (the pid in data/ui/api.pid,
# checked against the port's listener), wait for /api/health, and print the pid only once the
# port's listener is that pid (HK-30: a health answer alone can come from the old service, as
# it did when the new one died with "Address already in use"). A failed private check or a
# restart that did not take leaves the service on its previous build and marks
# data/ui/private-check.failed or data/ui/restart.failed with the sha until a later build
# passes; every idle run repeats that the service is behind. data/ui/deployed holds
# `engine <sha>` and `web <sha>`, one line each, written once the run has reached origin/main
# and that artifact is current for it, so it reads "the artifact on disk is this sha".
#
# lsof is called by its absolute path, /usr/sbin/lsof: under launchd the PATH is whatever the
# plist sets, and /usr/sbin was not on it, which is how the listener went unseen (HK-30).
set -euo pipefail
# TESSERA_DEPLOY_ROOT deploys another checkout (the self-test's fixture); default: this one.
ROOT="${TESSERA_DEPLOY_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
MODE=deploy
for arg in "$@"; do
  case "$arg" in
    --dry-run) MODE=dry ;;
    --launchd) MODE=launchd ;;
    --self-test) MODE=selftest ;;
    --start-service) MODE=startservice ;;  # internal: the self-test's, see start_service
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# The service, as the loop sees it. TESSERA_DEPLOY_STUB=<dir> replaces every call with files
# in that directory (busy, pid, built, restarted, private), which is how the self-test runs
# without a console or a compiler.
ADDR="${TESSERA_ADDR:-127.0.0.1:8787}"
PORT="${ADDR##*:}"
STUB="${TESSERA_DEPLOY_STUB:-}"
LSOF="${TESSERA_LSOF:-/usr/sbin/lsof}"
[ -x "$LSOF" ] || LSOF="$(command -v lsof || echo /usr/sbin/lsof)"
# Seconds start_service waits for /api/health before printing the pid (the self-test sets 1).
HEALTH_WAIT="${TESSERA_DEPLOY_HEALTH_WAIT:-60}"

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
  "$LSOF" -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null | head -1 || true
}

build_engine() {
  if [ -n "$STUB" ]; then echo engine >> "$STUB/built"; return 0; fi
  (cd "$ROOT" && cargo build --release --quiet --bin tessera --bin tessera-ui)
}

build_web() {
  if [ -n "$STUB" ]; then echo web >> "$STUB/built"; return 0; fi
  (cd "$ROOT/web" && npm run --silent build >/dev/null)
}

# The private checks against the engine just built. Public CI cannot run them (no private
# checkout there), so this is where a merge that breaks the private crate or a private
# strategy test is caught: before the service restarts on it.
private_checks() {
  if [ -n "$STUB" ]; then
    echo private >> "$STUB/checked"
    [ "$(cat "$STUB/private" 2>/dev/null || echo 0)" = 0 ]
    return $?
  fi
  (cd "$ROOT" && scripts/check.sh --private-only > data/ui/private-check.log 2>&1)
}

# Starts a new service and prints its pid. In stub mode the new pid is the old one plus one;
# with $STUB/sticky set the stub's listener stays the old pid, which is the case where the
# old service was never stopped and the new one could not bind.
#
# The console is launched detached from this shell's file descriptors (HK-47). restart_service
# calls this inside $(...), and the earlier form, `(cd "$ROOT" && nohup tessera-ui > log 2>&1 &
# echo $! > pid)`, backgrounded the whole and-list: bash forked a subshell for it that stayed
# alive as the console's parent with the command substitution's pipe still on its stdout (the
# redirections applied to nohup alone), so $(...) never returned, and $! named that subshell,
# not the console. Now the subshell gets the log, /dev/null, and nothing else, and execs the
# console, so $! is the console's pid (setsid, when the platform has it, gives it its own
# session; macOS has none, which is what AbandonProcessGroup in the LaunchAgent is for), and
# nothing of this shell outlives the call. The pid file is written here, by the caller's side.
start_service() {
  local old="$1" new detach=""
  if [ -n "$STUB" ]; then
    new="$((${old:-1000} + 1))"
    echo "$new" >> "$STUB/started"
    [ "$(cat "$STUB/sticky" 2>/dev/null || echo 0)" = 1 ] || echo "$new" > "$STUB/pid"
    echo "$new"
    return 0
  fi
  command -v setsid >/dev/null 2>&1 && detach=setsid
  (cd "$ROOT" && TESSERA_ROOT="$ROOT" exec $detach nohup ./target/release/tessera-ui) \
    > "$ROOT/data/ui/api.log" 2>&1 < /dev/null &
  new=$!
  echo "$new" > "$ROOT/data/ui/api.pid"
  for _ in $(seq 1 "$HEALTH_WAIT"); do curl -sf "http://$ADDR/api/health" >/dev/null 2>&1 && break; sleep 1; done
  echo "$new"
}

restart_service() {  # prints the new pid once the port's listener is that pid
  local old new now recorded
  old="$(listener_pid)"
  if [ -z "$STUB" ]; then
    recorded="$(cat "$ROOT/data/ui/api.pid" 2>/dev/null || true)"
    if [ -n "$recorded" ] && [ "$recorded" != "$old" ]; then
      log "note: data/ui/api.pid says $recorded but the listener on $PORT is ${old:-nobody}; using the listener"
    fi
    if [ -n "$old" ]; then
      kill "$old" 2>/dev/null || true
      for _ in $(seq 1 30); do [ -z "$(listener_pid)" ] && break; sleep 1; done
      [ -z "$(listener_pid)" ] || { log "restart failed: the old service ($old) did not stop"; return 1; }
    fi
  fi
  new="$(start_service "$old")"
  now="$(listener_pid)"
  if [ "$now" != "$new" ]; then
    if [ -n "$now" ] && [ "$now" = "$old" ]; then
      log "restart failed: the old service ($old) still holds the port; the new one ($new) could not bind (data/ui/api.log)"
    else
      log "restart failed: the listener on $PORT is ${now:-nobody}, not the new service ($new); see data/ui/api.log"
    fi
    return 1
  fi
  [ -n "$STUB" ] && echo "restarted from ${old:-none}" >> "$STUB/restarted"
  echo "$new"
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

# The sha each artifact was last built from, in data/ui/deployed (HK-44), one line each:
#   engine <sha>
#   web <sha>
# An artifact whose marker is behind origin/main is rebuilt whether or not this run did the
# pull; one whose marker already matches is not rebuilt. No marker means the loop has no record,
# so the artifact is rebuilt rather than assumed current.
deployed() {  # $1 engine|web — the sha, and nothing when there is no marker
  local line
  line="$(grep -m1 "^$1 " "$ROOT/data/ui/deployed" 2>/dev/null || true)"
  printf '%s' "${line#* }"
}

mark_deployed() {  # $1 engine|web  $2 sha — the other line is kept as it was
  local e w
  e="$(deployed engine)"; w="$(deployed web)"
  if [ "$1" = engine ]; then e="$2"; else w="$2"; fi
  : > "$ROOT/data/ui/deployed.tmp"
  [ -z "$e" ] || echo "engine $e" >> "$ROOT/data/ui/deployed.tmp"
  [ -z "$w" ] || echo "web $w" >> "$ROOT/data/ui/deployed.tmp"
  mv -f "$ROOT/data/ui/deployed.tmp" "$ROOT/data/ui/deployed"
  return 0
}

# Whether $1..$2 is a range git can diff: both shas are commits and $1 is an ancestor of $2. A
# marker that fails this — gone, or not an ancestor after a rewrite — means rebuild, not assume.
usable_range() {  # $1 old $2 new
  [ -n "$1" ] || return 1
  git -C "$ROOT" cat-file -e "$1^{commit}" 2>/dev/null || return 1
  git -C "$ROOT" merge-base --is-ancestor "$1" "$2" 2>/dev/null
}

# Whether an artifact has to be rebuilt to reach $3, judged by the sha it was last built from.
needs_build() {  # $1 engine|web  $2 marker  $3 new
  local kind="$1" marker="$2" new="$3" e w other
  [ -n "$marker" ] && [ "$marker" = "$new" ] && return 1
  usable_range "$marker" "$new" || return 0
  read -r e w other <<< "$(touched "$marker" "$new")"
  if [ "$kind" = engine ]; then [ "$e" = 1 ]; else [ "$w" = 1 ]; fi
}

# How many commits from the furthest-behind of the given shas to $1, for the refusal line.
commits_behind() {  # $1 new  $2.. shas
  local new="$1" n=0 m count
  shift
  for m in "$@"; do
    usable_range "$m" "$new" || continue
    count="$(git -C "$ROOT" rev-list --count "$m..$new")"
    if [ "$count" -gt "$n" ]; then n="$count"; fi
  done
  printf '%s' "$n"
}

# A sha for a log line: seven characters, or "none" when there is none.
short() {  # $1 sha
  [ -n "$1" ] && git -C "$ROOT" rev-parse --short "$1" 2>/dev/null || echo none
}

# Which marker the dry run would build on: named, so the plan says which one is behind.
marker_note() {  # $1 engine|web  $2 marker
  if [ -n "$2" ]; then echo "the $1 marker $(short "$2") is behind"; else echo "no $1 marker yet"; fi
}

deploy() {  # $1: dry (1) or real (0)
  local dry="$1"
  cd "$ROOT"
  mkdir -p data/ui
  local branch; branch="$(git rev-parse --abbrev-ref HEAD)"
  if [ "$branch" != "main" ]; then log "refusing: checkout is on $branch, not main"; return 3; fi
  if [ -n "$(git status --porcelain --untracked-files=no)" ]; then log "refusing: main has local changes"; return 3; fi
  git fetch -q origin main
  local old new de dw engine=0 web=0
  old="$(git rev-parse HEAD)"; new="$(git rev-parse origin/main)"
  de="$(deployed engine)"; dw="$(deployed web)"
  # HK-44: the marker decides what to rebuild, not the pull delta. A session that pulls main by
  # hand advances HEAD, so old = new says nothing about whether the artifacts on disk were built
  # from it; the sha each was last built from does.
  if needs_build engine "$de" "$new"; then engine=1; fi
  if needs_build web "$dw" "$new"; then web=1; fi
  if [ "$old" = "$new" ] && [ "$engine" = 0 ] && [ "$web" = 0 ]; then
    # Nothing to pull and nothing to build: the artifacts on disk are current for this sha, so
    # record that, and a marker a hand pull left behind stops being reported as behind.
    [ "$de" = "$new" ] || mark_deployed engine "$new"
    [ "$dw" = "$new" ] || mark_deployed web "$new"
    if [ "$(cat data/ui/private-check.failed 2>/dev/null || true)" = "$old" ]; then
      log "nothing new: main is at $(git rev-parse --short "$old"); private checks failed on it, the service is still on the build before (data/ui/private-check.log)"
    elif [ "$(cat data/ui/restart.failed 2>/dev/null || true)" = "$old" ]; then
      log "nothing new: main is at $(git rev-parse --short "$old"); the restart on it did not take, the service is still on the build before (data/ui/deploy.log, data/ui/api.log)"
    else
      log "nothing new: main is at $(git rev-parse --short "$old")"
    fi
    return 0
  fi
  if ! git merge-base --is-ancestor "$old" "$new"; then log "refusing: origin/main ($(git rev-parse --short "$new")) is not a fast-forward of main ($(git rev-parse --short "$old"))"; return 3; fi
  local busy; busy="$(running_work)"
  if [ -n "$busy" ]; then log "refusing: $busy running; $(commits_behind "$new" "$old" "$de" "$dw") new commit(s) wait for the next run"; return 3; fi
  local plan
  if [ "$old" = "$new" ]; then plan="already at $(git rev-parse --short "$new")"; else plan="pull $(git rev-parse --short "$old")..$(git rev-parse --short "$new")"; fi
  [ "$engine" = 1 ] && plan="$plan, build engine ($(marker_note engine "$de")), private checks, restart"
  [ "$web" = 1 ] && plan="$plan, build web ($(marker_note web "$dw"))"
  [ "$engine" = 0 ] && [ "$web" = 0 ] && plan="$plan, nothing to build (docs or scripts only)"
  if [ "$dry" = 1 ]; then log "dry run: would $plan"; return 0; fi
  if [ "$old" != "$new" ]; then
    git pull -q --ff-only origin main
    log "pulled $(git rev-parse --short "$old")..$(git rev-parse --short "$new"): $(git log -1 --format=%s)"
  fi
  # Each marker is written once its artifact is current for the sha just reached, so the next
  # run rebuilds only what changes after it.
  if [ "$engine" = 1 ]; then
    build_engine; log "built the engine"; mark_deployed engine "$new"
  elif [ "$de" != "$new" ]; then
    mark_deployed engine "$new"
  fi
  if [ "$web" = 1 ]; then
    build_web; log "built the web bundle"; mark_deployed web "$new"
  elif [ "$dw" != "$new" ]; then
    mark_deployed web "$new"
  fi
  if [ "$engine" = 1 ]; then
    if ! private_checks; then
      git rev-parse HEAD > data/ui/private-check.failed
      log "private checks failed on $(git rev-parse --short "$new"): the service keeps its previous build; see data/ui/private-check.log"
      return 4
    fi
    rm -f data/ui/private-check.failed
    log "private checks passed"
    local pid
    if ! pid="$(restart_service)"; then
      git rev-parse HEAD > data/ui/restart.failed
      log "the service keeps its previous build; restart by hand (AGENTS.md, build and run) and check data/ui/api.log"
      return 4
    fi
    rm -f data/ui/restart.failed
    log "restarted the service: pid $pid (the listener on $PORT)"
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
  <key>AbandonProcessGroup</key><true/>
  <key>StandardOutPath</key><string>$ROOT/data/ui/deploy-launchd.log</string>
  <key>StandardErrorPath</key><string>$ROOT/data/ui/deploy-launchd.log</string>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$HOME/.cargo/bin</string></dict>
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
    # The loop keys on the sha it last deployed (HK-44): the fixture starts as a checkout whose
    # first deploy already happened, so both markers are the scaffold commit.
    scaffold="$(git -C deploy rev-parse HEAD)"
    printf 'engine %s\nweb %s\n' "$scaffold" "$scaffold" > deploy/data/ui/deployed
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
    grep -q '^private$' stub/checked && grep -q 'private checks passed' "$dir/last.err" || { echo "self-test: engine change did not run the private checks before restarting"; cat "$dir/last.err"; exit 1; }
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
    [ "$rc" = 0 ] && grep -q 'dry run: would pull .*build engine (the engine marker .* is behind), private checks, restart' "$dir/last.err" && [ ! -e stub/built ] || { echo "self-test: dry run: rc $rc"; cat "$dir/last.err"; exit 1; }
    # 8. The private checks fail on an engine change: pulled and built, but no restart, exit 4,
    #    the failure in the log and marked; the next idle run says the service is behind.
    echo 1 > stub/private
    rm -f stub/checked
    rc=0; run || rc=$?
    [ "$rc" = 4 ] && grep -q 'private checks failed on .*: the service keeps its previous build' "$dir/last.err" || { echo "self-test: failing private checks: rc $rc"; cat "$dir/last.err"; exit 1; }
    grep -q '^engine$' stub/built && grep -q '^private$' stub/checked && [ ! -e stub/restarted ] || { echo "self-test: failing private checks must build, check, and not restart"; ls stub; exit 1; }
    [ "$(git -C deploy rev-parse HEAD)" = "$(git -C upstream rev-parse HEAD)" ] || { echo "self-test: failing private checks should still have pulled"; exit 1; }
    [ "$(cat deploy/data/ui/private-check.failed)" = "$(git -C deploy rev-parse HEAD)" ] || { echo "self-test: the failure marker should carry the sha"; exit 1; }
    rc=0; run || rc=$?
    [ "$rc" = 0 ] && grep -q 'nothing new: .*private checks failed on it, the service is still on the build before' "$dir/last.err" || { echo "self-test: the idle run after a failure should say the service is behind: rc $rc"; cat "$dir/last.err"; exit 1; }
    # 9. The private checks pass on the next engine change: restart proceeds, marker cleared.
    echo 0 > stub/private
    rm -f stub/built stub/restarted stub/checked
    (cd upstream && echo "fn main() { println!(\"fixed\"); }" > src/main.rs && git commit -q -am "WB-96: fix" && git push -q origin main)
    rc=0; pid="$(run)" || rc=$?
    [ "$rc" = 0 ] && [ "$pid" = 4245 ] && grep -q 'private checks passed' "$dir/last.err" && grep -q 'restarted from 4244' stub/restarted || { echo "self-test: passing private checks: rc $rc pid '$pid'"; cat "$dir/last.err"; ls stub; exit 1; }
    [ ! -e deploy/data/ui/private-check.failed ] || { echo "self-test: the failure marker should be cleared after a passing build"; exit 1; }
    # 10. The restart does not take: the stub keeps the old listener bound (sticky), so the
    #     run fails loudly (exit 4, the marker, the log line) and the service stays on the
    #     previous build; the next idle run says so; with the port free again the next engine
    #     change restarts and clears the marker.
    echo 1 > stub/sticky
    rm -f stub/built stub/restarted stub/started
    (cd upstream && echo "fn main() { println!(\"sticky\"); }" > src/main.rs && git commit -q -am "WB-95: sticky" && git push -q origin main)
    rc=0; run || rc=$?
    [ "$rc" = 4 ] && grep -q 'restart failed: the old service (4245) still holds the port; the new one (4246) could not bind' "$dir/last.err" || { echo "self-test: sticky restart: rc $rc"; cat "$dir/last.err"; exit 1; }
    [ "$(cat stub/pid)" = 4245 ] && [ ! -e stub/restarted ] && grep -q '^4246$' stub/started || { echo "self-test: sticky restart must leave the old pid bound and record no restart"; ls stub; cat stub/pid; exit 1; }
    [ "$(cat deploy/data/ui/restart.failed)" = "$(git -C deploy rev-parse HEAD)" ] || { echo "self-test: the restart marker should carry the sha"; exit 1; }
    grep -q 'the service keeps its previous build; restart by hand' "$dir/last.err" || { echo "self-test: the failed restart should say what to do:"; cat "$dir/last.err"; exit 1; }
    rc=0; run || rc=$?
    [ "$rc" = 0 ] && grep -q 'nothing new: .*the restart on it did not take, the service is still on the build before' "$dir/last.err" || { echo "self-test: the idle run after a failed restart should say the service is behind: rc $rc"; cat "$dir/last.err"; exit 1; }
    rm -f stub/sticky stub/built stub/restarted stub/started
    (cd upstream && echo "fn main() { println!(\"unstuck\"); }" > src/main.rs && git commit -q -am "WB-94: unstuck" && git push -q origin main)
    rc=0; pid="$(run)" || rc=$?
    [ "$rc" = 0 ] && [ "$pid" = 4246 ] && grep -q 'restarted from 4245' stub/restarted && [ ! -e deploy/data/ui/restart.failed ] || { echo "self-test: restart after the port freed: rc $rc pid '$pid'"; cat "$dir/last.err"; ls stub; exit 1; }
    grep -q 'restarted the service: pid 4246 (the listener on' "$dir/last.err" || { echo "self-test: a real restart names the listener:"; cat "$dir/last.err"; exit 1; }
    # 11. The script calls lsof by its absolute path, and the LaunchAgent PATH has /usr/sbin.
    if grep -nE '(^|[ (;|])lsof -' "$script" | grep -q .; then echo "self-test: a bare lsof call remains:"; grep -nE '(^|[ (;|])lsof -' "$script"; exit 1; fi
    bash "$script" --launchd 2>/dev/null | grep -q '/usr/sbin' || { echo "self-test: the LaunchAgent PATH should include /usr/sbin"; exit 1; }
    # 12. start_service returns from inside $(...) against a fake service that sleeps and serves
    #     nothing: within five seconds, the pid file naming the live process (HK-47: the launcher
    #     subshell used to outlive the call as the console's parent with the command
    #     substitution's pipe on its stdout, so the loop sat until the console died). Under the
    #     PATH bash and, when it exists, /bin/bash, the shell the LaunchAgent runs.
    mkdir -p deploy/target/release
    printf '#!/bin/sh\nexec sleep 300\n' > deploy/target/release/tessera-ui
    chmod +x deploy/target/release/tessera-ui
    for shell in bash /bin/bash; do
      [ "$shell" = bash ] || [ -x "$shell" ] || continue
      rm -f deploy/data/ui/api.pid "$dir/start.out"
      (cd deploy && pid="$(TESSERA_DEPLOY_STUB= TESSERA_ADDR=127.0.0.1:9 TESSERA_DEPLOY_HEALTH_WAIT=1 "$shell" "$script" --start-service 2>"$dir/start.err")"; echo "$pid" > "$dir/start.out") &
      waiter=$!
      for _ in $(seq 1 50); do kill -0 "$waiter" 2>/dev/null || break; sleep 0.1; done
      if kill -0 "$waiter" 2>/dev/null; then
        kill "$waiter" 2>/dev/null || true
        stale="$(cat deploy/data/ui/api.pid 2>/dev/null || true)"
        [ -z "$stale" ] || { pkill -P "$stale" 2>/dev/null || true; kill "$stale" 2>/dev/null || true; }
        echo "self-test: start_service under $shell did not return within five seconds inside \$(...)"; cat "$dir/start.err"; exit 1
      fi
      wait "$waiter" || true
      pid="$(cat "$dir/start.out" 2>/dev/null || true)"
      [ -n "$pid" ] && [ "$pid" = "$(cat deploy/data/ui/api.pid 2>/dev/null)" ] || { echo "self-test: start_service under $shell printed '$pid', the pid file says '$(cat deploy/data/ui/api.pid 2>/dev/null)'"; cat "$dir/start.err"; exit 1; }
      ps -o command= -p "$pid" 2>/dev/null | grep -q 'sleep 300' || { echo "self-test: pid $pid from start_service under $shell is not the fake service"; exit 1; }
      kill "$pid"
    done
    # 13. The LaunchAgent abandons the process group, so a stopped loop never stops the console.
    bash "$script" --launchd 2>/dev/null | grep -q '<key>AbandonProcessGroup</key><true/>' || { echo "self-test: the LaunchAgent must set AbandonProcessGroup"; exit 1; }
    # 7. Not on main, or dirty: refuse.
    (cd deploy && git checkout -q -b elsewhere) ; rc=0; run || rc=$?
    [ "$rc" = 3 ] && grep -q 'not main' "$dir/last.err" || { echo "self-test: off-main run: rc $rc"; cat "$dir/last.err"; exit 1; }
    (cd deploy && git checkout -q main && echo "local" >> src/main.rs); rc=0; run || rc=$?
    [ "$rc" = 3 ] && grep -q 'local changes' "$dir/last.err" || { echo "self-test: dirty run: rc $rc"; cat "$dir/last.err"; exit 1; }
    # 14. The marker matches origin/main: nothing is rebuilt, even though a hand pull has just
    #     moved the checkout onto it. The marker, not the pull delta, is what decides (HK-44).
    (cd deploy && git checkout -q -- src/main.rs)
    (cd upstream && echo "fn main() { println!(\"marker\"); }" > src/main.rs && git commit -q -am "WB-93: marker" && git push -q origin main)
    (cd deploy && git pull -q --ff-only origin main)
    head="$(git -C deploy rev-parse HEAD)"
    [ "$head" = "$(git -C upstream rev-parse HEAD)" ] || { echo "self-test: the hand pull should have left main at origin/main"; exit 1; }
    printf 'engine %s\nweb %s\n' "$head" "$head" > deploy/data/ui/deployed
    rm -f stub/built stub/restarted stub/checked
    rc=0; run || rc=$?
    [ "$rc" = 0 ] && grep -q 'nothing new' "$dir/last.err" || { echo "self-test: a marker at main: rc $rc"; cat "$dir/last.err"; exit 1; }
    [ ! -e stub/built ] && [ ! -e stub/restarted ] || { echo "self-test: a marker at main must rebuild nothing"; ls stub; cat "$dir/last.err"; exit 1; }
    # 15. A hand pull advanced main past the marker (HK-44): the run rebuilds the engine and
    #     restarts the service even though it did not do the pull itself, because the marker says
    #     the artifact on disk is a build behind.
    (cd upstream && echo "fn main() { println!(\"hand\"); }" > src/main.rs && git commit -q -am "WB-92: hand pull" && git push -q origin main)
    (cd deploy && git pull -q --ff-only origin main)   # the hand pull the Project rules forbid
    head="$(git -C deploy rev-parse HEAD)"
    [ "$head" = "$(git -C upstream rev-parse HEAD)" ] || { echo "self-test: the hand pull should have left main at origin/main"; exit 1; }
    [ "$(sed -n 's/^engine //p' deploy/data/ui/deployed)" != "$head" ] || { echo "self-test: the fixture's marker should be behind after the hand pull"; exit 1; }
    rm -f stub/built stub/restarted stub/checked
    rc=0; run || rc=$?
    [ "$rc" = 0 ] || { echo "self-test: the hand-pull run: rc $rc"; cat "$dir/last.err"; exit 1; }
    grep -q '^engine$' stub/built && grep -q '^private$' stub/checked || { echo "self-test: a hand pull must rebuild the engine and run the private checks"; ls stub; cat "$dir/last.err"; exit 1; }
    grep -q 'restarted from ' stub/restarted || { echo "self-test: a hand pull must restart the service"; ls stub; cat "$dir/last.err"; exit 1; }
    grep -q 'pulled ' "$dir/last.err" && { echo "self-test: the hand-pull run should not pull again"; cat "$dir/last.err"; exit 1; }
    [ "$(sed -n 's/^engine //p' deploy/data/ui/deployed)" = "$head" ] || { echo "self-test: the engine marker should record the sha just built"; cat deploy/data/ui/deployed; exit 1; }
    [ "$(sed -n 's/^web //p' deploy/data/ui/deployed)" = "$head" ] || { echo "self-test: the web marker should be brought up to the sha too"; cat deploy/data/ui/deployed; exit 1; }
  )
  echo "deploy-local self-test passed"
}

case "$MODE" in
  deploy) deploy 0 ;;
  dry) deploy 1 ;;
  launchd) launchd_plist ;;
  selftest) self_test ;;
  startservice)  # internal: start_service alone, for the self-test; refuses a port in use
    [ -z "$(listener_pid)" ] || { echo "refusing: $(listener_pid) already listens on $PORT" >&2; exit 3; }
    start_service "" ;;
esac
