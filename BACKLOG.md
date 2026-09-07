# Work queue

The ticket list an agent loop works through. `docs/PRODUCT_BACKLOG.md` holds product intent and
long-form acceptance criteria; this file is the executable queue. Protocol:

- One ticket per commit. The commit message starts with the id. `scripts/check.sh` must pass.
- A ticket's **Done when** line names a test, fixture, or measurable output that ships in the
  same commit. If it cannot be tested, rewrite the ticket until it can.
- States: `todo`, `doing`, `done <date>` (the commit carries the id; `git log --grep WB-01`), `blocked <reason>`. Take the first `todo`
  whose `Blocked by` tickets are done. Never take two at once.
- Anything discovered while working goes in as a new ticket, not into the current one.

## Feature workbench (Studies page)

### WB-01 Feature expression grammar — `done 2026-09-06`
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

### WB-02 Costless equity curve and breakeven cost — `done 2026-09-07 WB-02: costless equity curve and breakeven cost` — Blocked by WB-01
For each feature and horizon: position = clipped z-score of the feature (and a `sign` variant),
P&L = position x forward return with zero costs; report cumulative P&L series, Sharpe, turnover
(mean absolute position change per bar), and breakeven cost in bps = mean P&L per unit turnover.
Add the series to `StudyResult` and CSV output; UI shows the curve per selected cell.
**Done when:** a synthetic feature equal to the forward return plus noise yields the expected
Sharpe within tolerance in a unit test, a pure-noise feature yields breakeven near zero, and the
results grid in the UI sorts by breakeven cost.

### WB-03 Study targets — `todo` — Blocked by WB-01
Add a `target` field: `return` (current), `realized_variance` (sum of squared mid returns over the
horizon), `abs_move`, `spread_change`, `fair_value_residual` (mid minus a 60 s EMA, and mid minus
microprice). IC, deciles, and the equity curve all run against the chosen target.
**Done when:** unit tests compute each target on a fixture, and a study of `spread_bps` against
`realized_variance` on the SOL fixture reports a positive IC.

### WB-04 Panel loader over every bar resolution — `todo` — Blocked by WB-01
The study reads its panel through the SDK loader: 1-minute, 5-minute, and daily CSV bars as well
as tick-built lake bars. Book-only bases report "unavailable on this grid" instead of failing.
OHLCV bases: `return_n`, `range_bps`, `gap_bps`, `volume | zscore n`, `high_252_distance`.
**Done when:** a daily study of `return_1 | zscore 20` on `examples/data` runs end to end and a
test asserts the same IC from the CSV path and from an equivalent in-memory panel.

### WB-05 Exogenous series registry with availability times — `todo` — Blocked by WB-04
`local.toml [[data.series]]`: name, path (CSV or parquet), kind `level` or `event`, optional
symbol column, and an `available_at` column or a fixed publication lag. Studies join series as-of
the bar's time using availability, never the nominal date. Funding and open interest from the
lake register automatically.
**Done when:** a test with a series whose `available_at` is after its nominal date proves the
bar before availability does not see the value and the bar after does; funding rate is usable as
a base in a SOL study.

### WB-06 Cross-sectional mode — `todo` — Blocked by WB-04
For daily panels across many symbols: rank the feature across symbols per date, IC per date,
mean and t across dates, and a costless long-short decile portfolio with its equity curve.
**Done when:** a synthetic panel where the feature is the next-day return gives IC near 1 and the
long-short curve is monotone; the UI offers `time-series` and `cross-sectional` modes.

### WB-07 Aggregation transforms and event studies — `todo` — Blocked by WB-05
`agg daily sum|mean|last|realized_var` lifts intraday series to the daily grid; slow series ride
fast grids by forward fill. For `event` series: average forward and backward return path around
events with counts and t per offset.
**Done when:** a test builds a daily realized variance from 1-minute fixture bars and matches a
direct computation; an event-study fixture with a known post-event drift reproduces it.

