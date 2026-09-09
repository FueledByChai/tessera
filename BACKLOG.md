# Work queue

The ticket list an agent loop works through. `docs/PRODUCT_BACKLOG.md` holds product intent and
long-form acceptance criteria; this file is the executable queue. Protocol:

- One ticket per commit. The commit message starts with the id. `scripts/check.sh` must pass.
- A ticket's **Done when** line names a test, fixture, or measurable output that ships in the
  same commit. If it cannot be tested, rewrite the ticket until it can.
- Git is the record of done: a ticket is done when a commit whose subject starts with its id
  is on `main`. `scripts/backlog-status.sh` lists every ticket with its derived state, date,
  and sha; `--next` names the first `todo` whose `Blocked by` tickets have landed. This file
  carries only the claims: no state (or `todo`), `doing` while someone works it,
  `blocked <reason>`. Take what `--next` reports, never two at once, and clear the `doing`
  claim in the ticket's own commit; never write a done line.
- Anything discovered while working goes in as a new ticket, not into the current one.

## Console UI

### UI-01 Terminal theme: black panels, larger aligned form controls
The terminal theme (`web/app/globals.css`, the block "sampled from Bloomberg screens") paints
every panel navy: `--panel #0f0f3a`, `--panel-2 #181850`, lines `#2a2a6a`/`#1c1c50`, plus
literal navy on fieldsets, code, and the config template. Studies, Data, and the strategy page
read as a blue console. A Bloomberg screen's ground is black; navy is a highlight for selected
rows and a few data panels, not the panel colour. Move the theme to black panels (`#050505`,
`#0b0b0b`) with the existing gray and amber lines, keep navy only for selection and hover
highlights, and fix the form controls: inputs, selects, and textareas in one form row share a
height and top edge, dropdowns match text fields, and the control font goes from the inherited
~12 px to 18 px with labels 10 → 15 px (the table convention), without growing the controls.
Applies to the studies form, the run form, the strategy page, and the data page.
**Done when:** a stylesheet check (`web/scripts/theme-check.mjs`, run from the web step of
`scripts/check.sh`) parses the terminal-theme rules and fails on any background or border colour
with a blue hue outside the `.active`, `:hover`, and heat-map highlight selectors, and on any
terminal-theme input/select/textarea font-size below 18 px; the browser-automation check on the
studies form and the strategy run form reports every control in a row within 1 px of the same
top and bottom edge (`getBoundingClientRect`); the README screenshot is refreshed.

### UI-02 Modern mode form controls match the terminal sizes
UI-01 sized and aligned form controls only under the terminal theme; modern mode still renders
the base 10-14 px controls with the 40/42 px height mismatch between text and date fields.
**Done when:** the browser alignment measurement from UI-01 passes with modern mode selected and
`theme-check.mjs` gains a modern-mode font-size rule.

### UI-03 Pages fit a 13-inch laptop; wide tables scroll inside their panel
On a 13-inch MacBook (1440 or 1280 px wide) the run page's monthly and annual performance
table (`.monthly-panel`), the studies page's feature-by-horizon heat map and its ranked and
decile tables, and other dense grids hang off the right edge of the window instead of
scrolling. `.table-wrap` already has `overflow-x: auto`, so the likely cause is grid and flex
children whose minimum width defaults to their content (`minmax(0, 1fr)` and `min-width: 0` are
missing on some panels), plus fixed column counts and the 18/15 px grid fonts at narrow widths.
Fix so the page body never scrolls horizontally: every panel is `min-width: 0`, wide tables
scroll within their own wrapper, multi-column layouts collapse below 1280 px, and the heat map
keeps its cells readable by dropping the second line (breakeven) into the hover title under
1280 px rather than shrinking the font below 15 px.
**Done when:** a headless measurement (`web/scripts/layout-check.mjs`, run by the web step of
`scripts/check.sh` against the built bundle with fixture data, or against the running service
when present) at 1280 and 1440 px on the run overview, the strategy page, and the studies page
asserts `document.documentElement.scrollWidth <= window.innerWidth` and that no element's right
edge exceeds the viewport unless an ancestor has `overflow-x: auto`; `theme-check.mjs` fails on
any grid track wider than `minmax(0, 1fr)` without `min-width: 0` on the track's children.

