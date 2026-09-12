# Product Backlog

This is the working feature and user-story backlog for the Tessera application. It records product
intent and acceptance criteria; it is not evidence that a feature has been implemented.

Statuses: **Proposed**, **Ready**, **In progress**, **Complete**, or **Deferred**.

## Locked product decisions

The locked product decisions are decision records under `docs/decisions/` (records 0004 to
0018, one per decision, with their context, alternatives, consequences, and what would show
each was wrong); see the index at [docs/decisions/README.md](decisions/README.md). A change
is a new record that supersedes the old one, never an edit here.

## Epic A: Strategy, engine, and data separation

### BT-101 — Versioned strategy-package manifest

**Status:** In progress (September 2, 2026): manifests are declared in code (`crate::sdk::Manifest`) with id, version, name, description, rules, asset scope, warm-up, and parameters; the engine exports them via `tessera sdk-manifests` and the catalog syncs from the binary. Source hashes and out-of-checkout packages remain open.  
**User story:** As a strategy author, I want a package to describe itself so the application can
discover and run it without hardcoded strategy-specific UI or registry logic.

**Acceptance criteria:**

- The manifest contains package ID, version, name, source hash, runtime entry point, parameters,
  capabilities, required data, and supported portfolio controls.
- A package outside the application checkout can be registered read-only.
- Changing package contents changes its hash and cannot mutate a released version.
- The application can display a package without launching its trading logic.

### BT-102 — External data-library catalog

**Status:** Proposed  
**User story:** As a user, I want to register market-data locations without embedding their paths in
strategies or distributing the data with the application.

**Acceptance criteria:**

- A data library has an ID, provider, local location, frequencies, fields, asset classes, adjustment
  semantics, coverage, and revision/snapshot metadata.
- Strategies request data through dataset IDs and instrument IDs rather than absolute paths.
- Removing or moving a data library produces a clear unavailable-data error without corrupting old
  runs.
- Raw vendor files remain outside the open-source repository.

### BT-103 — Versioned strategy-worker protocol

**Status:** Proposed  
**User story:** As an application maintainer, I want strategy code to execute in a separate process
so private or third-party packages are not linked into the web service.

**Acceptance criteria:**

- The engine sends a frozen, versioned run request to a worker process.
- The worker emits structured progress and the standard artifact bundle.
- A failed, timed-out, or malformed worker cannot crash the web service.
- The protocol is language-neutral even if the first SDK supports Rust only.

### BT-104 — Complete immutable run provenance

**Status:** Proposed  
**User story:** As a researcher, I want every result to identify its exact strategy, data, universe,
parameters, and costs so it can be audited and reproduced later.

**Acceptance criteria:**

- A run freezes engine version, strategy package/hash, dataset snapshots, requested selection,
  resolved instruments, parameters, costs, calendars, adjustment rules, and random seeds.
- Reports clearly identify missing data and survivorship-biased universe membership.
- Later package, catalog, or preset changes do not alter an existing run.

## Epic B: Instruments and universes

### BT-201 — Searchable explicit instrument picker

**Status:** Complete (September 1, 2026) for Gap Fade, Moving Average Cross Sample, ETF ORB, and Crypto Daily Trend via `GET /api/instruments` and the run-form picker. Provider symbol and canonical symbol are both the EODHD symbol for now; strategy-level field requirements are expressed as required bar resolutions. Universe-driven strategies (Limit Buyer, Two-Day Rebound Short, Overnight Attention), the FX pair grid, and MNQ remain outside the picker.  
**User story:** As a user, I want to add one or more specific instruments to a backtest, such as BTC,
ETH, and SOL, without editing a configuration file.

**Acceptance criteria:**

- Search results show canonical symbol, provider symbol, venue, asset class, quote currency, and data
  coverage.
- Duplicate selections are prevented.
- The strategy's required frequency and fields are checked before submission.
- The resolved instrument identities are frozen in the run.

### BT-202 — Saved, versioned static universes

**Status:** In progress (September 2, 2026): SDK runs accept `universe:stocks`, `universe:etfs`, and `universe:all`, expanded from the EODHD universe files and frozen into the run config. Saved custom lists remain open.  
**User story:** As a user, I want to select a named basket such as US ETFs or G10 USD FX pairs rather
than adding every instrument manually.

**Acceptance criteria:**

- Users can create, name, clone, and version an instrument basket.
- Runs freeze the exact resolved membership rather than only the basket name.
- Current-membership universes are explicitly labeled as survivorship-biased for historical tests.

### BT-203 — Point-in-time dynamic universes

**Status:** Deferred  
**User story:** As a cross-sectional researcher, I want universe membership resolved separately for
each historical session so delistings and membership changes are not silently omitted.

**Acceptance criteria:**

- Membership source and effective dates are recorded.
- Delisted instruments remain addressable when historical data exists.
- The report distinguishes membership exclusions from missing price data and failed strategy rules.

### BT-204 — Strategy/data compatibility gate

**Status:** Proposed  
**User story:** As a user, I want to know before a run whether the selected instruments and data can
actually support the strategy.

**Acceptance criteria:**

- The gate checks asset class, fields, frequency, sessions, extended-hours needs, calendars, shorting,
  and fractional sizing.
- Unsupported combinations are blocked by default with a concrete explanation.
- Any experimental override is explicit and frozen into the run.

## Epic C: Manifest-driven parameters and UI

### BT-301 — Typed parameter declarations

**Status:** Complete (September 2, 2026) for SDK strategies: int, decimal, bool, and choice parameters with defaults, bounds, steps, units, help, and simple/advanced tiers, validated once in Rust and re-resolved at run start. Cross-parameter constraints live in the strategy constructor.  
**User story:** As a strategy author, I want to declare configurable parameters once so Rust
validation, the UI, presets, and research tools share one contract.

**Acceptance criteria:**

