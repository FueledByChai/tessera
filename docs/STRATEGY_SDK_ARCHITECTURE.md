# Event-Driven Strategy SDK Architecture

Status: foundational historical runtime implemented September 1, 2026. Live connectivity remains
deferred.

## Product goal

Tessera should provide TradingView-like strategy authoring with portfolio-aware execution. A
strategy author writes parameters, indicator/feature state, signal rules, optional ranking, and order
intent. The platform owns data access, clocks, synchronized events, simulated or live execution,
positions, cash, costs, immutable provenance, standard artifacts, reports, comparisons, and research
workflows.

Strategy code must not branch on whether it is running historically or live. Historical files and a
future broker feed are adapters that emit the same market events. A simulated broker and a future
live broker consume the same order intents.

## One-file authoring layer (September 2, 2026)

`src/sdk/` wraps the event engine in a TradingView-style authoring surface:

- `manifest.rs`: parameters and metadata declared in the strategy file, validated and resolved with defaults; drives the run form and the catalog entry.
- `strategy.rs`: the `Strategy` trait (`manifest`, `new`, `on_bar`, optional session and fill hooks), the `Ctx` order/account API, and `SdkInstance`, which adapts one instance per symbol onto `EventStrategy`.
- `indicators.rs`: streaming indicators that return `None` until warm.
- `runner.rs`: resolution-agnostic data loading (daily adjusted, 5m, 1m; regular or extended session), warm-up replay before the requested start, one shared simulated account, and the standard artifact bundle plus report.
- `build.rs`: discovers `src/strategies/user/*.rs`, so a strategy is registered by existing.

The lower-level `EventStrategy` contract below remains available for strategies that need portfolio scope or custom session assembly.

## Authoring boundary

The event SDK is implemented in `src/event_engine.rs`. A strategy implements `EventStrategy`:

```rust
pub trait EventStrategy {
    fn id(&self) -> &'static str;
    fn scope(&self) -> StrategyScope;

    fn on_market_event(
        &mut self,
        event: &MarketEvent,
        portfolio: &PortfolioSnapshot,
    ) -> Result<Vec<OrderIntent>>;

    fn on_broker_event(&mut self, event: &BrokerEvent) -> Result<Vec<OrderIntent>>;
}
```

A minimal moving-average example lives in `src/strategies/user/moving_average_cross.rs`, written
against the one-file SDK that sits on top of this engine. It owns only the two averages, the
crossing logic, parameters, and entry/exit intent. It does not open files, calculate P&L, apply
commissions, or render reports; `src/sdk/` adapts it onto the event engine described here.

## Event semantics

Historical sessions and future live feeds use this causal sequence:

1. `SessionStart`
2. `BarOpen` containing only the opening price currently observable
3. `BarClose` containing completed OHLCV
4. repeated bar-open/bar-close events
5. `SessionEnd`

The split between `BarOpen` and `BarClose` is deliberate. A signal based on a completed bar may place
an order for the next bar open without exposing that next bar's high, low, close, or volume.

The engine processes broker events before strategy callbacks at each market event. Existing stops and
targets therefore execute before a strategy reacts to that bar's close. Orders returned by the
strategy are then submitted to the broker adapter, and resulting fills/rejections are returned to the
strategy.

## Strategy scopes

### Per instrument

The engine creates one isolated strategy state for each selected instrument. One strategy source can
therefore run on QQQ alone or independently on QQQ, SPY, IWM, and DIA. The engine filters each market
slice and broker event to the appropriate instance.

### Portfolio

One strategy instance receives the synchronized slice for every selected instrument. This is the
scope for cross-sectional ranking, relative-value signals, shared capital, top-N selection, and
portfolio-level allocations.

The host rejects a strategy whose declared scope does not match its configured host. Instrument
selection remains run configuration rather than hardcoded strategy behavior.

## Broker boundary

`BrokerAdapter` owns market-event processing, order submission, fills, positions, and the portfolio
snapshot. `SimulatedBroker` currently supports:

- immediate and next-bar-open market entries;
- bracket stops and profit targets;
- deterministic stop-before-target handling when both occur in one bar;
- immediate, next-open, and current-close exits;
- stop replacement and managed-stop audit state;
- per-unit commissions and adverse entry/exit ticks; and
- normalized completed trades and realized equity.

`LiveBrokerAdapter` adds connection health and broker reconciliation to the same boundary. It is an
interface only in this milestone; no order can currently reach a brokerage account.

## Historical engine

`HistoricalEventEngine` replays stored sessions through the same event callbacks that a live adapter
will use. It supports per-instrument and portfolio hosts, deterministic symbol ordering, broker-event
feedback, and bounded follow-up order handling. Bulk CSV/Parquet/Polars work may still prepare the
event stream; the time-sensitive strategy and portfolio transitions remain causal and sequential.

The simulated broker maintains both realized and marked total equity. At the end of a finite
historical replay it discards unfillable next-open orders and liquidates remaining positions at the
last observed price with configured exit costs. This makes multi-session strategies report complete,
auditable equity without embedding backtest-end behavior in their signal code.

## ETF ORB migration and parity

ETF ORB is the first migrated production-research strategy. The historical adapter validates the
premarket reference and complete regular session before replay. The strategy then:

- records the premarket reference at `BarOpen`;
- builds the 09:30–09:45 range from completed bars;
- evaluates only information available at each decision boundary;
- schedules the entry for the correct later bar open after all configured refinements;
- submits broker-neutral bracket intent;
- manages stops and conditional exits from completed bars; and
- updates its causal quarterly training labels only after the session completes.

The frozen QQQ comparison used 2020-01-01 through 2026-08-28. Both implementations produced 1,661
covered sessions, 1,557 trades, 21.59% CAGR, 1.88 Sharpe, and -9.19% maximum drawdown. The normalized
daily-return, trade, and coverage CSV files were byte-identical. A synthetic deterministic parity test
also runs in the normal Rust test suite.

## Shared reporting

The existing standard artifact contract remains the reporting boundary. The engine owns Parquet
serialization, schema validation, metrics, charts, tables, and report presentation. Migrated strategy
wrappers still map domain-specific completed trades and coverage into the standard records; further
strategy migrations should keep reducing wrapper-specific result assembly rather than reintroducing
custom report generators.

## Explicitly deferred

- QQQ live shadow mode and an operational live-session screen.
- IBKR paper/live market data, order routing, reconciliation, and safety controls.
- Porting the remaining pre-SDK strategies (kept in a private repository) to one-file SDK
  strategies; Limit Buyer and Gap Fade have been ported and verified against their legacy results.

Those items remain separate from this completed historical foundation so the presence of a live
interface cannot be mistaken for brokerage connectivity.
