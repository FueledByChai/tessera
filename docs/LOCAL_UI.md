# Tessera local UI

The planned separation between the open-source application, external strategy packages, and
external data libraries is recorded in [OPEN_SOURCE_ARCHITECTURE_DIRECTION.md](OPEN_SOURCE_ARCHITECTURE_DIRECTION.md).
Proposed features and their acceptance criteria are tracked in [PRODUCT_BACKLOG.md](PRODUCT_BACKLOG.md).

Double-click `Launch Tessera.command` in Finder. The first launch may take a few minutes while the Rust service is built. Later launches reuse the compiled service and open `http://127.0.0.1:3322/` directly.

The initial vertical slice provides:

- A local SQLite catalog at `data/ui/tessera_ui.sqlite3`.
- Automatic, non-destructive import of every `artifacts/*/report.html` result as an immutable legacy run.
- Every one-file SDK strategy discovered by `build.rs` (bundled examples plus any folders listed in `local.toml` `[strategies] dirs`), each with its manifest parameters exposed as form fields. Legacy catalog rows imported from older artifacts remain visible for their run history but are not runnable.
- A Strategies catalog rendered as a dense monitor table instead of tiles, so it scales to a large number of entries. Rows are grouped by asset class, status, or shown flat; custom releases nest under their base strategy; columns sort by name, version, status, assets, run count, and last run; a filter box matches name, id, asset, or description; arrow keys move the selection and Enter opens it. A sticky inspector rail shows the selected strategy's description, config, base strategy, source hash, run history summary, and Open / View source actions.
- An isolated two-worker queue.
- One run form for every SDK strategy: universe or explicit symbols, bar resolution, session, position size, capital, entry limits, and the manifest parameters grouped by tier, with explicit dates and research labels.
- Strategy-specific simple and advanced parameter editing, including costs-off research mode.
- A searchable instrument picker backed by `GET /api/instruments`, which indexes the configured catalog (`catalog.csv` plus universe lists and provider sub-catalogs) against the daily, five-minute, and one-minute files actually on disk and returns symbol, name, venue, asset class, currency, listing status, and first/last bar dates per resolution. The picker prevents duplicates, keeps priority order, flags selected symbols that lack data for the chosen resolution, and the API re-validates data presence before the run is queued. The resolved instruments are frozen in each run's `strategy_config.toml`.
- Immutable named presets stored in the local catalog.
- A controlled Sweeps & Comparisons workspace: one- or two-parameter Development grids over any numeric manifest parameter (up to 25 immutable configurations), live sensitivity heatmaps, stability-aware ranking, and normalized side-by-side comparison of up to four completed runs. Sweep ranking never reads Validation or Final Holdout results.
- An immutable Portfolio Builder backed by the engine's event-time capital model. It supports unconstrained full-size overlays, verified non-overlapping capital reuse, and normalized fixed allocations; source costs and lagged volatility targets stay frozen. Every saved portfolio includes an interactive report plus daily, weekly, and monthly return-correlation matrices.
- Per-run output folders and worker logs under `artifacts/ui_runs/`.
- Starred runs and cached run metrics. Each completed run caches CAGR, total return, Sharpe, Sortino, Calmar, max drawdown, annualized volatility, win rate, trade count, and the report window in the catalog (`metrics_json` on `runs`), computed when a job finishes and backfilled in the background at service start for older runs (legacy reports without parquet output fall back to `summary.json` when present, otherwise show no metrics). The strategy page's Historical runs table and the Runs list show those columns, sort by any of them, and filter by research label or starred-only. Any run can be starred from the list or its detail page via `POST /api/runs/{id}/star`; starring is a curation flag and never alters run artifacts. The Portfolio Builder's source list shows starred runs only by default, with a "Show all" toggle.
- Interactive structured run details: log-scale equity, metrics, coverage warnings, monthly and annual returns, long/short breakdown, trades, and the frozen configuration.
- A Data Coverage workspace that audits selected signal sessions by symbol, year, and exact missing symbol-date. This is deliberately labeled signal-session coverage rather than full-calendar vendor coverage.
- A centralized, immutable execution-cost library with fixed-tick/per-unit, all-in basis-point, and costs-off profiles. Entry and exit assumptions are stored separately, compatible global defaults can be overridden by an individual run, and the selected profile is frozen into the run manifest.
- A Data workspace showing the configured library (provider, folders, calendar symbol, latest date, file counts, freshness from the optional `freshness_file`) with a button that launches the optional `update_command` from `local.toml`, plus local schedules for that command. Seeded schedules are disabled by default and execute only while the local service is running.
- Localhost serving for every preserved original HTML report.
- Modern dark and Bloomberg-inspired terminal display modes. Terminal mode is the default and the choice is remembered in browser storage. It uses a command ribbon with a live clock, a bottom status strip (engine state, worker queue, report count, latest US EOD date, display mode, clock), amber function-title bars in place of page intros and hero copy, one monospace family for all text, 20px grid rows with 10px amber headers, outlined amber primary actions, blue text links, dark-amber editable fields, and equity charts drawn as a thin white line over a faint blue fill with dotted gridlines, right-hand equity axis labels, and an amber crosshair with a date / equity / drawdown readout. Dense grids carry units in the column header (CAGR %, Max DD %) rather than on each value. The terminal palette was sampled from Bloomberg WEI, DES, HP, GP, BTMM, ECO, and OVME screens: black ground, navy `#0f0f3a` data panels, amber `#f89828` labels, tags, and identifiers, white `#f0f0f0` values, dark-red `#a00820` section bars with white titles, gray `#383838` column-header bars, green `#32c832` and red `#e03040` for signed values, blue `#6a9bff` text links, and amber-filled editable fields with dark text that turn yellow on focus.
- Readable desktop typography with 15px table bodies and 13px table headers.
- A strategy Code workspace that exposes the one-file Rust source for every runnable strategy, including strategies compiled in from private folders. Built-in and released source is read-only and SHA-256 identified; editable drafts live under `strategy_workspace/drafts/`. "New one-file strategy" writes the SDK skeleton into a draft, "Build & run" compiles a dev engine and registers a `<id>__dev` catalog row, and Release promotes it to an immutable version.
- Versioned source development from the browser: create a strategy from the skeleton or fork an existing one, edit its file, run Rust formatting, compile all targets, execute the library test suite, inspect the complete worker log, and promote a successfully tested draft to a new immutable catalog identity.
- Custom releases receive a dedicated compiled engine binary under `strategy_workspace/releases/`. They share the SDK run configuration and reporting plumbing, but execute the released source instead of the currently installed engine. New backtests preserve a source snapshot and hash in their artifact directory and run manifest.

