# Open-Source Architecture Direction

Status: implemented as of September 2026. The repository now contains only the engine, the
one-file strategy SDK, the local UI, two example strategies, and a synthetic example dataset.
Market data is configured through a git-ignored `local.toml` (see `local.example.toml`), private
strategy folders are compiled in through `[strategies] dirs`, and the pre-SDK strategies,
configurations, and research live in a separate private repository. The sections below record the
original direction and remain the reference for the boundaries.

Detailed implementation epics, user stories, acceptance criteria, and delivery milestones are tracked
in [PRODUCT_BACKLOG.md](PRODUCT_BACKLOG.md).

The implemented historical foundation for the shared event model, strategy scopes, broker boundary,
and first ETF ORB migration is documented in
[STRATEGY_SDK_ARCHITECTURE.md](STRATEGY_SDK_ARCHITECTURE.md). Live brokerage connectivity remains a
separate deferred milestone.

## Goal

Make the backtesting application independently open-sourceable. The engine and UI must not own,
compile in, or distribute the user's private strategies or market data. A user should be able to
install the application, point it at one or more external data libraries, install or author strategy
packages, choose explicit instruments or reusable universes, and produce an immutable,
reproducible run.

## Product boundaries

### Backtesting application

The open-source application owns:

- the run queue and isolated worker processes;
- clocks, calendars, event sequencing, portfolio/capital accounting, and cost models;
- the instrument and dataset catalogs;
- immutable run manifests and artifact validation;
- shared reporting, comparisons, sweeps, and portfolio construction;
- the strategy-package development and installation workflow.

Historical and live runtimes should emit the same causal session, bar-open, bar-close, and broker
events. Strategies consume those events and emit broker-neutral order intents; simulated and future
live brokers implement execution outside strategy code.

It must not contain assumptions about a particular strategy's signals or a particular machine's
data paths.

### Strategy packages

A strategy package owns:

- a stable package identity and version;
- its signal, ranking, entry, exit, and strategy-specific sizing rules;
- its typed parameter schema and defaults;
- its required fields, bar frequency, session requirements, and supported instrument capabilities;
- optional strategy-specific audit artifacts.

It emits normalized daily equity, trades, coverage, and metadata through the standard reporting
contract. Private packages should be installable from directories outside the application checkout.

The preferred long-term boundary is a versioned process protocol, not a Rust dynamic-library ABI.
The application launches a package worker with a frozen run request and receives validated artifacts
plus structured progress. This allows independently versioned strategies and, later, strategy SDKs
for more than one language without linking untrusted strategy code into the web service.

### Data libraries

Market data stays outside the application repository. The application stores catalog records and
read-only locations, not copied vendor datasets. A data-library adapter describes:

- dataset identity, provider, revision/snapshot, and physical location;
- instrument identity, venue, asset class, quote currency, timezone, and trading calendar;
- available frequencies, fields, adjusted/raw-price semantics, and date coverage;
- universe membership and, when available, point-in-time membership history;
- quality and missing-session information.

Absolute paths are local configuration and must never be baked into a strategy package.

The Data workspace is a current inventory of registered sources and datasets. It summarizes provider,
market, asset type, resolution, date/timestamp coverage, symbol counts, freshness, and update health,
then supports symbol-level drill-down. A run's immutable `data used` snapshot and signal-session
coverage remain attached to that run rather than serving as the primary Data-page model.

## Instrument and universe selection

Every run should accept an `InstrumentSelection` independent of the strategy code:

1. **Explicit instruments**: one or more catalog instrument IDs, such as BTC-USD, ETH-USD, and
   SOL-USD.
2. **Saved universe**: a named, versioned basket such as `US common stocks`, `US ETFs`, or
   `G10 USD FX pairs`.
3. **Dynamic universe** (later): a point-in-time query such as active US common stocks with minimum
   dollar volume, resolved separately for each session.

Ticker text alone is not a durable identifier. The resolved run should freeze the provider symbol,
venue, asset class, currency, and dataset ID for each instrument. FX selections must resolve to
tradable pairs (for example EUR/USD, USD/CAD, and USD/JPY), not standalone currency names.

## Strategy capabilities

Before a run, the application compares the selected instruments and data with a strategy capability
manifest. Examples include:

- supported asset classes;
- daily, five-minute, or one-minute bars;
- regular-session or extended-hours requirements;
- required OHLCV, corporate-action, quote, funding, or benchmark fields;
- independent per-instrument signals versus cross-sectional ranking;
- long/short and fractional-position support;
- portfolio construction modes supported by the strategy.

The UI may allow an experimental override, but it must clearly record and warn about unsupported or
missing capabilities rather than silently adapting data.

## Portfolio semantics

