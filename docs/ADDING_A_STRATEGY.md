# Adding a Strategy

**Preferred path: one file.** Drop `src/strategies/user/<id>.rs` implementing `crate::sdk::Strategy`
(see `src/strategies/user/rsi_mean_reversion.rs` and `docs/templates/sdk_strategy_skeleton.rs`), or
create it from the Code workspace. Declare parameters in `manifest()`, write rules in `on_bar()`, and
run it from the UI at any bar resolution. Nothing below is required for that path; it documents the
engine-level contracts the SDK is built on and the older hand-wired strategies.

Everything below the SDK section documents the engine-level contracts the SDK is built on. The
hand-wired `EventStrategy` implementations that predate the SDK are no longer part of this
repository; the full event, scope, and broker design is in
[`STRATEGY_SDK_ARCHITECTURE.md`](STRATEGY_SDK_ARCHITECTURE.md). Strategies you do not want to
publish belong in a separate folder listed under `[strategies] dirs` in `local.toml`; `build.rs`
compiles them in exactly like the bundled examples.

`ConfiguredStrategy` in `src/strategy.rs` remains a compatibility boundary for batch-style
strategies that predate the event engine. It should not be copied as the default design for new work.

## Preferred event-driven contract

An `EventStrategy` declares either `PerInstrument` or `Portfolio` scope and handles causal
`SessionStart`, `BarOpen`, `BarClose`, and `SessionEnd` events. It receives an engine-owned portfolio
snapshot and returns broker-neutral `OrderIntent` values. The same strategy callbacks are intended
for both historical replay and future live feeds.

The strategy must not load vendor files, inspect future bars, calculate broker fills, maintain the
authoritative account balance, write standard artifacts, or render reports.

## What a strategy owns

- Its configuration schema and validation.
- Prior-session feature computation and signal rules.
- Candidate selection, entry, stop, and exit behavior expressed as order intent.
- Position-sizing inputs specific to the strategy.
- A typed run summary for CLI output.

## What remains shared

- US-equity transaction-cost defaults and validation in `src/config.rs`.
- Market-data dependencies and common indicator utilities.
- Portfolio equity accounting conventions.
- Standard artifact serialization and schema validation through `write_standard_artifacts`.
- The HTML report generator, including coverage, equity, drawdown, monthly and annual returns,
  risk metrics, and all/long/short trade breakdowns.

## Standard artifact contract

Every strategy maps its domain results into `StandardRunMetadata`, `StandardDailyRecord`,
`StandardTradeRecord`, and `StandardCoverageRecord`, then calls the shared
`write_standard_artifacts` function. The engine—not the strategy—writes and validates:

- `run_config.toml`: complete resolved configuration and run dates.
- `trades.parquet`: at minimum `trade_date`, `symbol`, `entry_time`, `exit_time`, `pnl`, and
  `return_percent`. Include `direction` and `leverage` when applicable.
- `daily_equity.parquet`: `date`, `ending_equity`, and `fills`.
- `coverage.parquet`: one row per candidate-day with `status` equal to `covered`, `missing_file`,
  or `missing_session`.

The shared writer immediately parses the completed bundle through the same loader used by the UI,
so a schema mismatch fails the run instead of silently falling back to a partial report. The shared
`report` command then owns all metrics, charts, tables, and HTML; strategy-specific report code is
not permitted for a standard UI strategy.

## Tick data, second bars, and book features

With `lake_dir` set in `local.toml`, instruments named `EXCHANGE:SYMBOL` (for example
`PARADEX:SOL-USD-PERP`) come from a parquet tick lake laid out as
`<lake>/<feed>/exchange=<EX>/symbol=<SYM>/date=<YYYY-MM-DD>/*.parquet` with feeds `trades`
(recvTimestampMicros, price, size, aggressor), `book_snapshots` (bookEpoch, anchor, bids/asks
ladders) and `book_events` (bookEpoch, side, price, newSize, action CHANGE/DELETE). Sessions are
UTC days.

