# Standing instructions for agents

This file is the contract for any agent working in this checkout, whatever harness or model
runs it. It replaces instructions that would otherwise have to be repeated in chat. The first
part is the ticket loop, which is the same in every project that uses it; the **Project rules**
below it are this project's own and are what the loop prompts mean when they say "Project rules".

## The loop

- **Settings.** `.loop.toml` holds everything the loop knows about this project:
  `default_branch`, `backlog` (the ticket file), `check` (the full check), `check_fast` (the
  check to run while iterating), `review_paths` (changes that need a human review),
  `trailer_required`, and `kit` (where the loop kit lives). `scripts/loop-config.sh --all`
  prints the effective values. Prompts and scripts read them from there; they never hard-code
  a branch, a path, or a build command.
- **Tickets.** The backlog is a list of tickets, each a paragraph of intent plus a **Done when**
  line naming the test, fixture, or measurable output that proves it. Git is the record of
  done: a ticket is done when a commit whose subject starts with its id is on the default
  branch. `scripts/backlog-status.sh` derives every ticket's state from the commits and
  `--next` names the first `todo` whose `Blocked by` tickets have landed. The backlog file
  carries only claims: `doing` while someone works a ticket, `blocked <reason>` when it needs
  a decision. Clear the `doing` claim in the ticket's own commit and never write a done line.
  Anything discovered while working goes in as a new ticket, not into the current one.
- **Claims and hand-off.** Before work starts, `scripts/open-ticket-pr.sh <id> --claim` pushes
  `ticket/<id>` to origin; `backlog-status.sh --next` passes over claimed ids, so several
  agents can hold several tickets. After the commit, `scripts/open-ticket-pr.sh <id>` pushes
  the branch and opens the pull request; a green PR up to date with the default branch merges
  on its own, one that touches a review path is labelled `needs-review` and waits for the
  owner. Never push the default branch. Never force-push. Never rewrite its history.
- **Commits.** One commit per ticket. The subject starts with the ticket id (`AB-12: ...`); the
  body says what changed and how the done line is proven; the message ends with a
  `Co-Authored-By: <agent> <email>` trailer naming the agent and model that did the work when
  `trailer_required` is on (the PR script refuses a commit without one).
- **Definition of done.** The full check (`check` in `.loop.toml`) passes, and the commit
  includes the test or fixture that proves the ticket's done line. Run the fast check while
  iterating and the full check before committing. CI runs the same script; there are no
  separate hand-written CI steps to keep in sync.
- **Isolation.** Prefer an isolated worktree per ticket. The check script knows how to run
  from one (see the project rules for what it resolves).
- **Releases.** Tag them: `scripts/release-notes.sh <from> <to>` lists what shipped, and
  `--archive <tag>` moves the shipped tickets out of the backlog into `CHANGELOG.md`.
- **Prompts.** `loop/prompts/next-ticket.md` takes the next ticket to done;
  `loop/prompts/grill-me.md` turns a loose idea into stories, acceptance criteria, and
  tickets. A harness with slash commands wraps them (`.claude/commands/`); any other agent is
  pointed at the prompt file directly.
- **The kit.** The loop scripts and prompts are copies from the loop kit named by `kit` in
  `.loop.toml`; `scripts/loop-kit-sync.sh --check` fails when they drift, and
  `scripts/loop-kit-sync.sh` brings them up to the kit's tag. Change them in the kit, not in
  `scripts/`. Until the kit is published it is the `loop/` directory of this checkout, so a
  change goes into `loop/scripts/` and the sync copies it out.

## Project rules

Tessera is an event-driven backtesting engine (Rust) with a one-file strategy SDK and a local
Bloomberg-style research console (Vite + React bundle served by the Rust service).

### Layout

- This checkout is the public, AGPL repo (`FueledByChai/tessera`). Engine in `src/`, SDK in
  `src/sdk/`, service in `src/bin/tessera_ui.rs`, web app in `web/` (`web/app/page.tsx`,
  `web/app/globals.css`), docs in `docs/`, synthetic examples in `examples/`. Product intent
  and long-form acceptance criteria live in `docs/PRODUCT_BACKLOG.md`; the executable queue
  is `BACKLOG.md`.
- `../Tessera-private` is the private repo: strategies (`strategies/*.rs`, compiled in through
  `local.toml [strategies] dirs`), frozen configs, research scripts, the legacy crate, and the
  research log. Never copy private strategies, configs, or data paths into this repo.
- Market data lives outside both repos (see `local.toml`). Never modify, delete, or commit data.
- On the Mac mini the checkout lives at `~/Code/Tessera` with the private repo beside it at
  `~/Code/Tessera-private`, outside iCloud Drive and outside the folders macOS guards
  (Documents, Desktop): iCloud evicted dependencies and artifacts into placeholders that
  stalled Node tools for minutes, and launchd could not read a script under Documents at all.
  Keep it there; never move it back into a synced or guarded folder.