The existing reports and strategy configurations are not modified. A newly queued run gets its own output directory and database record; the UI never reuses or overwrites an existing run directory.

The seeded cost profiles are:

- US equities: one adverse $0.01 tick plus $0.005/share on entry and exit.
- US equities: 10 bps all-in round trip.
- Spot FX: 4 bps all-in round trip.
- Crypto spot: 10 bps all-in round trip.
- Costs off: zero modeled execution cost for gross-alpha diagnostics.

Custom profiles are append-only. The current fixed-tick strategy adapters require symmetric entry/exit tick and per-unit assumptions and do not yet support a minimum commission; all-in basis-point profiles may store different entry and exit estimates because the engines consume their exact sum.

## One-file strategies (Strategy SDK)

A strategy is a single Rust file in `src/strategies/user/` that implements `crate::sdk::Strategy`: a `manifest()` declaring parameters (int, decimal, bool, choice with defaults, ranges, help, simple/advanced tier), a `new(params, symbol)` constructor, and `on_bar(ctx, bar)`. The context exposes the account (`equity`, `position`, `is_flat`) and accepts orders (`buy`, `buy_with` with stop/target, `sell_short`, `close`, `set_stop`) with platform sizing (`Size::Default`) or strategy overrides (`Size::Percent`, `Size::Units`). Streaming indicators (`Rsi`, `Sma`, `Ema`, `Atr`, `Bollinger`, `Vwap`, `RollingHigh`, `RollingLow`, `Crossover`) come from `crate::sdk::prelude`. `build.rs` discovers every file in that folder, so there is no module list or registry to edit; the reference file is `src/strategies/user/rsi_mean_reversion.rs` and the skeleton is `docs/templates/sdk_strategy_skeleton.rs`.