- Supported types include integer, decimal, boolean, enum, duration/time, string, and instrument.
- Declarations include defaults, units, validation limits, steps/choices, help text, UI grouping, and
  simple/advanced/expert tiers.
- Cross-parameter constraints such as `fast_length < slow_length` are supported.
- Strategy code validates the resolved values again at worker startup.

### BT-302 — Generic parameter-form renderer

**Status:** Complete (September 2, 2026) for SDK strategies: the run form renders manifest parameters by kind plus platform settings (symbols, resolution, session, position size, capital).  
**User story:** As a user, I want an appropriate control for every strategy parameter without each
strategy needing a custom page.

**Acceptance criteria:**

- Numeric, checkbox, dropdown, time, and instrument controls are generated from the manifest.
- Invalid fields and cross-field failures show actionable messages before submission.
- Simple, advanced, and expert views are generated from parameter metadata.
- Existing immutable presets continue to work with compatible package versions.

### BT-303 — Explicit optimizer eligibility and domains

**Status:** Proposed  
**User story:** As a strategy author, I want to state which parameters are reasonable to search and
over what domain so research tools do not optimize arbitrary configuration values.

**Acceptance criteria:**

- Editable and optimizer-eligible are separate flags.
- Validation domains and narrower recommended optimization domains are separate.
- Numeric linear/log grids and categorical choices are supported.
- Position size, gross leverage, costs, research dates, and holdout definitions default to not
  optimizer-eligible.
- A user may include size or leverage only in a clearly labeled sensitivity study.

## Epic D: Portfolio sizing and exposure

### BT-401 — Percent-of-equity position targets

**Status:** Proposed  
**User story:** As a user, I want every position target expressed as a percentage of current account
equity so sizing remains understandable across one or many selected instruments.

**Acceptance criteria:**

- 100% produces notional equal to current account equity; 200% produces 2.0x notional.
- Sizing uses the configured decision-time equity without look-ahead.
- Rounding and fractional-unit behavior are asset-capability aware and recorded.

### BT-402 — Portfolio gross and net exposure limits

**Status:** In progress (September 2, 2026): account-wide entry limits (max entries per day, max open positions) with priority, seeded-random, or alphabetical tie-break are enforced by the simulated broker at fill time for SDK runs. Gross and net notional caps remain open.  
**User story:** As a user, I want portfolio-level exposure limits enforced when several instruments
signal simultaneously.

**Acceptance criteria:**

- Gross exposure is the sum of absolute notionals divided by current equity.
- Net exposure is signed notional divided by current equity.
- The engine has deterministic behavior when requested orders exceed a limit.
- Rejected, clipped, or deferred orders are captured in an allocation audit artifact.

### BT-403 — Fixed targets without automatic rescaling

**Status:** Proposed  
**User story:** As a user, I want unused exposure to remain unused unless I explicitly choose a
rescaling rule.

**Acceptance criteria:**

- Two selected instruments at 100% each and a 2.0x cap produce 2.0x when both signal.
- The same configuration produces 1.0x when only one signals.
- A single instrument at 200% with a 2.0x cap produces 2.0x.
- The default engine never enlarges a position merely because another instrument has no signal.

### BT-404 — Exposure sensitivity studies

**Status:** Proposed  
**User story:** As a researcher, I want to compare position-size and leverage assumptions without
letting an optimizer quietly select the riskiest result.

**Acceptance criteria:**

- Size and leverage can be sweep axes in a run explicitly labeled `Sensitivity`.
- They are excluded from ordinary optimization and walk-forward selection by default.
- Reports normalize risk metrics and show gross/net exposure histories for comparison.

## Epic E: Optimization and walk-forward research

### BT-501 — Manifest-driven controlled sweeps

**Status:** Proposed  
**User story:** As a researcher, I want sweep controls generated from optimizer-eligible parameters
so the UI does not maintain a separate hardcoded list.

**Acceptance criteria:**

- The UI starts from recommended domains but allows narrower user-defined domains.
- Invalid combinations are excluded with recorded reasons.
- Combination limits, costs, dates, seeds, and evidence requirements are frozen.
- Validation or final-holdout outcomes cannot influence Development ranking.

### BT-502 — Walk-forward definition and execution

**Status:** Proposed  
**User story:** As a researcher, I want repeated train/select/test folds so parameter selection can be
evaluated out of sample.

**Acceptance criteria:**

- The definition includes training length, test length, step, objective, search parameters, minimum
  trades, and tie-breaking rules.
- Each training candidate and selected out-of-sample run is immutable and linked to its fold.
- Selection reads only the training window.
- Failed or data-incomplete folds remain visible rather than disappearing.

### BT-503 — Walk-forward report

**Status:** Proposed  
**User story:** As a researcher, I want to see whether a tuned strategy remains stable rather than
only seeing the best training result.

**Acceptance criteria:**

- The report shows every fold's training period, selected parameters, test period, and test metrics.
- The headline equity curve is stitched solely from out-of-sample fold returns.
- Parameter instability and selection turnover are visualized.
- In-sample, out-of-sample, post-selection, and final-holdout results are unmistakably labeled.

## Epic F: Data inventory, coverage, and freshness

### BT-601 — Data-source inventory

**Status:** Proposed  
**User story:** As a user, I want to see every registered market-data source so I know where the
application's data comes from and which source is responsible for each dataset.

**Acceptance criteria:**

- The Data workspace lists friendly source/provider name, adapter type, configured library, connection
  status, last catalog scan, and last successful update.
- Credentials and API keys are never displayed.
- A source may expose several independently summarized datasets and resolutions.
- Local files not associated with a registered source are labeled `Uncataloged`, not silently omitted.

### BT-602 — Dataset and market coverage summary

**Status:** Proposed  
**User story:** As a user, I want a concise inventory by market, asset type, and resolution so I can
tell whether I have US common stocks, US ETFs, FX, crypto, or international equities at daily,
five-minute, or one-minute resolution.

