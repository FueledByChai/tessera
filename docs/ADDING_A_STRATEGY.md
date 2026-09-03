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
step_secs = 1                       # sampling grid; horizons and delay are in bars of it
features = ["obi_l1", "obi_l5", "obi_l10", "microprice_bps", "trade_imbalance", "spread_bps", "return_1"]
horizons = [1, 5, 30, 60]
decision_delay_bars = 1             # bars between observing the book and acting
```

The study reports, per symbol and pooled, the Spearman rank correlation between the feature and the
forward mid-price return (the information coefficient), decile mean forward returns in basis
points, and a t-statistic for top-minus-bottom decile. The UI's Studies page runs the same thing
against the configured lake. Use receive timestamps and a non-zero delay, or the edge will look
better than it is.

## Platform sizing and price guards

`Size::Default` uses the run's `position_percent` (fraction of equity). The simulated broker then
applies account guards at fill time, because the fill price can differ from the price the strategy
sized against:

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