## Feature workbench (Studies page)

### WB-01 Feature expression grammar
Replace the fixed feature list in `src/study.rs` with expressions: a base series followed by
transforms, e.g. `trade_count | rate 1 | ratio_to sma 300`, `signed_volume | zscore 30`,
`obi_l1 | diff 1`, `a | times b`. Bases: every `BookFeatures` field plus trade count, volume,
buy/sell volume, per-side depth. Transforms: `ema n`, `sma n`, `zscore n`, `diff n`, `lag n`,
`rate n`, `ratio_to`, `pct_rank n`, `abs`, `sign`, `clip lo hi`, `times <expr>`. Streaming state
per symbol; unknown names produce a clear error listing what exists.
**Done when:** unit tests cover parsing (including nested `times` and error text), each transform
against hand-computed values, and a study over the existing eight names produces identical IC to
the current implementation (fixture from a 1-day SOL grid checked into `target/`-free test data).
Note: the parity fixture is a deterministic synthetic 1-second grid generated in the test rather
than a checked-in SOL day; it exercises book gaps and all eight names.

### WB-02 Costless equity curve and breakeven cost — Blocked by WB-01
For each feature and horizon: position = clipped z-score of the feature (and a `sign` variant),
P&L = position x forward return with zero costs; report cumulative P&L series, Sharpe, turnover
(mean absolute position change per bar), and breakeven cost in bps = mean P&L per unit turnover.
Add the series to `StudyResult` and CSV output; UI shows the curve per selected cell.
**Done when:** a synthetic feature equal to the forward return plus noise yields the expected
Sharpe within tolerance in a unit test, a pure-noise feature yields breakeven near zero, and the
results grid in the UI sorts by breakeven cost.

### WB-03 Study targets — Blocked by WB-01
Add a `target` field: `return` (current), `realized_variance` (sum of squared mid returns over the
horizon), `abs_move`, `spread_change`, `fair_value_residual` (mid minus a 60 s EMA, and mid minus
microprice). IC, deciles, and the equity curve all run against the chosen target.
**Done when:** unit tests compute each target on a fixture, and a study of `spread_bps` against
`realized_variance` on the SOL fixture reports a positive IC.

### WB-04 Panel loader over every bar resolution — Blocked by WB-01
The study reads its panel through the SDK loader: 1-minute, 5-minute, and daily CSV bars as well
as tick-built lake bars. Book-only bases report "unavailable on this grid" instead of failing.
OHLCV bases: `return_n`, `range_bps`, `gap_bps`, `volume | zscore n`, `high_252_distance`.
**Done when:** a daily study of `return_1 | zscore 20` on `examples/data` runs end to end and a
test asserts the same IC from the CSV path and from an equivalent in-memory panel.

### WB-05 Exogenous series registry with availability times — Blocked by WB-04
`local.toml [[data.series]]`: name, path (CSV or parquet), kind `level` or `event`, optional
symbol column, and an `available_at` column or a fixed publication lag. Studies join series as-of
the bar's time using availability, never the nominal date. Funding and open interest from the
lake register automatically.
**Done when:** a test with a series whose `available_at` is after its nominal date proves the
bar before availability does not see the value and the bar after does; funding rate is usable as
a base in a SOL study.

### WB-06 Cross-sectional mode — Blocked by WB-04
For daily panels across many symbols: rank the feature across symbols per date, IC per date,
mean and t across dates, and a costless long-short decile portfolio with its equity curve.
**Done when:** a synthetic panel where the feature is the next-day return gives IC near 1 and the
long-short curve is monotone; the UI offers `time-series` and `cross-sectional` modes.

### WB-07 Aggregation transforms and event studies — Blocked by WB-05
`agg daily sum|mean|last|realized_var` lifts intraday series to the daily grid; slow series ride
fast grids by forward fill. For `event` series: average forward and backward return path around
events with counts and t per offset.
**Done when:** a test builds a daily realized variance from 1-minute fixture bars and matches a
direct computation; an event-study fixture with a known post-event drift reproduces it.