**Acceptance criteria:**

- Each summary row identifies provider, country/region, exchange or market, asset class/subtype,
  resolution, session type, symbol count, earliest record, latest record, and last update.
- Examples can distinguish US common stocks from US ETFs and spot FX from crypto spot or perpetuals.
- International markets such as ASX, Shanghai/Shenzhen Stock Connect, or Japanese equities appear
  only when files and catalog metadata actually exist.
- Empty, header-only, malformed, and partially downloaded files are counted separately.

### BT-603 — Searchable instrument coverage drill-down

**Status:** Proposed  
**User story:** As a user, I want to search for an instrument such as IWM and see every dataset and
resolution available for it.

**Acceptance criteria:**

- Search supports canonical symbol, provider symbol, description/name, exchange, and asset class.
- Instrument identity shows provider symbol, venue, timezone, currency, and trading calendar.
- Each available resolution shows first timestamp, last timestamp, row count, expected sessions or
  bars, missing/duplicate/invalid counts, and latest file modification/catalog scan.
- Results distinguish regular-session-only from extended-hours data and raw from adjusted prices.
- Selecting a dataset can show coverage by year without loading an entire large file into the browser.

### BT-604 — Honest aggregate date coverage

**Status:** Proposed  
**User story:** As a user, I want aggregate beginning and ending dates that do not imply every symbol
has the same history.

**Acceptance criteria:**

- Dataset summaries show earliest record anywhere, median instrument start, latest record anywhere,
  and the percentage/count of instruments current through the latest expected session.
- The UI explains that the earliest database record may belong to only one instrument.
- Drill-down exposes symbol-specific date ranges.
- Coverage snapshots are timestamped so a later rescan cannot rewrite what an old run observed.

### BT-605 — Freshness and update health

**Status:** Proposed  
**User story:** As a user, I want to know whether each dataset is current and whether its updater is
working.

**Acceptance criteria:**

- Every dataset shows latest data timestamp, expected latest timestamp based on its calendar and
  update cadence, freshness state, and last successful update time.
- `Current`, `Updating`, `Stale`, `Partial`, `Failed`, and `Unknown` states have explicit definitions.
- The latest job exit state and concise error are visible without treating a configured schedule as a
  successful update.
- Intraday freshness uses timestamps and sessions rather than only a calendar date.

### BT-606 — Data inventory scan and refresh

**Status:** Proposed  
**User story:** As a user, I want to rescan an existing library or request an authorized provider
update and see progress without freezing the application.

**Acceptance criteria:**

- Catalog scans and provider downloads are separate actions with separate progress and audit history.
- Scans are incremental by default and run in a worker process.
- The UI reports files examined, symbols added/changed/removed, malformed files, and resulting
  coverage changes.
- A failed scan preserves the last valid catalog snapshot.

### BT-607 — Move run-specific coverage to Run details

**Status:** Proposed  
**User story:** As a researcher, I want the exact data used by a particular backtest available with
that result without confusing it with my current data inventory.

**Acceptance criteria:**

- The `Completed structured run` selector is removed from the primary Data inventory.
- Each structured Run detail includes a `Data used` section with source, dataset snapshot, resolution,
  resolved instruments/universe, requested date range, actual coverage, and missing symbol-dates.
- Existing coverage artifacts remain immutable and continue to drive partial-data warnings.
- The Data workspace may link to affected runs, but does not use a run as its primary navigation model.

### BT-608 — Data workspace information architecture

**Status:** Proposed  
**User story:** As a user, I want inventory, instrument search, and update operations organized clearly
so broad coverage questions and operational maintenance do not compete on one screen.

**Acceptance criteria:**

- The Data workspace has distinct `Inventory`, `Instrument search`, and `Updates & schedules` views.
- Inventory opens with source cards and a filterable dataset coverage table.
- Instrument search preserves its query and selected instrument when navigating back from another
  workspace.
- Update actions show their source, scope, estimated work when available, and current state.

## Epic G: Columnar market-data storage and external volumes

Parquet is a columnar file format, not a database server. These stories treat partitioned Parquet as
the proposed canonical analytical format behind the data-library interface, with DuckDB/Polars-style
query engines free to consume it. Raw provider files remain a separate, auditable landing layer until
the migration is validated.

### BT-701 — Market-data storage architecture decision

**Status:** Proposed  
**User story:** As an application maintainer, I want a documented raw, curated, and catalog storage
model so format and location choices do not leak into strategy code.

**Acceptance criteria:**

- An ADR defines the raw provider landing layer, canonical analytical layer, catalog/manifest layer,
  ownership boundaries, and retention policy.
- The decision compares partitioned Parquet with the current CSV layout using representative daily,
  one-minute, and five-minute workloads before declaring Parquet canonical.
- Strategies access bars through dataset IDs and the engine data interface, never Parquet paths.
- The decision defines which source files must be retained to reproduce or audit a conversion.

### BT-702 — Versioned canonical bar schema

**Status:** Proposed  
**User story:** As a data-library author, I want a stable versioned schema so files from different
providers and resolutions have consistent meaning.

**Acceptance criteria:**

- The schema defines canonical instrument ID, provider symbol, venue, asset type, timestamp,
  timezone/session semantics, resolution, OHLCV, currency, adjustment state, and source revision.
- Timestamps have an explicit UTC representation while exchange timezone and session date remain
  recoverable.
- Numeric types, null behavior, corporate-action treatment, and duplicate-key rules are explicit.
- Readers reject unsupported schema versions with an actionable error rather than guessing.

### BT-703 — Partition, row-group, and compression benchmark

**Status:** Proposed  
**User story:** As a researcher, I want the physical layout chosen from real query patterns so the
conversion does not replace many CSV files with many tiny Parquet files.

**Acceptance criteria:**

- Candidate layouts are benchmarked for single-symbol scans, multi-symbol cross-sectional scans,
  date-window reads, incremental updates, and full-universe backtests.
