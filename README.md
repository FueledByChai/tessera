# Tessera

*Tessera* (Latin: one tile of a mosaic). Bars, fills, and runs are the tiles; the portfolio is the picture.

An event-driven backtesting engine written in Rust, with a local browser UI styled after a
trading terminal. Strategies are single Rust files: declare the parameters, write the bar
logic, and the engine handles replay, order simulation, position sizing, costs, portfolio
limits, and reporting. Strategies are resolution-agnostic, so the same file runs on daily,
five-minute, or one-minute bars, and on one symbol or a screened universe of thousands.

```rust
use crate::sdk::prelude::*;

pub struct RsiMeanReversion { rsi: Rsi, oversold: f64, overbought: f64 }

impl Strategy for RsiMeanReversion {
    fn manifest() -> Manifest {
        Manifest::new("rsi_mean_reversion", "RSI Mean Reversion", "v1")
            .param(Param::int("length", 14).range(2.0, 200.0))
            .param(Param::decimal("oversold", 10.0).range(1.0, 50.0))
            .param(Param::decimal("overbought", 90.0).range(50.0, 99.0))
    }
    fn new(p: &Params, _symbol: &str) -> Result<Self> {
        Ok(Self { rsi: Rsi::new(p.usize("length")?), oversold: p.decimal("oversold")?, overbought: p.decimal("overbought")? })
    }
    fn on_bar(&mut self, ctx: &mut Ctx, bar: &Bar) -> Result<()> {
        if let Some(rsi) = self.rsi.update(bar.close) {
            if rsi < self.oversold && ctx.is_flat() { ctx.buy(Size::Default); }
            if rsi > self.overbought && ctx.is_long() { ctx.close("overbought"); }
        }
        Ok(())
    }
}

pub fn entry() -> StrategyEntry { StrategyEntry::of::<RsiMeanReversion>() }
```

Drop a file like that into `src/strategies/user/` and it appears in the UI with its parameters
exposed as form fields. No registry edits, no module lists.

## Features

- **One-file strategies** with a small API: `buy`, `sell_short`, `buy_limit`, `close`,
  `set_stop`, `exit_at`, `exit_after_minutes`, and per-symbol or shared state.
- **Deterministic simulation**: next-bar-open or this-bar-close execution, resting limit orders
  with gap vetoes and expiry, stops checked conservatively on the same bar, timed exits, and
  day-order cancellation at the session close.
- **Portfolio-aware**: per-day entry caps, open-position caps, and priority, random (seeded),
  or alphabetical tie-breaks when more signals fire than the limits allow.
- **Universe screening**: strategies can screen a daily universe and the engine loads intraday
  data only for candidate symbol-days, so a year across all US common stocks runs in seconds.
- **Costs**: tick slippage and per-unit commission or an all-in round-trip basis-point charge,
  managed as immutable cost profiles in the UI.
- **Reports**: standalone HTML tear sheet with equity, underwater, monthly returns, rolling
  Sharpe, trade distribution, and coverage qualification, plus Parquet artifacts for every run.
- **Local UI**: strategy catalog, instrument search, run history with starred runs, sweeps,
  portfolios that combine starred runs, a data-library page, and a code workspace that
  creates, builds, and releases strategy versions in isolated checkouts.
- **Bring your own data**: point `local.toml` at any CSV-folder library. A synthetic dataset is
  bundled so everything works from a fresh clone.

## Quick start

Requires a recent stable Rust toolchain and, for the UI, Node 20 or later.

```bash
cargo build --release
cargo run --release --bin tessera -- run-strategy \
  --config examples/configs/rsi_mean_reversion.toml \
  --start 2019-01-01 --end 2025-12-31 \
  --output-dir artifacts/rsi_demo
open artifacts/rsi_demo/report.html
```

`examples/configs/` holds a run configuration for each bundled example, and
`cargo run --release --bin tessera -- sdk-manifests` prints every discovered strategy and
its parameters as JSON.

### Local UI

```bash
cargo build --release --bin tessera --bin tessera-ui
./target/release/tessera-ui          # API on http://127.0.0.1:8787
cd web && npm install && npm run dev    # UI on http://127.0.0.1:3322
```

The UI runs strategies through the engine binary, records each run with a frozen parameter
snapshot in a SQLite catalog under `data/ui/`, and stores artifacts under `artifacts/`.
See `docs/LOCAL_UI.md` for the workflows.

## Your own market data

Copy `local.example.toml` to `local.toml` (git-ignored) and fill in the folders:

```toml
[data]
daily_dir = "/data/eod"          # <SYMBOL>.csv: Date,Open,High,Low,Close,Adjusted_close,Volume
five_minute_dir = "/data/5m"     # <SYMBOL>.csv: Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume
one_minute_dir = "/data/1m"
catalog_dir = "/data/catalog"    # catalog.csv plus stocks.txt / etfs.txt universe lists
calendar_symbol = "SPY.US"       # daily file whose dates define the trading calendar
```

Daily bars are adjusted with `Adjusted_close / Close`; intraday bars are raw, and the SDK
exposes both so strategies can compare a previous adjusted close with a raw intraday open
correctly. Any provider works as long as it can be exported to this layout.

## Private strategies

Keep proprietary strategies out of the repository by listing extra folders in `local.toml`:

```toml
[strategies]
dirs = ["../my-private-strategies"]
```

`build.rs` compiles every `*.rs` file in those folders into the engine alongside the bundled
examples, and the UI treats them exactly like in-tree strategies. A file in a later folder with
the same name as an example replaces it.

## Writing strategies

- `docs/ADDING_A_STRATEGY.md` walks through the SDK: manifest, parameters, context API,
  order vocabulary, indicators, daily context, screening, and multi-symbol runs.
- `docs/STRATEGY_SDK_ARCHITECTURE.md` explains the event engine underneath.
- `docs/templates/sdk_strategy_skeleton.rs` is the skeleton the UI uses for new strategies.
- `src/strategies/user/` contains the two bundled examples, RSI mean reversion and moving
  average cross.

## Repository layout

```
src/event_engine.rs   replay loop, simulated broker, order intents, entry arbitration
src/sdk/              one-file strategy SDK: manifest, context, indicators, runner
src/strategies/user/  bundled example strategies (discovered by build.rs)
src/bin/tessera    engine CLI: run-strategy, sdk-manifests, report, combine
src/bin/tessera-ui local API server for the browser UI
src/report.rs         HTML tear sheet and metrics
src/portfolio.rs      combines completed runs into portfolios
web/                  Next.js single-page terminal UI
examples/             synthetic data and run configurations
docs/                 architecture, SDK guide, UI guide, product backlog
```

## Development

```bash
cargo fmt --check
cargo test
cd web && npm run lint && npm run build
```

## License

AGPL-3.0. See `LICENSE`. Strategies you write and keep in private folders are your own; the
license covers the engine and UI in this repository.