### Build, run, restart

- Engine: `cargo build --release --bin tessera --bin tessera-ui` (private strategies compile in).
- Web: `cd web && npm run build` (~3 s). The service serves `web/dist` from disk; no restart.
- Service: http://127.0.0.1:8787. Restart after an engine change: kill the pid in
  `data/ui/api.pid` (verify with `lsof -nP -iTCP:8787 -sTCP:LISTEN`), then
  `TESSERA_ROOT=$PWD nohup ./target/release/tessera-ui > data/ui/api.log 2>&1 &` and write the new
  pid to `data/ui/api.pid`. Do not restart while a job is `running` in the catalog.
  `scripts/deploy-local.sh` does all of this from `origin/main` on a schedule (pull, build what
  changed, restart only for engine changes and only when idle; `--dry-run` shows the plan,
  `--launchd` prints the LaunchAgent that runs it every five minutes).
- CLI runs: `./target/release/tessera run-strategy --config <toml> --start <date> --end <date>
  --output-dir <dir>`. Scratch outputs go under `target/` or the session scratchpad, never `artifacts/`.

### The check

`scripts/check.sh` is the full check: the loop self-tests, fmt, tests, build, parity of the
bundled examples against `examples/expected`, web typecheck/lint/build with the theme, layout,
and chart checks, and the private checks when that checkout exists. `--no-web` is the fast
check. If an engine change intentionally alters results, refresh the baseline with
`scripts/check.sh --refresh-baseline` and say why in the commit; `examples/expected/` is the
review path, so that PR waits for the owner. CI runs `--quick` in the engine job and
`--web-only` in the web job. From a worktree the script finds the main checkout through the
shared git dir, the private checkout beside it (`TESSERA_PRIVATE_ROOT` overrides), writes
`local.toml` from the main one with its relative paths made absolute, links
`web/node_modules`, and builds the private legacy crate against the worktree's engine;
`scripts/check.sh --resolve` shows what a run would use.

### Engine and data rules

- Strategies go through the SDK (`docs/ADDING_A_STRATEGY.md`), never new match arms in the service.
- Quantities are `f64`; prices reaching strategies are split-adjusted; `bar.raw_close()` is the
  unadjusted print for floors and dollar volume.
- Daily data is sanitized at load (`[data] sanitize_prices`); keep it on. Any trade exiting above
  4x its entry, or any equity swing that reverses in a day, is a data error until proven otherwise.
- Universe-sized runs (tens of thousands of symbols) must stay linear in memory: no per-row
  strings in tables that scale with symbols x sessions; coverage uses the interned column table.
- Broker guards (buying power, solvency, commission cap, tick floor, min price) are not optional.
- Do not touch market data or `artifacts/`; do not restart the service while a job is running.

### UI conventions

- Terminal look: black panels, amber accent, dense monospace tables, 18/15 px table fonts.
  Navy only as a hover or selection highlight. Form controls in both display modes are 42 px
  boxes with 18 px text and 15 px labels, bottom-anchored so a row's controls share edges; the
  sizing block stays last in `globals.css`. `web/scripts/theme-check.mjs` (run by
  `scripts/check.sh`) fails on blue backgrounds or borders, on smaller control fonts, and on
  bare `fr` grid tracks. Pages must fit a 13-inch laptop: wide tables go dense under 1500 px
  rather than scrolling; `web/scripts/layout-check.mjs` measures 1280 and 1440 px, on CI against
  a scratch console seeded by `scripts/scratch-console.sh` (`TESSERA_ADDR` picks its port).
- Long tables go behind tabs or caps with a "show all"; a page must open at the top.
- Charts are inline SVG in the terminal style (`EquityChart`, the study charts). The studies
  page must keep rendering every chart from `web/fixtures/study-result.json`:
  `web/scripts/chart-check.mjs` (run by `scripts/check.sh`) fails on a missing chart, a console
  error, or a chart placed above the results grid.
- Relative API origin (`import.meta.env.VITE_API_ORIGIN ?? ""`); dev server on 5173 proxies `/api`.

### Docs to keep current

`docs/ADDING_A_STRATEGY.md` (SDK contract and guards), `docs/DATA_SOURCES.md` (sources and
sanitation), `docs/LOCAL_UI.md` (service and web), `docs/LOOP.md` (the loop and its tooling),
`README.md` screenshot when the run view changes.

### Other commands

`.claude/commands/nightly-studies.md` runs the registered feature studies against the private
research configs and appends to the research log; it is this project's, not the loop's.