- Daily and intraday data may use different partition and row-group strategies.
- Zstandard and at least one lower-CPU compression option are compared for size, read time, and write
  time.
- The selected layout and benchmark hardware/results are recorded and reproducible.

### BT-704 — Configurable data-library roots

**Status:** Proposed  
**User story:** As a user, I want raw, curated, catalog, staging, and artifact locations configured
outside application code so data can live on an internal or external volume.

**Acceptance criteria:**

- Locations are supplied through local configuration or environment variables and are never
  hardcoded in strategies.
- Each registered root has a stable library ID independent of its absolute path.
- Startup checks report writable/read-only/unavailable state and available capacity.
- Moving a library requires an explicit relink operation and does not rewrite immutable old runs.

### BT-705 — Migration inventory and dry-run planner

**Status:** Proposed  
**User story:** As an operator, I want to see exactly what a migration will read, write, skip, and
require before it changes any data.

**Acceptance criteria:**

- The planner inventories files, symbols, resolutions, date ranges, malformed inputs, source bytes,
  and estimated destination/staging capacity.
- A dry run proposes conversion batches and flags unsupported schemas without writing output.
- The plan is immutable, timestamped, and can be compared with the eventual execution audit.
- Insufficient free space blocks execution before a partial conversion begins.

### BT-706 — Resumable CSV-to-Parquet converter

**Status:** Proposed  
**User story:** As an operator, I want conversion to be idempotent and resumable so an interruption
does not require restarting a large data migration.

**Acceptance criteria:**

- Conversion streams bounded batches rather than loading the complete universe into memory.
- Completed outputs are written to staging, validated, and atomically published.
- Rerunning the same source snapshot neither duplicates rows nor rewrites unchanged partitions.
- Progress, errors, retries, source hashes, output files, schema version, and converter version are
  recorded.

### BT-707 — Conversion integrity and backtest parity gate

**Status:** Proposed  
**User story:** As a researcher, I want proof that converted data preserves the strategy evidence
before CSV files are retired.

**Acceptance criteria:**

- Validation compares row counts, keys, date ranges, nulls, duplicate bars, OHLC invariants,
  aggregates, and sampled/full-file hashes where appropriate.
- A representative strategy suite produces identical signals, trades, and returns within explicitly
  documented numeric tolerances on CSV and Parquet readers.
- Adjusted versus raw prices, corporate actions, extended hours, and daylight-saving boundaries have
  dedicated fixtures.
- Any failed parity check blocks source retirement and identifies the affected instruments/partitions.

### BT-708 — Dual-format engine data adapter

**Status:** Proposed  
**User story:** As a strategy author, I want the engine to read CSV or Parquet through one contract so
strategies do not change during the migration.

**Acceptance criteria:**

- The same typed bar request works against both adapters.
- Dataset catalog metadata, not filename inspection in strategy code, selects the adapter.
- Parquet may be preferred with an explicit CSV fallback during transition; every fallback is logged
  and frozen in run provenance.
- Mixed-format datasets cannot silently double-count overlapping bars.

### BT-709 — Parquet dataset manifest and coverage catalog

**Status:** Proposed  
**User story:** As a user, I want fast coverage and provenance queries without scanning every Parquet
file when the Data workspace opens.

**Acceptance criteria:**

- Each published snapshot records partitions, schema version, source snapshot, row counts, byte size,
  minimum/maximum timestamps, symbols, hashes, and publication time.
- Catalog publication is atomic and preserves the last valid snapshot after a failed update.
- Coverage and freshness screens query the catalog first and label its scan timestamp.
- A deeper validation scan can reconcile the manifest with physical files on demand.

### BT-710 — Incremental provider update pipeline

**Status:** Proposed  
**User story:** As an operator, I want nightly downloads to update only affected analytical partitions
so fresh data does not require rebuilding the full library.

**Acceptance criteria:**

- Provider downloads land in a raw staging area before curated data is changed.
- Inserts, corrections, and deletions produce deterministic replacement partitions and a new snapshot.
- Readers see either the old complete snapshot or the new complete snapshot, never half-published data.
- Provider API usage, touched partitions, freshness, and update exit status are auditable.

### BT-711 — Small-file compaction and storage maintenance

**Status:** Proposed  
**User story:** As an operator, I want controlled compaction so incremental updates remain fast without
creating an unbounded number of tiny files.

**Acceptance criteria:**

- Compaction thresholds are based on measured file count/size and query performance.
- Compaction publishes a new immutable dataset snapshot before old parts become eligible for cleanup.
- Orphan, superseded, and temporary files are reported before deletion.
- Active readers and running backtests retain access to the snapshot they started with.

### BT-712 — External-volume identity and fail-closed mount guard

**Status:** Proposed  
**User story:** As a Mac user, I want downloads and backtests to recognize the intended external drive
so an unplugged volume cannot redirect writes to my internal disk.

**Acceptance criteria:**

- A configured external library records volume identity as well as mount path and verifies both before
  reading or writing.
- Missing, renamed, read-only, or unexpectedly substituted volumes fail closed with a clear alert.
- The updater never creates a normal directory at an absent mount point or silently falls back to the
  internal SSD.
- In-progress writes tolerate disconnects by leaving only recoverable staging data and no published
  partial snapshot.

### BT-713 — External-storage performance qualification

**Status:** Proposed  
**User story:** As a user, I want to benchmark a candidate external SSD with real Tessera workloads
before making it the active data volume.

**Acceptance criteria:**

- The test records connection type, negotiated link, filesystem, free space, device model, and thermal
  state where available.
- It measures sequential throughput, small/random reads, single-symbol queries, cross-sectional
  queries, conversion, and at least one representative end-to-end backtest.
- Results are compared with the current internal-drive baseline and saved with the data-library record.
- The product recommends archive-only or active-data use from documented thresholds, not the drive's
  advertised peak speed alone.