Resolutions `1s`, `5s`, `15s`, `30s` (any second count dividing a minute) build regular bars from
trades; buckets without trades repeat the last close with zero volume. The engine rebuilds the L2
book from snapshots and deltas (epoch resets, crossed-book repair) and samples it at every bar
close into `bar.book`:

```rust
if let Some(book) = bar.book {
    // book.bid, book.ask, book.mid, book.microprice, book.spread_bps,
    // book.obi_l1 / obi_l5 / obi_l10 (in [-1, 1]), book.bid_depth_l5, book.ask_depth_l5,
    // book.trade_count, book.buy_volume, book.sell_volume,
    // book.trade_imbalance(), book.microprice_bps()
    if book.obi_l1 > 0.6 && ctx.is_flat() { ctx.buy(Size::Default); }
}
```

`bar.book` is `None` for CSV bars and for the first bars of a session until the book is rebuilt.
`tessera lake-diagnose --lake <dir> --symbol EXCHANGE:SYMBOL --date YYYY-MM-DD` reports how often
the rebuilt touch brackets the venue's trades; expect 97% or better on a healthy feed.

## Feature studies

Before writing a strategy, measure whether a feature predicts anything:

```bash
tessera study --config study.toml --start 2026-06-25 --end 2026-06-29 --output-dir artifacts/obi
```

with

```toml
lake_dir = "/path/to/lake"
symbols = ["PARADEX:SOL-USD-PERP", "BINANCE_FUTURES:SOLUSDT"]
step_secs = 1                       # lake sampling grid; horizons and delay are in bars of it
# resolution = "daily"              # or "5m" / "1m": CSV bars through the SDK loader instead
# daily_dir = "examples/data/eod"   # with five_minute_dir / one_minute_dir, calendar_symbol
features = ["obi_l1", "obi_l5", "obi_l10", "microprice_bps", "trade_imbalance", "spread_bps", "return_1"]
horizons = [1, 5, 30, 60]
decision_delay_bars = 1             # bars between observing the book and acting
target = "return"                   # what features are scored against (see below)
```

Features are expressions: a base series, then any number of streaming transforms joined by `|`.

```text
obi_l1                                     a base on its own
signed_volume | zscore 30                  z-score over the last 30 bars
trade_count | rate 1 | ratio_to sma 300    trades per second vs. their 5-minute mean
obi_l1 | diff 1                            change since the previous bar
obi_l1 | times (spread_bps | zscore 60)    interaction; parentheses nest a pipeline
```

Bases: `obi_l1`, `obi_l5`, `obi_l10`, `microprice_bps`, `spread_bps`, `trade_imbalance`,
`return_n`, `signed_volume`, `bid`, `ask`, `mid`, `microprice`, `bid_size`, `ask_size`,
`bid_depth_l5`, `ask_depth_l5`, `trade_count`, `buy_volume`, `sell_volume`, `volume`, `close`,
`range_bps`, `gap_bps`, `high_n_distance`. `return_n` and `high_n_distance` take any window
(`return_5`, `high_252_distance`: the close against the highest high of the last 252 bars, in
bps, never above zero); `range_bps` is the bar's high-low range over its close; `gap_bps` is the
open against the previous close.

