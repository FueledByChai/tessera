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