### WB-08 Diagnostics: incremental IC, stability, regime buckets — Blocked by WB-02
Incremental IC against an "accepted" feature set (regress out, score the residual); IC per day with
sign-consistency count; IC by spread tercile, realized-vol tercile, and hour of day; feature
autocorrelation.
**Done when:** a feature that is a linear copy of an accepted feature reports incremental IC near
zero in a test; the UI shows the per-day IC strip and the regime table.

### WB-09 Presets and promotion — Blocked by WB-02
Named feature expressions saved in SQLite, a "promote to accepted" action, and a parquet export of
accepted features plus targets for model fitting.
**Done when:** a preset survives a service restart, promotion changes what WB-08 orthogonalizes
against, and the export round-trips through `tessera parquet-schema`.

### WB-10 Study charts — Blocked by WB-02
IC decay across horizons, decile bars, the costless equity curve, and daily IC, as inline SVG in
the terminal style (see `EquityChart`).
**Done when:** each chart renders from a fixture result in the browser without console errors and
the study page opens at the top with the results grid first.

### WB-11 Non-overlapping costless curve for slow features — Blocked by WB-02
The WB-02 curve pays every bar the forward return of an `h`-bar hold, so a feature whose position
barely changes (spread_bps: turnover 0.002/bar on the SOL day) reports a breakeven of tens of bps
that no non-overlapping execution would earn. Add a variant that rebalances every `h` bars (or
holds until the position flips) and report its Sharpe and breakeven next to the per-bar ones.
**Done when:** a unit test shows the per-bar and rebalanced variants agree for `h = 1` and the
rebalanced breakeven of a constant-position feature is finite and far below the per-bar figure.

### WB-12 Studies form knows the grid's features and symbols — Blocked by WB-04
On a CSV grid the form still offers the order-book feature checkboxes (they come back as
"unavailable on this grid") and takes symbols as typed text. Hide or grey the book features when
a CSV grid is chosen, pick CSV symbols from the catalog with the run form's instrument picker,
and pre-tick the OHLCV set (`return_1`, `range_bps`, `gap_bps`, `high_252_distance`).
**Done when:** the browser check selects the daily grid and finds no order-book checkbox and a
catalog-backed symbol picker; a submitted daily study reports nothing unavailable.

### WB-13 Studies form lists the registered series — Blocked by WB-05
Series from `[[data.series]]` and the lake side feeds (funding_rate, funding_annualized,
open_interest, open_interest_usd) are usable as bases but only appear in the expression hint;
the form should list them as feature checkboxes with their kind and availability rule, from a
`GET /api/studies/series` endpoint, and grey out lake feeds on CSV grids.
**Done when:** the browser check on the studies form finds a checkbox per registered series and
a lake study submitted from it with `funding_rate` ticked produces that cell.

## Housekeeping

### HK-01 Required symbols from the manifest
A manifest can declare symbols the runner always appends (End-of-Year Dogs needs `IWM.US` for the
hedge even when the form lists only `universe:stocks`).
**Done when:** a test shows the appended symbol present in the plan when omitted from the form,
and the Dogs manifest declares it.

### HK-02 Sanitation counts on the run page
Show dropped off-calendar rows, dropped spikes, and skipped symbols from the run log in the
Overview tab, with the skipped symbols listed on the Symbols tab.
**Done when:** a run over `examples/data` with an injected holiday row shows the count in the UI.

### HK-03 CI runs scripts/check.sh
Replace the hand-written CI steps with `scripts/check.sh --quick` plus the web job.
**Done when:** the workflow file calls the script and a deliberate parity break fails CI locally
with `act` or in a PR.

### HK-04 check.sh works from a worktree
From `.claude/worktrees/<name>` the script cannot find `../Tessera-private` (so the private checks
silently skip), a fresh worktree has no `local.toml` (so private strategies do not compile in), and
the private legacy crate builds against the main checkout's engine, so a private strategy that uses
a new SDK method fails the private check until the public commit reaches `main`. Found while
working HK-01 in a worktree.
**Done when:** `scripts/check.sh` run from a worktree resolves the private checkout through the
repository's common git dir (or `TESSERA_PRIVATE_ROOT`), copies or points at the main `local.toml`,
and a test run from a worktree reports the private checks as run, not skipped.