### BT-714 — Capacity, drive-health, and disconnect monitoring

**Status:** Proposed  
**User story:** As an operator, I want early warnings about capacity or external-drive problems before
an updater or migration fails.

**Acceptance criteria:**

- Configurable free-space warning and hard-stop thresholds apply before downloads, conversion, and
  compaction.
- Available device-health information, unexpected disconnects, filesystem errors, and repeated I/O
  failures appear in Updates & schedules.
- A failure state does not imply that a configured updater actually completed successfully.
- Recovery instructions identify the affected library and last known-good snapshot.

### BT-715 — Backup, restore, and source-retention policy

**Status:** Proposed  
**User story:** As a user, I want the external data volume treated as primary storage rather than as a
backup so a single drive failure does not destroy irreplaceable data or provenance.

**Acceptance criteria:**

- The policy classifies raw vendor data, curated Parquet, catalogs, configurations, and immutable run
  artifacts by whether they are backed up, reproducible, or re-downloadable.
- At least one independent backup destination and a documented restore procedure exist before source
  CSV retirement.
- A restore drill verifies catalogs, hashes, dataset registration, and a representative backtest.
- Backup exclusions and retention windows are visible and explicit.

### BT-716 — Staged rollout and CSV retirement

**Status:** Proposed  
**User story:** As an operator, I want a reversible rollout by dataset and resolution so a storage
migration cannot break every strategy at once.

**Acceptance criteria:**

- The rollout starts with a small ETF pilot, then daily data, then selected intraday data, before a
  full-universe migration.
- Each stage has size, speed, integrity, and strategy-parity gates plus a documented rollback.
- CSV readers and source files remain available until the corresponding stage is accepted.
- Deletion requires an explicit approved cleanup action and produces an audit record of retired files.

### BT-717 — Append-friendly intraday landing and compaction

**Status:** Proposed  
**User story:** As a data maintainer, I want frequent intraday arrivals staged safely before Parquet
compaction so Parquet is not treated like a row-by-row transactional database.

**Acceptance criteria:**

- New bars first land in an append-safe journal or bounded batch files with provider response metadata.
- A deterministic job deduplicates and compacts completed batches into the canonical layout.
- Late corrections and partially completed sessions can be republished without mutating an already
  frozen run snapshot.
- Crash recovery and replay are tested at each transition between download, staging, and publication.

## Epic H: Event-driven strategy runtime

### BT-801 — Event-driven Strategy SDK

**Status:** Complete  
**User story:** As a strategy author, I want to write signal and order-intent logic once so historical
and future live adapters can execute it without duplicating the strategy.

**Acceptance criteria:**

- The SDK defines causal market events, broker feedback, portfolio snapshots, and order intents.
- Strategy callbacks cannot observe a bar's high, low, close, or volume at that bar's open.
- A minimal moving-average strategy demonstrates the contract without loading data or reporting.

### BT-802 — Engine-owned historical event loop

**Status:** Complete  
**User story:** As a researcher, I want one deterministic replay loop so each strategy does not
reimplement time sequencing.

**Acceptance criteria:**

- Stored sessions replay as session start, bar open, bar close, and session end events.
- Broker processing precedes the strategy callback at each event.
- Symbol ordering and follow-up order processing are deterministic and bounded.

### BT-803 — Simulated and live broker boundaries

**Status:** Complete  
**User story:** As an engine maintainer, I want execution behind an adapter so strategy logic does not
calculate fills or account balances.

**Acceptance criteria:**

- A simulated adapter owns orders, bracket execution, costs, positions, realized equity, and trades.
- A live adapter interface includes connectivity and reconciliation requirements.
- No live brokerage connection or order routing is implied by this story.

### BT-804 — Per-instrument and portfolio strategy scopes

**Status:** Complete  
**User story:** As a strategy author, I want the same SDK to support an independent rule on each
ticker or a synchronized cross-asset portfolio rule.

**Acceptance criteria:**

- Per-instrument scope creates isolated strategy state and filtered events per symbol.
- Portfolio scope receives one synchronized slice and one shared state instance.
- Scope mismatches fail clearly and both modes have deterministic tests.

### BT-805 — Reference event strategy

**Status:** Complete  
**User story:** As a contributor, I want a small example that shows only the code a strategy author
should normally need to write.

**Acceptance criteria:**

- The reference strategy contains parameters, rolling indicator state, signals, and order intents.
- It contains no vendor paths, replay loop, fill model, capital accounting, or report renderer.

### BT-806 — ETF ORB event-engine migration and parity

**Status:** Complete  
**User story:** As a researcher, I want the QQQ ETF ORB migrated without changing its frozen
historical behavior.

**Acceptance criteria:**

- ETF ORB uses the shared historical event loop and simulated broker.
- A deterministic synthetic test compares the event engine with the frozen batch reference.
- The QQQ 2020-01-01 through 2026-08-28 daily, trade, and coverage CSV outputs are byte-identical;
  the comparison records 1,661 covered sessions and 1,557 trades.

### BT-807 — QQQ shadow mode and IBKR paper/live operation

**Status:** Deferred  
**User story:** As an operator, I want to observe and then paper trade the same QQQ ORB logic against
live data before enabling real orders.

**Acceptance criteria:**

- Shadow mode records events, decisions, intended orders, and broker-independent expected state.
- Paper/live operation adds feed health, reconciliation, duplicate-order prevention, kill switches,
  restart recovery, and an operational session screen.
- Live orders remain impossible until the explicit paper and safety gates are accepted.

### BT-808 — Remaining strategy migrations

**Status:** In progress (September 2, 2026): Gap Fade and Limit Buyer ship as one-file SDK strategies (`src/strategies/user/gap_fade.rs`, `limit_buyer.rs`) with parity checks against their legacy runs (Gap Fade 137 vs 134 trades, Limit Buyer 297 vs 297 trades on 2025). Legacy engines stay frozen for existing runs; Overnight Attention, Two-Day Rebound Short, Crypto Daily Trend, and the FX grid are still on the old path.  
**User story:** As a maintainer, I want existing strategies moved onto the common runtime so custom
replay, accounting, and reporting code can be retired.