**Diagnostics.** Every time-series cell carries diagnostics: the incremental IC, which is
the Spearman IC of the feature's residual after an ordinary least-squares fit on the study's
`accepted` expressions (the form's "Accepted features"), so a feature that only restates an
accepted one scores near zero however good its plain IC; IC per day with the share of days that
carry the cell's sign; IC by spread tercile, trailing 60-bar realized-variance tercile, and
hour of the bar's clock (when the study spans more than one hour); and the feature's
autocorrelation at lag 1 and at the horizon. `daily_ic.csv` and `regimes.csv` hold the tables;
the studies page shows the per-day IC strip and the regime table for the selected cell.

**Presets and promotion.** The studies page keeps a feature library: named expressions saved
in the console's catalog database (`GET`/`POST /api/features`), so they survive a service
restart. "Use" puts one among a study's candidates; "promote" (`POST /api/features/{id}/promote`
with `{"accepted": true}`, or the "promote to accepted" action on a cell in the results) moves it
into the accepted set, which every later study regresses its candidates against on top of the
expressions typed into the form. A promoted feature that cannot run on the chosen grid (an
order-book base on daily bars) is left out of that study rather than blocking it. A study with an
accepted set also writes `accepted.parquet` next to its results: `symbol`, `time_us` (the bar's
close, microseconds UTC), `date`, one Float64 column per accepted expression, and
`target_<horizon>` per horizon holding what the study scored that bar against (the target
`decision_delay_bars` later, over the horizon; warm-up bars are dropped, targets past the end of
the data are NaN). It downloads from the results strip (`GET /api/studies/{id}/export`) and reads
with `tessera parquet-schema --path <file>` (`--csv-out` for a CSV copy), ready for a model fit.

**Aggregation and lifting.** On an intraday grid `agg daily` is a running intraday feature
(today's volume so far, today's realized variance so far). On the daily grid it lifts: with
`intraday_source = "1m"` (or `"5m"`; the service picks the finest library present) each
`agg daily` feature is computed on that intraday panel of the same symbol and the day's final
value lands on the daily bar, so a daily study can score `return_1 | agg daily realized_var`
built from minute bars. Without a source such features are reported unavailable on the daily
grid. Slow series ride fast grids the other way round, by the `level` forward fill of the
series registry.

**Event studies.** Every registered `event` series gets an event study in the result (and
`events.csv`): for each event bar (the first bar that could see the row) the cumulative return
from the event bar to each offset in `-event_window ..= +event_window` (default 20 bars),
averaged over events with a full window, with a t-statistic and count per offset. The path is
normalised to zero at the event, so a run-in shows as negative values climbing to zero and
post-event drift as the values after it. The studies page lists it under EVT.