### WB-08 Diagnostics: incremental IC, stability, regime buckets — `todo` — Blocked by WB-02
Incremental IC against an "accepted" feature set (regress out, score the residual); IC per day with
sign-consistency count; IC by spread tercile, realized-vol tercile, and hour of day; feature
autocorrelation.
**Done when:** a feature that is a linear copy of an accepted feature reports incremental IC near
zero in a test; the UI shows the per-day IC strip and the regime table.

### WB-09 Presets and promotion — `todo` — Blocked by WB-02
Named feature expressions saved in SQLite, a "promote to accepted" action, and a parquet export of
accepted features plus targets for model fitting.
**Done when:** a preset survives a service restart, promotion changes what WB-08 orthogonalizes
against, and the export round-trips through `tessera parquet-schema`.

### WB-10 Study charts — `todo` — Blocked by WB-02
IC decay across horizons, decile bars, the costless equity curve, and daily IC, as inline SVG in
the terminal style (see `EquityChart`).
**Done when:** each chart renders from a fixture result in the browser without console errors and
the study page opens at the top with the results grid first.

### WB-11 Non-overlapping costless curve for slow features — `todo` — Blocked by WB-02
The WB-02 curve pays every bar the forward return of an `h`-bar hold, so a feature whose position
barely changes (spread_bps: turnover 0.002/bar on the SOL day) reports a breakeven of tens of bps
that no non-overlapping execution would earn. Add a variant that rebalances every `h` bars (or
holds until the position flips) and report its Sharpe and breakeven next to the per-bar ones.
**Done when:** a unit test shows the per-bar and rebalanced variants agree for `h = 1` and the
rebalanced breakeven of a constant-position feature is finite and far below the per-bar figure.

## Housekeeping

### HK-01 Required symbols from the manifest — `done 2026-09-06 HK-01: required symbols from the manifest`
A manifest can declare symbols the runner always appends (End-of-Year Dogs needs `IWM.US` for the
hedge even when the form lists only `universe:stocks`).
**Done when:** a test shows the appended symbol present in the plan when omitted from the form,
and the Dogs manifest declares it.

### HK-02 Sanitation counts on the run page — `todo`
Show dropped off-calendar rows, dropped spikes, and skipped symbols from the run log in the
Overview tab, with the skipped symbols listed on the Symbols tab.
**Done when:** a run over `examples/data` with an injected holiday row shows the count in the UI.

### HK-03 CI runs scripts/check.sh — `todo`
Replace the hand-written CI steps with `scripts/check.sh --quick` plus the web job.
**Done when:** the workflow file calls the script and a deliberate parity break fails CI locally
with `act` or in a PR.

### HK-04 check.sh works from a worktree — `todo`
From `.claude/worktrees/<name>` the script cannot find `../Tessera-private` (so the private checks
silently skip), a fresh worktree has no `local.toml` (so private strategies do not compile in), and
the private legacy crate builds against the main checkout's engine, so a private strategy that uses
a new SDK method fails the private check until the public commit reaches `main`. Found while
working HK-01 in a worktree.
**Done when:** `scripts/check.sh` run from a worktree resolves the private checkout through the
repository's common git dir (or `TESSERA_PRIVATE_ROOT`), copies or points at the main `local.toml`,
and a test run from a worktree reports the private checks as run, not skipped.

### HK-05 Ticket state derived from git — `todo`
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

### HK-06 Release notes from commits and a changelog archive — `todo` — Blocked by HK-05
`scripts/release-notes.sh <from-ref> [<to-ref>]` lists commits whose subject starts with a
ticket id, grouped by prefix (WB, HK) with date and short sha, as Markdown. `--archive <tag>`
moves the completed tickets' entries out of this file into `CHANGELOG.md` under the tag's
heading, so the queue holds only open work and the changelog is the record of what shipped
when. Tag releases; the notes are the diff between tags.
**Done when:** the script over `3022548..main` lists HK-01 and WB-02 under their sections with
dates; `--archive` on a scratch copy produces a `CHANGELOG.md` containing them and a
`BACKLOG.md` without them; `scripts/check.sh --quick` runs the script's self-test.