Strategies are resolution-agnostic. The run form chooses daily, five-minute, or one-minute bars, regular or extended hours, the symbol list (one strategy instance per symbol on a shared account), position size, and initial capital; manifest parameters render automatically. The engine exposes `tessera sdk-manifests` and `tessera run-strategy --config <toml>`; the local service syncs every compiled manifest into the catalog at startup (`base_strategy_id = "sdk"`, `command_name = "run-strategy"`).

The SDK also covers intraday strategies with daily context, resting limit orders, and universe-scale screens. A manifest can declare `daily_context()` (intraday runs receive completed daily bars through `on_daily_bar`), `screened_universe()` (a daily `screen()` pass across every symbol decides which symbol-days get intraday bars, so the full US common-stock universe runs in seconds), `allows_short()`, and `entry_limits(max_per_day, tie_break, seed)`. The context offers `buy_limit`/`sell_short_limit` with expiry, gap veto, percent stop, and timed exits; `exit_at`/`exit_after_minutes`; `priority()` for the daily entry cap; shared run-wide state (`shared_get/set/push/series`); and `symbol_index()` so list order can act as priority. The run form exposes a universe selector (all US common stocks, all ETFs, or both), entry limits (max entries per day, max open positions, priority / seeded-random / alphabetical tie-break), and accepts any cost profile including all-in basis points. Two production strategies ship as one-file SDK versions: `gap_fade` (any ETF list in priority order; first confirmed instrument trades, or largest gap) and `limit_buyer` (screened universe). Their legacy engines remain frozen for existing runs.

From the Code workspace, **New one-file strategy** writes a skeleton draft; **Build & run** compiles a dev engine for the draft, registers `<id>__dev` catalog entries that point at it, and opens the strategy page. **Release** still produces an immutable version with its own engine. Parameter sweeps are not yet available for SDK strategies.

## Strategy code workflow

Open **Code** in the sidebar or use **View source code** from a strategy page. A new strategy starts from one of the existing execution templates so it automatically has a valid data, parameter, cost, and reporting contract.

1. Create a draft from a released strategy.
2. Select and edit the Rust files in the draft bundle, then save each changed file.
3. Run **Format**, **Compile**, and **Run tests**. Validation runs in a separate local worker checkout and never edits the installed production source.
4. After a successful test against the current source hash, choose a new strategy id, name, and version and build the immutable release.
5. Open the newly registered strategy from the catalog and run backtests normally.

Changing any draft file invalidates its previous validation. Released strategies and source bundles cannot be overwritten through the UI. The initial editor deliberately supports strategy families already represented by the engine; adding a completely new data/execution adapter still requires extending the shared Rust plumbing.

## Development startup

Run the API from the repository root:

```sh
cargo run --bin tessera-ui
```

Run the browser application in a second terminal for development:

```sh
cd web
npm run dev -- --host 127.0.0.1 --port 3322
```

The double-click launcher uses the built production server (`npm run build` followed by `npm run start`) so it is independent of any development server. It stages disposable web dependencies and build output under `~/Library/Caches/Tessera/web-runtime`; source code, the SQLite catalog, and immutable run artifacts stay in the Tessera project. This avoids macOS cloud-storage eviction of `node_modules` when the project is under Documents.

The service health check is `http://127.0.0.1:8787/api/health`. Runtime logs for the double-click launcher are written under `data/ui/logs/`.