**Acceptance criteria:**

- Each migration freezes its reference outputs and proves parity or documents intended differences.
- Limit Buyer, Gap Fade, Two-Day Rebound Short, Overnight Attention, Crypto Daily Trend, and FX
  strategies no longer own duplicate runtime plumbing after migration.

## Epic I: Delivery loop quality

### BT-901 — Every code change ships with its proof

**Status:** Proposed
**User story:** As the owner, I want a pull request that changes code to be refused unless it
also changes a test, fixture, or check, so that the "done line has a proof" rule is enforced by
the loop rather than by trust.

**Acceptance criteria:**

- A PR touching a configured code path and no configured proof path, and adding no line
  matching the project's test pattern, fails a required check with the files named.
- A commit-body line `No new test: <reason>` lets the PR through and the reason appears in the
  check's output.
- The rule, the paths, and the pattern come from `.loop.toml`; a project in any language sets
  its own (a Java project's pattern is `@Test`, Python's `def test_`).
- The same script runs in the local check and in CI.

### BT-902 — Test coverage can only go up

**Status:** Proposed
**User story:** As the owner, I want the check to fail when line coverage drops below a
checked-in floor, and the floor to rise with the coverage, so that agents cannot erode the test
suite while adding code.

**Acceptance criteria:**

- The project's coverage command prints one percentage; the kit compares it with the floor
  file and fails below it, naming both numbers.
- When coverage exceeds the floor, the check says so and names the new floor; the ticket that
  raised coverage raises the floor in its commit.
- The ratchet runs in the full local check and in the CI engine job, not in the fast check.
- A missing coverage tool fails the local check with the exact install commands, and CI
  installs the tool itself.
- The mechanism is the same for a Java or Python project; only the command differs.

### BT-903 — Every pull request gets a review before it merges

**Status:** Proposed
**User story:** As the owner, I want an agent to review each PR against its ticket's done line
and the Project rules, and the merge to wait for that review, so that a green build is not the
only thing between an agent's commit and main.

**Acceptance criteria:**

- Each open PR's head commit receives exactly one `Agent review` commit status; a new push
  gets a fresh review.
- The status is red only for: no proof of the done line, the done line not met, a
  Project-rules breach, or a named defect with file and line. Everything else is a comment and
  a green status.
- The review runs from a schedule on the owner's machine with the owner's agent subscription;
  no API key lives on GitHub.
- The ruleset requires the status; auto-merge waits for it.
- The owner can override a red status with one documented command that records a reason.

### BT-904 — Decisions are recorded once and cited everywhere

**Status:** Proposed
**User story:** As the owner, I want every architecture and product decision written down
with its alternatives and reasons, so that agents and I cite one record instead of
re-deciding, and a wrong decision can be superseded without losing why it was made.

**Acceptance criteria:**

- A decision is one file under `docs/decisions/`, numbered, with Context, Decision,
  Alternatives, Consequences, and "what would show this was wrong"; an index lists them.
- A decision is never edited in place: a change is a new record that names the one it
  supersedes, and the old one is marked superseded.
- grill-me reads the index before asking anything, stops and asks when a story implies a
  decision no record covers, writes the record in the session, and cites it in the ticket.
- Tessera's fifteen locked product decisions exist as records; the product backlog points
  at the index.

### BT-905 — A new project is interviewed into rules, decisions, and a check before code

**Status:** Proposed
**User story:** As the owner starting a project, I want one session that asks the questions
that shape everything after (users, data, runtime, deploy, UI, non-negotiables) and leaves
behind the Project rules, the decision records, the first epics, and a check skeleton, so
that `/next-ticket` can run on day one.

**Acceptance criteria:**

- `grill-project` asks in rounds over the five areas and does not draft until each has a
  decision or an explicit deferral with a date.
- It writes `docs/decisions/` records, the Project rules section of `AGENTS.md`, the first
  epics in the product backlog, and `scripts/check.sh` with the decided stack's test, lint,
  format, build, and coverage commands (Rust, Python, Node/TypeScript, Java, Go known; TODO
  lines otherwise).
- Every decision names what would show it was wrong.

### BT-906 — grill-me grills, and shows the screen it is describing

**Status:** Proposed
**User story:** As the owner, I want grill-me to keep asking until every story is testable,
and to sketch any screen it is about to describe, so that tickets do not arrive with an
unprovable done line or a table described in prose.

**Acceptance criteria:**

- At least three rounds; the last is proofs and edge cases only; it cannot draft a story
  without a named proof.
- A story that touches a screen carries a text wireframe in the draft the owner confirms,
  and the ticket points at it; the design canvas is used when the owner asks or the screen
  is new.
- A story that contradicts a recorded decision stops for "which wins" and, if the decision
  changes, a superseding record.

## Epic J: Feature workbench

### BT-1001 — Realized volatility at any lookback as a study feature

**Status:** Proposed
**User story:** As a researcher, I want realized vol over 5-second, 30-second, 60-second, or
any other lookback as a feature expression, so that I can test whether recent vol predicts
future price moves and future vol without leaving the studies page.

**Acceptance criteria:**

- `rv n` (root of the summed squared bar-to-bar returns over the last n bars, in bps) and
  `std n` (rolling standard deviation of any series) are transforms in the feature grammar,
  composable with the existing ones (`mid | rv 5s | std 60s` is vol-of-vol).
- A window written with an `s` suffix on the lake grid is converted from the study's step
  (`rv 30s` is 30 bars at step 1 and 6 bars at step 5); on daily and minute grids a seconds
  window is refused with a message naming the lake grid.
- Every windowed transform accepts the suffix, not only the new ones.
- Seconds without a trade count as zero returns, so `rv` measures vol per unit of time.
- The grammar documentation lists both transforms and the suffix.