Instrument selection and capital allocation are separate choices. For a multi-instrument strategy,
the run must explicitly define what happens when zero, one, or many instruments signal. Candidate
modes include equal weight among currently active signals, fixed sleeves per selected instrument,
strategy-defined ranking with a maximum position count, and volatility-weighted allocation. The
resolved mode and all inactive cash must appear in the run manifest.

The initial generic model should separate these controls:

- **Target notional per position** as a percentage of current portfolio equity.
- **Maximum gross exposure** as a multiple of current portfolio equity.
- **Maximum net exposure** when long and short positions can coexist.
- **Maximum simultaneous positions** and optional per-instrument limits.
- **Active-position rescaling**, which determines whether unused capacity is reassigned.

For example, two perpetual-futures instruments may each target 100% of current equity with maximum
gross exposure of 2.0x and active-position rescaling disabled. If both signal, each receives 1.0x
notional and portfolio gross exposure is 2.0x. If only one signals, gross exposure remains 1.0x; the
engine does not silently double that position. Notional exposure is distinct from an exchange's
margin-leverage setting. The first implementation should backtest notional P&L and exposure without
claiming to simulate liquidation or exchange-specific margin rules.

## Parameter and optimization declarations

The strategy package should publish a typed parameter manifest. The browser application must render
forms, presets, sweeps, and optimization controls from this manifest rather than hardcoding a Rust
override structure and a matching UI component for every strategy.

Each parameter declaration should include:

- stable ID, label, description, type, and default value;
- validation range, step, allowed values, units, and cross-parameter constraints;
- UI group, display order, control hint, and `simple`, `advanced`, or `expert` tier;
- whether the value is editable for an ordinary run;
- whether it is eligible for optimization;
- a conservative default optimization domain, which may be narrower than the validation domain;
- whether changing it affects signals, execution, sizing, costs, or research methodology.

An illustrative strategy-package declaration is:

```toml
[[parameters]]
id = "fast_length"
label = "Fast moving average"
type = "integer"
default = 20
units = "sessions"
editable = true
ui_group = "Signal"
ui_tier = "simple"
ui_control = "number"

[parameters.validation]
minimum = 2
maximum = 250
step = 1

[parameters.optimization]
enabled = true
minimum = 5
maximum = 100
step = 5
scale = "linear"

[[parameters]]
id = "average_type"
label = "Moving-average type"
type = "enum"
default = "ema"
allowed_values = ["sma", "ema", "wma"]
editable = true
ui_group = "Signal"
ui_tier = "simple"
ui_control = "select"

[parameters.optimization]
enabled = true
allowed_values = ["sma", "ema"]

[[constraints]]
expression = "fast_length < slow_length"
message = "Fast length must be shorter than slow length."
```

The manifest is presentation and orchestration metadata, not the only validation boundary. The
strategy worker must validate the resolved configuration again before reading data or placing
simulated orders.

`optimizable` means that the parameter may be included in a sweep or walk-forward search; it does
not mean the application automatically optimizes it. A research definition separately chooses the
parameters, search domains, objective, minimum evidence requirements, training window, validation
window, and final holdout policy. Transaction costs, data membership, research dates, and the final
holdout must not become accidental optimization variables.

Walk-forward analysis belongs to the engine. For each fold it creates immutable training child runs,
selects a configuration using only that training window, freezes the selected values, and evaluates
the immediately following out-of-sample window. The aggregate report must show every fold, chosen
parameters, turnover in selected parameters, and the stitched out-of-sample equity curve.

## Immutable run provenance

Every run freezes:

- application/engine version;
- strategy package ID, version, source hash, and package snapshot;
- complete resolved strategy parameters;
- data-library and dataset snapshot IDs;
- requested selection and the resolved point-in-time instrument membership;
- asset metadata, calendars, adjustment rules, costs, dates, and random seeds;
- coverage and missing-data decisions;
- the standard report artifact bundle.

This preserves reproducibility even when strategy packages, catalogs, or source data later change.

## Recommended migration sequence

1. Add catalog-backed explicit instrument selection to Crypto Daily Trend. Keep equal-weight behavior
   explicit while the portfolio-allocation decision is finalized.
2. Add saved, versioned universes and a reusable instrument picker to run forms.
3. Replace strategy-specific absolute data paths with catalog dataset IDs and an engine-provided data
   access interface.
4. Move strategy discovery from hardcoded Rust matches to package manifests and an external package
   directory.
5. Introduce the versioned worker-process protocol, migrate bundled strategies into separately
   installable packages, and keep only sample strategies/fixture data in the public repository.
6. Add point-in-time dynamic universes after membership provenance and delisting handling are defined.

## Near-term non-goals

- Publishing vendor market data with the application.
- Treating today's stock or ETF membership as historically point-in-time without a warning.
- Automatically claiming that a strategy is valid for every asset class merely because OHLCV data
  can be loaded.
- A stable native Rust plugin ABI.