**Cross-sectional mode.** `mode = "cross_sectional"` (the form's Mode select) reads a
many-symbol panel across symbols instead of along time: on every date with at least `buckets`
symbols, the feature is ranked across symbols against their targets, giving an IC per date
(the cell's IC is the mean, with a t across dates and the share of positive dates), decile
means, and a costless long-short decile portfolio, long the top decile and short the bottom,
equal weights, rebalanced every date, whose cumulative curve, Sharpe, turnover (absolute weight
change per date; a full swap of both sides is 4), and breakeven cost fill the cell. Only the
pooled `ALL` cell exists in this mode. Meant for daily panels; the time-series mode remains the
default.

**Exogenous series.** Anything observed outside the bar stream joins a study as a base:
declare it under `[[data.series]]` in `local.toml` (name, CSV or parquet path, `kind` of
`level` or `event`, the time and value columns, an optional `symbol_column`, and either an
`available_at_column` or a fixed `publication_lag_secs`), and write `cpi_surprise | zscore 12`
like any other feature. Rows join each bar as-of the moment they became observable, never their
nominal date: a value for the 3rd published on the 5th is invisible on the 3rd and 4th. `level`
series carry forward; `event` series are the value on the first bar that sees them and zero
elsewhere. Lake instruments register `funding_rate`, `funding_annualized`, `open_interest`, and
`open_interest_usd` on their own, available at the receive timestamp. The result lists the
series it had; the CLI prints them under the grid line.

**Grids.** A study runs on tick-built lake bars (`step_secs`, the default) or, with
`resolution = "daily"`, `"5m"`, or `"1m"`, on CSV bars read through the same SDK loaders a
backtest uses (daily prints sanitized against `calendar_symbol`, intraday prints in the regular
session), with plain symbols such as `SPY.US` and `daily_dir` / `five_minute_dir` /
`one_minute_dir` from `local.toml`. On a CSV grid the close stands in for the mid, returns and
the OHLCV bases (`return_n`, `range_bps`, `gap_bps`, `volume`, `close`, `high_n_distance`) work
as usual, and any expression that reads the order book is reported as "unavailable on this
grid" in the result instead of failing the study; `spread_change` and `microprice_residual`
targets need the lake. Horizons are bars of the grid (`5` on daily bars is five sessions), and
Sharpe annualizes with 252 sessions on CSV grids. A daily example:

```bash
tessera study --config study.toml --start 2019-01-01 --end 2025-12-31 --output-dir target/daily_study
```

```toml
resolution = "daily"
daily_dir = "examples/data/eod"
calendar_symbol = "DEMO.US"
symbols = ["DEMO.US"]
features = ["return_1 | zscore 20", "range_bps | zscore 20", "high_252_distance"]
horizons = [1, 5, 20]
decision_delay_bars = 1
```
Transforms: `ema n`, `sma n`, `zscore n`, `diff n`, `lag n`, `rate n` (sum over `n` bars per
second), `ratio_to <transform>` (the value over a transform of itself), `pct_rank n`, `abs`,
`sign`, `clip lo hi`, `times <base | (expr)>`, `agg daily sum|mean|last|realized_var` (the
day's fold so far, resetting when the bar's date changes; `return_1 | agg daily realized_var`
is the session's realized variance in bps²). Windows count bars; a bar without a book is `NaN`
and a `NaN` inside a window propagates, so a lag is always a lag in bars. Unknown names fail with
the list of what exists. The grammar lives in `src/feature_expr.rs`.

`target` chooses what every feature is scored against over the horizon: `return` (mid return
from acting to the horizon, bps, the default), `realized_variance` (sum of squared bar-to-bar
mid returns over the horizon, bps²), `abs_move` (absolute return, bps), `spread_change` (quoted
spread at the horizon minus the spread when acting, bps), `fair_value_residual` (mid minus its
60-second EMA at the horizon, in bps of mid), and `microprice_residual` (mid minus microprice at
the horizon, bps of mid). IC, deciles, and the costless curve all run on the chosen target, so a
spread feature studied against `realized_variance` asks whether wide quotes precede busy
prints, and `fair_value_residual` asks whether a feature predicts the price sitting away from a
slow fair value once the horizon has passed.

The study reports, per symbol and pooled, the Spearman rank correlation between the feature and the
forward target (the information coefficient), decile mean forward returns in basis
points, and a t-statistic for top-minus-bottom decile. Each cell also carries a costless curve:
the position is the feature's z-score clipped to +-3 (and a `sign` variant), the P&L per bar is
that position times the forward return with no costs. From it come an annualized Sharpe (one
independent period per horizon, a market that never closes), turnover (mean absolute position
change per bar), and the breakeven cost in bps per unit traded (mean P&L over turnover): the
cost at which the edge pays nothing. `study.csv` has the numbers, `curves.csv` the cumulative
P&L series (up to 400 points per curve), and the UI's Studies page ranks cells by breakeven cost
and draws the curve of the selected cell. Use receive timestamps and a non-zero delay, or the
edge will look better than it is.

## Platform sizing and price guards

`Size::Default` uses the run's `position_percent` (fraction of equity). The simulated broker then
applies account guards at fill time, because the fill price can differ from the price the strategy
sized against:

- **Sanitized prints.** Daily files are cleaned before replay (`[data] sanitize_prices`, default
  on): rows on dates the calendar symbol did not trade are dropped (a $1,000,000 close on New
  Year's Day), one-bar spikes that revert next session are dropped (35 → 993 → 35), and a symbol
  whose close moves more than 8x overnight and stays there is skipped with a warning, because
  the file is mis-scaled from that day on. Sub-dollar prints are not judged. The run log
  reports the counts. Nasdaq test symbols (ZVZZT and friends) never enter a universe.
- **Raw prices.** Bars are split- and dividend-adjusted, so `bar.close` is comparable across time
  but not to a price floor or a dollar-volume threshold. `bar.raw_close()` returns the unadjusted
  print in every hook (`on_bar`, `on_daily_bar`, `screen`); the host carries each symbol's
  adjustment factor as a step function, so a name that reverse-split 1:20,000 shows its true
  $0.0025 print, not the adjusted $50. Use it for liquidity (`raw_close() * volume`) and for
  strategy-level price floors.
- **Minimum price.** Entries are skipped when the reference price is below the run's `min_price`
  (default $1.00, set 0 to disable). Sub-dollar prints make fixed-tick slippage and per-share
  commission meaningless.
- **Buying power.** Open entry notional may not exceed `max_gross_exposure` times total equity. A
  fill that would breach it is cut to the remaining buying power, or rejected when nothing fits.
  Runs default to the manifest's `.max_gross_exposure(x)` declaration, else to
  `max(1, position_percent x max_open_positions)`, so ten 10% slots mean a cash account and a
  volatility-targeted strategy declares the leverage it needs.
- **Solvency.** No entry fills once total equity is zero or negative.
- **Commission cap.** Per-unit commission is capped at `max_commission_percent_of_notional`
  (default 1% of the fill's value, the usual broker rule) so a 173-million-share position in a
  $0.0001 stock cannot pay more in commission than it is worth.
- **Tick floor.** Fill prices never round below one tick, so a slippage tick cannot produce a zero
  or negative price.

**Screened universes.** For intraday logic across a large universe, declare
`.screened_universe()` and implement `screen()`: it receives every daily bar of every symbol and
returns whether to load intraday bars for the next session. `src/strategies/user/rsi_intraday_screened.rs`
is the bundled example (price and dollar-volume screen, intraday RSI, flat by the close); a month
across all US stocks on 5-minute bars runs in about 20 seconds.

**Required symbols.** A strategy that needs an instrument the run form may not list (the
hedge ETF of a universe strategy) declares `.required_symbols(&["IWM.US"])`. The runner appends
those symbols to every run after universe expansion, whether the run comes from the form or a
frozen config, and skips ones already listed. Use the full `SYMBOL.US` form. `.run_defaults(...)`
only pre-fills the form; a required symbol survives the user editing the list.

**Memory budget.** Standard-mode runs hold every selected symbol's bars for the window. Before
reading any file the runner estimates the bar count from file sizes and date spans and refuses runs
above half of physical memory (override with `TESSERA_MEMORY_BUDGET_GB`), naming the estimate and
the alternatives: shorter window, fewer symbols, daily bars, or a screened-universe strategy, which
loads intraday data only for candidate days. A 5-minute replay of all US stocks over six years is
about 774 million bars, roughly 93 GB.

The all-US-stocks RSI run that motivated these guards sized 10% of equity at a $0.0001 reference
price, filled at the $0.01 tick (8.7x equity), paid $1.7M in commission, and kept trading with
negative equity. Rejected fills appear as `OrderRejected` events with the reason.

## Event-driven strategy checklist

1. Add `src/strategies/<strategy_id>.rs` and implement `EventStrategy`.
2. Choose `PerInstrument` for isolated symbol state or `Portfolio` for synchronized cross-asset
   decisions.
3. Register it behind a `crate::sdk::Strategy` adapter or a CLI command, and freeze its run
   configuration next to the results it produces.
4. Express entries, exits, brackets, and stop changes with `OrderIntent`; keep fills and capital in
   the broker adapter.
5. Use the engine-owned historical runner and standard artifact writer; do not add a custom replay
   loop or report generator.
6. Test no-look-ahead behavior, symbol isolation or portfolio synchronization, execution ordering,
   costs, and deterministic artifacts.
7. For a migration, freeze the prior outputs first and require explicit parity or document every
   intended behavior change.