### BT-1002 — Future vol as a target in bps, with ready-made vol features

**Status:** Proposed
**User story:** As a researcher, I want to pick "realized vol" as a study target and add the
standard vol features with one click, so that the heat table's numbers for a future-vol study
read in bps and a vol study takes seconds to set up.

**Acceptance criteria:**

- `realized_vol` is a study target: the square root of `realized_variance`, unit bps,
  selectable in the form and on the CLI.
- The feature library holds a seeded vol set on first start: rv 5s, rv 30s, rv 60s, the
  5s/60s vol ratio, and vol-of-vol; they are ordinary library entries afterwards.
- A synthetic panel with persistent vol regimes gives a high IC for trailing vol against
  future vol and an IC near zero against future return.

### BT-1003 — Promoted features are watched for IC decay

**Status:** Proposed
**User story:** As a researcher, I want every promoted feature rescored nightly on the study
it was promoted from and flagged when its IC has halved or flipped sign, so that a feature
that stopped working is caught by the console rather than by a losing month.

**Acceptance criteria:**

- Promotion freezes the study a feature was promoted from (grid, symbols, horizon, target)
  and the IC it had; a feature promoted before this has its baseline set by its first
  nightly run, and the panel says so.
- A nightly job inside the service's scheduler (decision 0001) rescores each promoted
  feature over a trailing window and stores one history row per feature per session.
- A feature is on watch after one run where the trailing IC is under half its baseline or
  has flipped sign, and alerted after two consecutive such runs; a gap in data, a base the
  grid cannot supply, or a horizon longer than the window is a skipped row with a reason,
  never decay.
- Alerts appear in a Feature decay panel on the studies page, directly under the feature
  library, with their numbers, and as a dated line in the private research log; nothing is
  sent outbound (decision 0002). The panel shows alerts, then watches, then skipped rows
  (a base the grid cannot supply, a horizon longer than the window, a data gap, a frozen
  study that no longer exists); ok and baseline rows sit behind a show-all, and alerts and
  watches cap at twenty behind the same show-all. Clicking a row opens the frozen promotion
  study's results; a skipped row for a missing study does not open anything. With nothing
  promoted, or before the job's first run, one line names the next step.