### HK-05 Ticket state derived from git
Every ticket edits this file to write `done <date> <sha>`, which is the one line two parallel
loops always collide on, and the sha cannot be known before the commit exists. Make git the
record: a ticket is `done` when a commit whose subject starts with its id is on `main`. This
file keeps only `todo`, `doing`, and `blocked` (`doing` stays as the claim; the loop clears it in
the ticket's commit). Add `scripts/backlog-status.sh` that prints every ticket with its derived
state, date, and short sha, and update `CLAUDE.md` and `.claude/commands/next-ticket.md` so the
loop stops writing `done` lines and picks the first `todo` whose blockers have a commit on `main`.
**Done when:** the status script lists HK-01 and WB-02 as done with their dates and shas while
their lines here carry no state, and a test fixture repo in the script's tests shows a `doing`
ticket with a landed commit reported as done.

### HK-06 Release notes from commits and a changelog archive — Blocked by HK-05
`scripts/release-notes.sh <from-ref> [<to-ref>]` lists commits whose subject starts with a
ticket id, grouped by prefix (WB, HK) with date and short sha, as Markdown. `--archive <tag>`
moves the completed tickets' entries out of this file into `CHANGELOG.md` under the tag's
heading, so the queue holds only open work and the changelog is the record of what shipped
when. Tag releases; the notes are the diff between tags.
**Done when:** the script over `3022548..main` lists HK-01 and WB-02 under their sections with
dates; `--archive` on a scratch copy produces a `CHANGELOG.md` containing them and a
`BACKLOG.md` without them; `scripts/check.sh --quick` runs the script's self-test.

### HK-07 Layout check runs in CI — Blocked by HK-03
`web/scripts/layout-check.mjs` (UI-03) skips when no Chromium is resolvable and when no console
answers on 8787, so on CI it currently proves nothing. Add `playwright` as a web devDependency,
install its Chromium in the workflow, and run the check against the built bundle with the
bundled example data (a scratch `TESSERA_ROOT` with `examples/`, one run, one study fixture).
**Done when:** the CI job logs `layout-check: ok` and a deliberate bare-`fr` regression fails it.

### HK-08 Layout check skips pages the console cannot supply
`web/scripts/layout-check.mjs` reports eight "could not open" failures when the console on 8787
has no runs or strategies (a scratch instance on an empty catalog, seen during WB-09), so a
check run beside a scratch service fails for reasons unrelated to the layout. Pages the console
cannot supply should be listed as skipped, like a missing console, while the rest are measured.
**Done when:** the check run against an empty catalog exits 0 with the run and strategy pages
listed as skipped and the studies page measured.

### HK-09 The loop hands off through pull requests
Today a ticket ends in a fast-forward merge and a service restart only the owner can do, so
tickets run one at a time even when nothing blocks them. An agent should push a `ticket/<id>`
branch (never `main`) and open a pull request whose body is the ticket report (id, what changed,
how the done line is proven, what the owner should look at), and the claim on a ticket should
be that remote branch rather than a `doing` line that only exists in a worktree:
`scripts/backlog-status.sh --next` skips ids with a `ticket/<id>` branch on `origin`, and
`scripts/open-ticket-pr.sh <id>` pushes the branch and opens the PR with `gh`. `CLAUDE.md` and
`.claude/commands/next-ticket.md` change from "never push" to "never push `main`". Needs `gh`
installed and authenticated on this machine.
**Done when:** the status script's self-test shows a ticket with a remote `ticket/<id>` branch
skipped by `--next`, and a ticket worked from a worktree ends with an open PR whose body carries
the report and whose checks are the CI jobs.

### HK-10 Merges gated by CI: branch protection and the merge queue — Blocked by HK-09
With several PRs open at once a merge must be tested as the result it produces, not as the
branch on its own. Turn on branch protection for `main` (the engine and web CI jobs as required
status checks, no direct pushes) and the GitHub merge queue, so each PR merges only after CI
passes on `main` plus the PRs ahead of it; the parity step is what catches two clean merges that
change engine results together. Auto-merge on green for PRs that leave `examples/expected`
untouched; a PR that refreshes the parity baseline requires a review. Document the policy and
the settings in `docs/LOOP.md`.
**Done when:** `gh api` shows the protection and queue settings on `main`; a PR that breaks
parity is blocked from merging while one that passes merges through the queue, both recorded in
LOOP.md with the commands used.

### HK-11 Local deploy loop
After a merge nobody rebuilds and restarts the console on the Mac mini. `scripts/deploy-local.sh`
pulls `main` fast-forward, rebuilds the engine and the bundle when the tree changed, and restarts
the service only when no job or study is `running` (the pid in `data/ui/api.pid`, verified
against the listener), writing the new pid; a schedule (launchd, or the app's scheduled tasks)
runs it every few minutes.
**Done when:** with a new commit on `origin/main` the script rebuilds, restarts, and prints the
new pid; with a running job it refuses and says why; with nothing new it exits quickly without
touching the service; a self-test covers those decisions against a stubbed service.

### HK-12 Self-hosted runner for the full check — Blocked by HK-10
The public runner cannot run the private checks or the layout check against real data. A
self-hosted runner on the Mac mini could run the full `scripts/check.sh` (private checks, the
real console) on PRs. A public repository lets fork PRs run code on the runner, so the job must
skip fork PRs, the repository must require approval for outside collaborators, and neither market
data nor the private checkout may reach CI logs. Decide first whether the trade-off is worth it.
**Done when:** a `full-check` job on the self-hosted runner logs `private checks passed` on a PR
from this repository and is skipped on a fork PR; the runner's setup and security settings are
in `docs/LOOP.md`.
Resolved differently: decided against a runner (docs/LOOP.md, "What CI does not run, and why
there is no self-hosted runner"). A public repository's self-hosted runner on the machine that
holds the data and the private checkout is more exposure than the enforcement is worth while
every PR comes from a checkout that runs the full check first; HK-18 covers the post-merge
case in the deploy loop instead. Revisit when PRs come from elsewhere.

### HK-13 web/node_modules lives outside iCloud
The checkout sits in iCloud Drive, which evicts `web/node_modules` under disk pressure: during
HK-09, 2,869 of its 3,697 files were dataless placeholders and `npm run lint` sat in file stats
for ten minutes fetching them one by one, stalling `scripts/check.sh`. Keep the dependencies out
of the synced tree: `scripts/check.sh` (and `npm ci`, through a documented step) should place
them in a non-synced directory such as `~/Library/Caches/tessera/web-node_modules` and leave
`web/node_modules` as a symlink to it, for the main checkout and every worktree.
**Done when:** after `npm ci`, `web/node_modules` is a symlink into a directory outside iCloud,
`ls -lO` finds no dataless file under it, and `scripts/check.sh --resolve` reports where it lives.
Resolved differently: the whole checkout moved to `~/Code/Tessera` (with the private repo beside
it), outside iCloud, after launchd also proved unable to read a script under Documents; nothing
under the checkout is evicted any more, so no symlink is needed.

### HK-14 Loop settings live in `.loop.toml`, not in the scripts
The loop scripts carry Tessera in their bodies: `scripts/open-ticket-pr.sh` hard-codes
`origin/main` and the parity path `examples/expected/` as the one change that forces a human
review, and the prompts name `scripts/check.sh --no-web` and Tessera's data rules. Another
project cannot adopt the loop without editing every file. Add a flat `.loop.toml` at the repo
root (TOML is only the config format; it says nothing about the project's language) with a
single `[loop]` table: `default_branch`, `backlog` (path of the ticket file), `check` (the full
check command), `check_fast` (the command to run while iterating), `review_paths` (globs whose
change turns off auto-merge), and `trailer_required` (whether commits must carry a
`Co-Authored-By` trailer naming the agent that did the work). A small reader,
`scripts/loop-config.sh <key>`, prints one value with the defaults that reproduce today's
behaviour when the file or key is missing, and `backlog-status.sh`, `open-ticket-pr.sh`, and
`release-notes.sh` read every project-specific value through it.
**Done when:** `scripts/loop-config.sh --self-test` covers a missing file, a missing key, and
each key set; `open-ticket-pr.sh` labels a branch `needs-review` when a configured
`review_paths` glob matches and not otherwise (proved in a fixture repo the self-test builds);
`grep -n 'examples/expected\|origin/main' scripts/open-ticket-pr.sh scripts/backlog-status.sh
scripts/release-notes.sh` finds only comments.

### HK-15 Instructions and prompts any coding agent can read — Blocked by HK-14
`CLAUDE.md` and `.claude/commands/*.md` are read by Claude Code alone; Codex, OpenCode, and the
other harnesses read `AGENTS.md` and have no slash commands. Move the standing instructions to
`AGENTS.md` and leave `CLAUDE.md` as one line that points at it. Move the prompt bodies to
`loop/prompts/next-ticket.md` and `loop/prompts/grill-me.md`, written against the `.loop.toml`
contract only (run the fast check while iterating, the full check before committing, sign with
the trailer the config requires) and free of project names, build tools, and harness tool
names (`AskUserQuestion` becomes "ask the owner, four questions at a time, in whatever way
the harness offers"). The `.claude/commands/*.md` files become two-line wrappers that say to
read and follow the prompt file with the arguments given. Project-specific rules (data, UI
conventions, docs to keep current) stay in `AGENTS.md` under a "Project rules" heading the
prompts refer to by name.
**Done when:** `scripts/check.sh` gains a step that fails if `loop/` mentions `tessera`,
`cargo`, `npm`, `Claude`, or `examples/expected` (case-insensitive; `.loop.toml` is exempt,
being this project's own values, as HK-14 found); `CLAUDE.md` is
a single pointer line; both wrappers under `.claude/commands/` are under five lines; and a
`/next-ticket` run from this checkout still claims, works, and opens a PR for a ticket.

### HK-16 The loop kit is its own repository — Blocked by HK-15
Once the scripts and prompts are generic they belong in one place every project pulls from.
Create a public repository (working name `loop-kit`; the owner picks the final name) holding
`scripts/backlog-status.sh`, `scripts/open-ticket-pr.sh`, `scripts/release-notes.sh`,
`scripts/loop-config.sh`, `loop/prompts/*.md`, an `AGENTS.md` template with the loop section
and an empty "Project rules" heading, a `.loop.toml` example, the CI workflow skeleton
(one job per configured check command, job names used as the required status checks), the
branch-ruleset JSON from `docs/LOOP.md`, and an `install.sh` that copies those files into a
target checkout and prints what the project still has to supply: `scripts/check.sh` and,
optionally, a deploy script. GitHub is the only hosting assumption (`gh`, rulesets,
auto-merge); say so in its README. Tessera consumes the kit through `scripts/loop-kit-sync.sh`,
which copies the kit's files in and diffs them, so the kit stays the source of truth.
**Done when:** in a fresh `git init` repo containing only a stub `scripts/check.sh`, `install.sh`
followed by `scripts/backlog-status.sh --self-test`, `scripts/release-notes.sh --self-test`,
and `scripts/loop-config.sh --self-test` all pass; and in this checkout
`scripts/loop-kit-sync.sh --check` reports no difference from the kit's tagged release.

### HK-17 CI installs Playwright's browser without depending on the apt mirror
PR #6's web job failed in 24 s with `Failed to install browsers` after an apt index hash
mismatch inside `npx playwright install --with-deps chromium`; the change was docs only, and a
rerun is the only remedy. Cache the browser under `~/.cache/ms-playwright` keyed on the
Playwright version in `web/package-lock.json`, and retry the install once before failing.
**Done when:** the workflow shows a cache step for the browser and a retry around the install,
and two consecutive CI runs on the same lock file show the second restoring the cache.

### HK-18 Deploy loop runs the private checks before it restarts the service
Public CI cannot run the private checks (HK-12 decided against a self-hosted runner), so a
merge that breaks the private legacy crate or a private strategy test reaches `main` unseen
until someone runs `scripts/check.sh` here. `scripts/deploy-local.sh` should run the private
checks (`scripts/check.sh --no-web`, or the private step alone when `check.sh` grows a flag for
it) after pulling and before restarting the service, refuse the restart when they fail, and
write the failure to `data/ui/deploy.log` so the owner sees it the next morning.
**Done when:** the deploy script's self-test shows a fixture where the private check fails
leaving the service untouched with `private checks failed` in the log, and one where it passes
and the restart proceeds; `docs/LOOP.md` names it as the post-merge guard.