**Wireframe** (the studies page, directly under the feature library; alerts, then watches,
then skipped rows; ok and baseline rows behind show-all; a row click opens the frozen
promotion study's results):

```
+ LIB  FEATURE LIBRARY - 3 accepted - 7 saved ---------------------------------+
| (unchanged)                                                                  |
+------------------------------------------------------------------------------+
+ DCY  FEATURE DECAY - 1 alert - 1 watch - 1 skipped - 2 ok - run 09-11 02:30 -+
| FEATURE           STUDY      BASE IC  TRAIL IC   OBS  STATUS   REASON        |
| mid | rv 30s      vol-5s-A     0.062     0.021   840  ALERT    halved        |
| ret 5s | std 60s  vol-5s-A     0.041    -0.008   840  WATCH    sign flip     |
| mom 60s           px-1s-B          -         -     0  SKIPPED  no base       |
| [show all 5]                                                                 |
+------------------------------------------------------------------------------+
  empty: "No promoted features yet." / "Nightly decay job has not run (02:30,
  disabled)" with a link to the automation page.
```

## Epic K: Console pages

### BT-1101 — A strategy page is a summary, a Run button, and its history; configuration lives in a dialog

**Status:** Proposed
**User story:** As a researcher who launches runs and reviews history equally often, I want
the strategy page to show the current configuration as one strip with a Run button and the
historical runs directly under it, with the full form in a dialog, so that a repeat run and a
look at history both happen without scrolling.

**Acceptance criteria:**

- The page order is: the hero (title, badges, View source, Configure run); Production rules
  folded, closed by default, with the rule count on the bar; a summary strip of the current
  configuration (research label, window, universe, resolution, capital, entry limits, every
  strategy parameter with its value, the preset name) with Configure run and Run buttons;
  the Historical runs table. Nothing else sits between the strip and the table.
- Configure run opens a modal dialog in the terminal look (decision 0003): one form with four
  labelled rows (Identity, Data, Limits, Parameters), presets (apply, save as) inside it, an
  advanced toggle, Cancel and Run. Escape or Cancel closes it and keeps the edits in memory
  until the page changes; Run inside it launches and closes it.
- The Run button on the page sends exactly the configuration the strip shows: the request
  body of a run launched from the strip equals the one launched from the dialog with no
  edits.
- Numeric fields in every form grid are sized to their content (six characters for numbers),
  dates and selects keep their width, and the grid auto-fits so a row holds at least six
  fields at 1280 px; every control keeps its 42 px height and 18 px text (UI-01 stands).
- At 1280 and 1440 px in both display modes the dialog fits the viewport and the page
  behind it does not scroll horizontally; the layout check proves it.

**Wireframe** (the strategy page, then the dialog):

```
+ STR  GAP_FADE v3 - ETF - PRODUCTION            [View source] [Configure run] +
| > Production rules (6)                                                folded |
| RUN  Development - 2024-01-01 -> 2026-09-10 - ETFs - 1m - $100k              |
|      entries 3/day, 10 open, priority - gap 1.5%  hold 30m  stop 2%          |
|      preset: baseline                              [Configure run]  [Run]    |
+------------------------------------------------------------------------------+
+ HST  HISTORICAL RUNS - 42 - starred only [ ] - label v                       +
| * NAME            WINDOW       CAGR  SHARPE  MAXDD  TRADES  LABEL   STARTED  |
| ...                                                                          |
+------------------------------------------------------------------------------+

  CONFIGURE RUN (modal, ~900 px wide)                                      [x]
  IDENTITY  label v    start [      ]  end [      ]   cost profile v
  DATA      universe v  resolution v  session v  size [   ] capital [   ] min px [ ]
  LIMITS    per day [  ]  open [  ]  tie-break v  seed [    ]
  PARAMS    gap % [   ]  hold min [   ]  stop % [   ]  ...        advanced [ ]
  presets: baseline v  [save as...]                          [Cancel]  [Run]
```

### BT-1102 — The strategies catalog is a scoreboard

**Status:** Proposed
**User story:** As a researcher choosing what to work on, I want each strategy's row to show
its last completed run's CAGR, Sharpe, and max drawdown with that run's date, in fewer and
denser columns, so that the catalog answers "how is it doing" without opening each page.

**Acceptance criteria:**

- Columns: #, name (version and status as badges under the name), asset, runs, CAGR, Sharpe,
  max DD, last run (date), open. Each metric column sorts.
- The metrics are the last completed run's cached metrics (`metrics_json`); a strategy with
  no completed run, or whose last run has no cached metrics, shows a dash and no error.
- Rows go dense (15 px cells, 13 px headers) under 1500 px like the run history, and the
  table fits 1280 px without horizontal scroll; the layout check proves it.

**Wireframe:**

```
+ CAT  STRATEGIES - 12 - sort: sharpe v                                        +
| #  NAME                     ASSET  RUNS  CAGR %  SHARPE  MAXDD %  LAST RUN  |
| 1  gap_fade                 ETF      42    18.2    1.31    -9.4   09-10 [>] |
|    v3 - production                                                          |
| 2  limit_buyer              US     117    11.0    0.92   -14.8   09-08 [>] |
|    v2 - research                                                            |
| 3  orb_breakout             ETF       0       -       -       -        - [>]|
|    draft                                                                    |
+------------------------------------------------------------------------------+
```

### BT-1103 — The console's left menu collapses to an icon rail

**Status:** Proposed
**User story:** As a researcher working on a 13-inch screen, I want the left menu to collapse
to a narrow rail of icons so that the dense tables get the width back without losing the
ability to move between pages.

**Acceptance criteria:**

- A control in the sidebar collapses it to a rail and expands it again. The state is the
  user's: nothing auto-collapses, and no viewport width triggers it.
- Collapsed, the sidebar is a fixed 56 px icon rail, the same width in both display modes,
  showing the BT brand mark, the ten nav icons, and the engine-status dot. The Tessera name,
  the Research Console subtitle, every item label, and the foot's two lines of text are not
  rendered.
- Hovering a nav icon shows its label in a themed tooltip; the brand mark and the engine dot
  carry tooltips too.
- The current page stays obvious: the active item keeps its amber colour and its inset bar.
- Collapsing animates the width and the label fade over 150 ms. The expanded sidebar is
  unchanged.
- The choice is remembered per browser; a fresh browser starts expanded. It is independent of
  the display-mode choice.
- At 1280 and 1440 px in both display modes the collapsed page does not scroll horizontally
  and no icon clips.
- The Code page's navigator and the Strategies catalog's inspector rail are unchanged.

**Wireframe:**

```
EXPANDED (unchanged)                    COLLAPSED (new, 56 px, both modes)
+------------------------+              +------+
| [BT] Tessera    [<]    |              |  BT  |  brand mark kept
|      Research Console  |              | [>]  |  toggle
|  ⌂  Dashboard          |              |      |
|  ◆  Strategies         |              |  ⌂   |  label on hover
|  ＋  New Backtest       |              |  ＋   |
|  ▤  Runs               |              |  ▤   |
|  ⇄  Compare            |              |  ⇄   |
|  ◫  Portfolios         |              |  ◫   |
|  ∿  Studies            |              |  ∿   |
|  ▦  Data               |              |  ▦   |
|  ¢  Costs              |              |  ¢   |
| </>  Code              |              | </>  |
|  ●  Engine ready       |              |  ●   |  label on hover
|     Local Mac · 0      |              |      |
+------------------------+              +------+
   224 px (216 terminal)                 56 px; active item keeps its highlight
```

## Recommended delivery milestones

### Completed foundation — Event-driven SDK steps 1–6

- Completed BT-801 through BT-806 on September 1, 2026.
- Kept QQQ shadow/live operation (BT-807) and remaining strategy migrations (BT-808) on the back
  burner as explicit deferred work.

### Milestone 1 — Generic Daily Trend vertical slice

- Implement BT-101 and BT-301 for one strategy package.
- Rename the reusable signal family from Crypto Daily Trend to Daily Trend without changing old run
  identities.
- Implement BT-201, BT-302, BT-401, BT-402, and BT-403 for BTC, ETH, and SOL daily data.
- Freeze strategy, data, instruments, costs, parameters, and exposure controls in the run manifest.

### Milestone 2 — Reusable catalogs and research UI

- Implement BT-102, BT-204, BT-303, BT-404, and BT-501.
- Replace the run-centric Data screen through BT-601, BT-602, BT-603, BT-604, BT-605, BT-607, and
  BT-608; retain provider update controls through BT-606.
- Add saved static universes through BT-202.
- Migrate at least one ETF strategy and one FX strategy to prove the contracts are not crypto-specific.

### Milestone 3 — True external strategy packages

- Implement BT-103 and complete BT-104.
- Move private strategies outside the application checkout.
- Keep only documented sample strategies and small synthetic/fixture datasets in the public project.

### Milestone 4 — Walk-forward analysis

- Implement BT-502 and BT-503 after manifest-driven sweeps and immutable provenance are stable.
- Add point-in-time universes through BT-203 only after the membership data contract is ready.

### Milestone 5 — Columnar storage migration

- Complete BT-701 through BT-703 before choosing the canonical layout.
- Implement BT-704 through BT-710 for a read-compatible, resumable ETF pilot with no source deletion.
- Qualify external storage through BT-712 through BT-715 before relocating the active library.
- Complete BT-711, BT-716, and BT-717 before retiring CSV or moving full intraday universes.
