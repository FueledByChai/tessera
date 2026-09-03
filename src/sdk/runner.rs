//! Generic runner for SDK strategies: any resolution, any symbol list, one code path.
//!
//! The runner loads bars for the chosen resolution and session type, replays a
//! warm-up window before the requested start, hosts one strategy instance per
//! symbol on a shared simulated account, and writes the standard artifact bundle
//! plus the HTML report. Strategy files never touch any of this.
//!
//! Two modes exist. The standard mode loads every symbol's bars up front. The
//! screened-universe mode (manifest `screened_universe()`) first streams every
//! symbol's daily file through the strategy's `screen()` hook and only then loads
//! intraday bars for the symbol-days that were selected, which keeps runs over
//! tens of thousands of symbols memory-bounded.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::America::New_York;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::event_engine::{
    EntryLimits, HistoricalEventEngine, HistoricalSession, MarketBar, Side, SimulatedBroker,
    SimulationCosts, StrategyHost, TieBreak,
};
use crate::report::{generate_report, load_report_view};
use crate::sdk::manifest::Manifest;
use crate::sdk::strategy::{Bar, InstanceSpec, SdkInstance, Shared, SizingPolicy, StrategyEntry};
use crate::strategy::{
    StandardArtifactBundle, StandardCoverageRecord, StandardDailyRecord, StandardRunMetadata,
    StandardTradeRecord, write_standard_artifacts,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Resolution {
    #[serde(rename = "daily")]
    Daily,
    #[serde(rename = "5m")]
    FiveMinute,
    #[serde(rename = "1m")]
    OneMinute,
}

impl Resolution {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::FiveMinute => "5m",
            Self::OneMinute => "1m",
        }
    }
    pub fn parse(value: &str) -> Result<Self> {
        Ok(match value.trim().to_ascii_lowercase().as_str() {
            "daily" | "eod" | "1d" | "d" => Self::Daily,
            "5m" | "five_minute" => Self::FiveMinute,
            "1m" | "one_minute" => Self::OneMinute,
            other => bail!("unsupported resolution {other:?}; use daily, 5m, or 1m"),
        })
    }
    pub fn is_intraday(self) -> bool {
        self != Self::Daily
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Regular,
    Extended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkDataConfig {
    pub resolution: Resolution,
    #[serde(default = "default_session")]
    pub session: SessionKind,
    pub daily_dir: PathBuf,
    #[serde(default)]
    pub five_minute_dir: PathBuf,
    #[serde(default)]
    pub one_minute_dir: PathBuf,
    pub symbols: Vec<String>,
    /// Daily file whose dates define the session calendar for screened runs.
    #[serde(default = "default_calendar_symbol")]
    pub calendar_symbol: String,
}

fn default_session() -> SessionKind {
    SessionKind::Regular
}
fn default_calendar_symbol() -> String {
    "SPY.US".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkSizingConfig {
    #[serde(default = "default_capital")]
    pub initial_capital: f64,
    /// Fraction of equity per `Size::Default` entry, for example `1.0` for fully invested.
    #[serde(default = "default_position_percent")]
    pub position_percent: f64,
    /// Entries are skipped when the reference price is below this. Sub-dollar prices make
    /// fixed-tick slippage and per-share commission meaningless, and a $0.0001 print can
    /// otherwise turn a 5% allocation into tens of millions of shares. Set to 0 to disable.
    #[serde(default = "default_min_price")]
    pub min_price: f64,
}

fn default_capital() -> f64 {
    100_000.0
}
fn default_position_percent() -> f64 {
    1.0
}
fn default_min_price() -> f64 {
    1.0
}

impl Default for SdkSizingConfig {
    fn default() -> Self {
        Self {
            initial_capital: default_capital(),
            position_percent: default_position_percent(),
            min_price: default_min_price(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkCostConfig {
    #[serde(default = "default_tick")]
    pub tick_size: f64,
    #[serde(default)]
    pub entry_slippage_ticks: u32,
    #[serde(default)]
    pub exit_slippage_ticks: u32,
    #[serde(default)]
    pub commission_per_unit_per_fill: f64,
    /// When set, replaces ticks and per-unit commission with a notional charge.
    #[serde(default)]
    pub all_in_round_trip_bps: Option<f64>,
    /// Caps per-unit commission at this percent of a fill's notional (default 1%, the
    /// common broker rule). `None` disables the cap.
    #[serde(default = "default_commission_cap")]
    pub max_commission_percent_of_notional: Option<f64>,
}

fn default_commission_cap() -> Option<f64> {
    Some(1.0)
}

fn default_tick() -> f64 {
    0.01
}

impl Default for SdkCostConfig {
    fn default() -> Self {
        Self {
            tick_size: default_tick(),
            entry_slippage_ticks: 0,
            exit_slippage_ticks: 0,
            commission_per_unit_per_fill: 0.0,
            all_in_round_trip_bps: None,
            max_commission_percent_of_notional: default_commission_cap(),
        }
    }
}

/// Account-wide entry limits enforced by the broker at fill time.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SdkLimitsConfig {
    #[serde(default)]
    pub max_entries_per_day: Option<usize>,
    #[serde(default)]
    pub max_open_positions: Option<usize>,
    /// Buying power as a multiple of total equity. When absent the runner uses the
    /// strategy manifest's default, else `max(1, position_percent x max_open_positions)`.
    #[serde(default)]
    pub max_gross_exposure: Option<f64>,
    /// `priority` (higher `ctx.priority()` first), `random` (seeded), or `alphabetical`.
    #[serde(default = "default_tie_break")]
    pub tie_break: String,
    #[serde(default)]
    pub seed: u64,
}

fn default_tie_break() -> String {
    "priority".to_owned()
}

impl SdkLimitsConfig {
    fn entry_limits(&self) -> Result<EntryLimits> {
        let tie_break = match self.tie_break.trim().to_ascii_lowercase().as_str() {
            "" | "priority" => TieBreak::Priority,
            "random" => TieBreak::Random,
            "alphabetical" => TieBreak::Alphabetical,
            other => {
                bail!("unsupported tie_break {other:?}; use priority, random, or alphabetical")
            }
        };
        Ok(EntryLimits {
            max_entries_per_day: self.max_entries_per_day,
            max_open_positions: self.max_open_positions,
            max_gross_exposure: self.max_gross_exposure,
            tie_break,
            seed: self.seed,
        })
    }
}

/// The frozen configuration of one SDK run. Written verbatim into the run folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdkRunConfig {
    pub strategy: String,
    pub data: SdkDataConfig,
    #[serde(default)]
    pub sizing: SdkSizingConfig,
    #[serde(default)]
    pub costs: SdkCostConfig,
    #[serde(default)]
    pub limits: SdkLimitsConfig,
    #[serde(default)]
    pub parameters: serde_json::Map<String, serde_json::Value>,
}

impl SdkRunConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.strategy.trim().is_empty(),
            "strategy id cannot be empty"
        );
        anyhow::ensure!(
            !self.data.symbols.is_empty(),
            "at least one symbol is required"
        );
        anyhow::ensure!(
            self.sizing.initial_capital.is_finite() && self.sizing.initial_capital > 0.0,
            "initial capital must be positive"
        );
        anyhow::ensure!(
            self.sizing.position_percent.is_finite()
                && self.sizing.position_percent > 0.0
                && self.sizing.min_price.is_finite()
                && self.sizing.min_price >= 0.0,
            "position percent must be positive"
        );
        anyhow::ensure!(self.costs.tick_size > 0.0, "tick size must be positive");
        self.limits.entry_limits()?;
        Ok(())
    }

    fn data_dir(&self) -> &Path {
        match self.data.resolution {
            Resolution::Daily => &self.data.daily_dir,
            Resolution::FiveMinute => &self.data.five_minute_dir,
            Resolution::OneMinute => &self.data.one_minute_dir,
        }
    }

    pub fn symbol_file(&self, symbol: &str) -> PathBuf {
        self.data_dir().join(format!("{symbol}.csv"))
    }

    pub fn daily_file(&self, symbol: &str) -> PathBuf {
        self.data.daily_dir.join(format!("{symbol}.csv"))
    }

    fn simulation_costs(&self) -> SimulationCosts {
        SimulationCosts {
            tick_size: self.costs.tick_size,
            entry_slippage_ticks: self.costs.entry_slippage_ticks,
            exit_slippage_ticks: self.costs.exit_slippage_ticks,
            commission_per_unit_per_fill: self.costs.commission_per_unit_per_fill,
            apply_exit_slippage_to_targets: false,
            all_in_round_trip_bps: self.costs.all_in_round_trip_bps,
            max_commission_percent_of_notional: self.costs.max_commission_percent_of_notional,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawDailyBar {
    #[serde(rename = "Date")]
    date: NaiveDate,
    #[serde(rename = "Open")]
    open: f64,
    #[serde(rename = "High")]
    high: f64,
    #[serde(rename = "Low")]
    low: f64,
    #[serde(rename = "Close")]
    close: f64,
    #[serde(rename = "Adjusted_close")]
    adjusted_close: f64,
    #[serde(rename = "Volume")]
    volume: f64,
}

#[derive(Debug, Deserialize)]
struct RawIntradayBar {
    #[serde(rename = "Timestamp")]
    timestamp: i64,
    #[serde(rename = "Open")]
    open: Option<f64>,
    #[serde(rename = "High")]
    high: Option<f64>,
    #[serde(rename = "Low")]
    low: Option<f64>,
    #[serde(rename = "Close")]
    close: Option<f64>,
    #[serde(rename = "Volume")]
    volume: Option<f64>,
}

fn valid_prices(open: f64, high: f64, low: f64, close: f64) -> bool {
    [open, high, low, close]
        .iter()
        .all(|price| price.is_finite() && *price > 0.0)
        && high >= low
}

/// Split-adjusted daily bars, each carrying its adjusted-to-raw ratio.
/// Writes machine-readable `progress:` lines to stderr, throttled to about one per second,
/// so the UI can show where a long run is. Format: `progress: <stage> <done>/<total> <label>`.
pub struct ProgressReporter {
    stage: &'static str,
    started: std::time::Instant,
    last: std::time::Instant,
    last_percent: usize,
}

impl ProgressReporter {
    pub fn new(stage: &'static str) -> Self {
        let now = std::time::Instant::now();
        Self {
            stage,
            started: now,
            last: now - std::time::Duration::from_secs(60),
            last_percent: usize::MAX,
        }
    }

    pub fn report(&mut self, done: usize, total: usize, label: &str) {
        let percent = if total == 0 { 100 } else { done * 100 / total };
        let now = std::time::Instant::now();
        if percent == self.last_percent && now.duration_since(self.last).as_secs() < 1 {
            return;
        }
        if now.duration_since(self.last).as_millis() < 250 && percent != 100 {
            return;
        }
        self.last = now;
        self.last_percent = percent;
        eprintln!(
            "progress: {} {done}/{total} {label} elapsed={}s",
            self.stage,
            self.started.elapsed().as_secs()
        );
    }

    pub fn finish(&mut self, total: usize) {
        self.last = std::time::Instant::now() - std::time::Duration::from_secs(60);
        self.report(total, total, "done");
    }
}

/// Buying power when the run does not set one: the strategy's declared need, else enough
/// for every allowed position at the configured size, and never below a cash account (1x).
pub fn default_max_gross_exposure(
    declared: Option<f64>,
    position_percent: f64,
    max_open_positions: Option<usize>,
) -> f64 {
    if let Some(value) = declared.filter(|v| v.is_finite() && *v > 0.0) {
        return value;
    }
    let slots = max_open_positions.filter(|n| *n > 0).unwrap_or(1) as f64;
    (position_percent * slots).max(1.0)
}

/// Approximate resident bytes per bar in standard mode: the parsed bar plus its copy in
/// the per-session tables (String key, map node, market bar).
const BYTES_PER_BAR_ESTIMATE: f64 = 120.0;

/// Estimates how many bars the standard loader would hold for `[from, end]` from file sizes
/// and each file's first and last dates, then compares against the memory budget. The
/// budget is half of physical memory unless `TESSERA_MEMORY_BUDGET_GB` says otherwise.
fn check_memory_budget(config: &SdkRunConfig, from: NaiveDate, end: NaiveDate) -> Result<()> {
    let bytes_per_row = match config.data.resolution {
        Resolution::Daily => 60.0,
        Resolution::FiveMinute | Resolution::OneMinute => 84.0,
    };
    let estimated_bars: f64 = config
        .data
        .symbols
        .par_iter()
        .map(|symbol| {
            let path = config.symbol_file(symbol);
            estimate_rows_in_window(&path, bytes_per_row, from, end).unwrap_or(0.0)
        })
        .sum();
    let estimated_bytes = estimated_bars * BYTES_PER_BAR_ESTIMATE;
    let budget = memory_budget_bytes();
    if estimated_bytes > budget {
        bail!(
            "this run needs roughly {:.1} GB for about {:.0} million {} bars across {} symbols, \
             above the {:.1} GB memory budget. Use a shorter window, fewer symbols, daily bars, \
             or a screened-universe strategy that loads intraday data only for candidate days \
             (override the budget with TESSERA_MEMORY_BUDGET_GB).",
            estimated_bytes / 1e9,
            estimated_bars / 1e6,
            config.data.resolution.label(),
            config.data.symbols.len(),
            budget / 1e9
        );
    }
    Ok(())
}

/// Rows of `path` that fall in `[from, end]`, estimated from the file size and the share
/// of the file's date span that overlaps the window. Reads only the head and tail.
fn estimate_rows_in_window(
    path: &Path,
    bytes_per_row: f64,
    from: NaiveDate,
    end: NaiveDate,
) -> Option<f64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len < 64 {
        return Some(0.0);
    }
    let mut head = vec![0u8; len.min(512) as usize];
    file.read_exact(&mut head).ok()?;
    let head = String::from_utf8_lossy(&head);
    let first = head.lines().nth(1).and_then(first_date_in_row)?;
    let tail_len = len.min(512);
    file.seek(SeekFrom::Start(len - tail_len)).ok()?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    let last = tail
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .find_map(first_date_in_row)?;
    if last < first {
        return Some(0.0);
    }
    let span_days = (last - first).num_days().max(1) as f64;
    let overlap_start = from.max(first);
    let overlap_end = end.min(last);
    if overlap_end < overlap_start {
        return Some(0.0);
    }
    let overlap_days = ((overlap_end - overlap_start).num_days() + 1) as f64;
    let rows = len as f64 / bytes_per_row;
    Some(rows * (overlap_days / span_days).min(1.0))
}

/// Date of a CSV row: daily rows start with `YYYY-MM-DD`; intraday rows carry a UTC epoch
/// in the first column.
fn first_date_in_row(line: &str) -> Option<NaiveDate> {
    let first = line.split(',').next()?.trim();
    if let Ok(date) = NaiveDate::parse_from_str(first, "%Y-%m-%d") {
        return Some(date);
    }
    let epoch: i64 = first.parse().ok()?;
    Some(chrono::DateTime::from_timestamp(epoch, 0)?.date_naive())
}

fn memory_budget_bytes() -> f64 {
    if let Some(gb) = env::var("TESSERA_MEMORY_BUDGET_GB")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
    {
        return gb * 1e9;
    }
    physical_memory_bytes().map_or(8e9, |bytes| bytes * 0.5)
}

fn physical_memory_bytes() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;
        return String::from_utf8_lossy(&output.stdout).trim().parse().ok();
    }
    #[cfg(target_os = "linux")]
    {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
        let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
        return Some(kb * 1024.0);
    }
    #[allow(unreachable_code)]
    None
}

/// Calendar days of history to read before `start` so `warmup_bars` completed bars are
/// available: bars per session for the resolution, doubled to cover weekends and holidays.
fn warmup_horizon_days(warmup_bars: usize, resolution: Resolution) -> i64 {
    let bars_per_session = match resolution {
        Resolution::Daily => 1,
        Resolution::FiveMinute => 78,
        Resolution::OneMinute => 390,
    };
    let sessions = warmup_bars.div_ceil(bars_per_session) as i64;
    sessions * 2 + 14
}

pub fn load_daily(path: &Path) -> Result<Vec<Bar>> {
    let mut reader = csv::Reader::from_path(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let market_time = NaiveTime::from_hms_opt(9, 30, 0).expect("valid time");
    let mut bars = Vec::new();
    for raw in reader.deserialize::<RawDailyBar>() {
        let Ok(raw) = raw else {
            continue;
        };
        if !(raw.close.is_finite()
            && raw.close > 0.0
            && raw.adjusted_close.is_finite()
            && raw.adjusted_close > 0.0)
        {
            continue;
        }
        let factor = raw.adjusted_close / raw.close;
        let (open, high, low, close) = (
            raw.open * factor,
            raw.high * factor,
            raw.low * factor,
            raw.adjusted_close,
        );
        if valid_prices(open, high, low, close) {
            bars.push(Bar {
                date: raw.date,
                time: market_time,
                open,
                high,
                low,
                close,
                volume: raw.volume.max(0.0),
                adjustment: factor,
            });
        }
    }
    bars.sort_by_key(|bar| bar.date);
    bars.dedup_by_key(|bar| bar.date);
    Ok(bars)
}

/// Intraday bars in New York time, optionally restricted to the regular session and to
/// a set of dates.
fn load_intraday(
    path: &Path,
    session: SessionKind,
    only_dates: Option<&HashSet<NaiveDate>>,
) -> Result<Vec<Bar>> {
    let mut reader = csv::Reader::from_path(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let regular_open = NaiveTime::from_hms_opt(9, 30, 0).expect("valid time");
    let regular_close = NaiveTime::from_hms_opt(16, 0, 0).expect("valid time");
    let mut bars = Vec::new();
    for raw in reader.deserialize::<RawIntradayBar>() {
        let Ok(raw) = raw else {
            continue;
        };
        let (Some(open), Some(high), Some(low), Some(close)) =
            (raw.open, raw.high, raw.low, raw.close)
        else {
            continue;
        };
        if !valid_prices(open, high, low, close) {
            continue;
        }
        let Some(utc) = Utc.timestamp_opt(raw.timestamp, 0).single() else {
            continue;
        };
        let local: DateTime<chrono_tz::Tz> = utc.with_timezone(&New_York);
        let date = local.date_naive();
        if only_dates.is_some_and(|dates| !dates.contains(&date)) {
            continue;
        }
        let time = local.time();
        if session == SessionKind::Regular && (time < regular_open || time >= regular_close) {
            continue;
        }
        bars.push(Bar {
            date,
            time,
            open,
            high,
            low,
            close,
            volume: raw.volume.unwrap_or(0.0).max(0.0),
            adjustment: 1.0,
        });
    }
    bars.sort_by_key(|bar| (bar.date, bar.time));
    bars.dedup_by_key(|bar| (bar.date, bar.time));
    Ok(bars)
}

fn market_bar(bar: &Bar) -> MarketBar {
    MarketBar {
        open: bar.open,
        high: bar.high,
        low: bar.low,
        close: bar.close,
        volume: bar.volume,
    }
}

/// Typed CLI summary, mirroring the other engines.
#[derive(Debug, Clone, Serialize)]
pub struct SdkRunSummary {
    pub strategy: String,
    pub symbols: usize,
    pub resolution: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub sessions: usize,
    pub trades: usize,
    pub ending_equity: f64,
    pub cagr_percent: f64,
    pub annualized_volatility_percent: f64,
    pub sharpe: Option<f64>,
    pub maximum_drawdown_percent: f64,
    pub output: PathBuf,
}

#[derive(Debug, Serialize)]
struct DailyRow {
    date: NaiveDate,
    equity: f64,
    return_percent: f64,
    fills: usize,
}

#[derive(Debug, Serialize)]
struct TradeRow {
    symbol: String,
    direction: &'static str,
    entry_date: NaiveDate,
    entry_time: NaiveTime,
    exit_date: NaiveDate,
    exit_time: NaiveTime,
    entry_price: f64,
    exit_price: f64,
    quantity: usize,
    gross_pnl: f64,
    commission: f64,
    pnl: f64,
    exit_reason: String,
}

#[derive(Debug, Serialize)]
struct CoverageRow {
    symbol: String,
    date: NaiveDate,
    status: &'static str,
}

fn write_csv<T: Serialize>(path: PathBuf, rows: &[T]) -> Result<()> {
    let mut writer = csv::Writer::from_path(&path)
        .with_context(|| format!("failed to write {}", path.display()))?;
    for row in rows {
        writer.serialize(row)?;
    }
    writer.flush()?;
    Ok(())
}

fn eastern_timestamp(date: NaiveDate, time: NaiveTime) -> String {
    format!("{date}T{}-05:00", time.format("%H:%M:%S"))
}

/// Everything the replay phase needs, regardless of how the sessions were assembled.
struct ReplayPlan {
    sessions: Vec<HistoricalSession>,
    coverage: Vec<CoverageRow>,
    instances: BTreeMap<String, SdkInstance>,
    equity_symbol: String,
    /// Description of the symbol set for the report.
    symbol_note: Vec<String>,
}

fn make_instance(
    entry: &StrategyEntry,
    params: &crate::sdk::manifest::Params,
    id: &'static str,
    symbol: &str,
    index: usize,
    count: usize,
    config: &SdkRunConfig,
    start: NaiveDate,
    daily: Arc<Vec<Bar>>,
    shared: &Arc<Mutex<Shared>>,
    records_equity: bool,
) -> Result<SdkInstance> {
    let inner = (entry.factory)(params, symbol)?;
    Ok(SdkInstance::new(
        InstanceSpec {
            id,
            symbol: symbol.to_owned(),
            symbol_index: index,
            symbol_count: count,
            sizing: SizingPolicy {
                position_percent: config.sizing.position_percent,
                min_price: config.sizing.min_price,
            },
            allows_short: entry.manifest.allows_short,
            live_from: start,
            daily,
            shared: Arc::clone(shared),
            records_equity,
        },
        inner,
    ))
}

/// Standard mode: every symbol's bars in memory, warm-up replayed before `start`.
#[allow(clippy::too_many_arguments)]
fn plan_standard(
    config: &SdkRunConfig,
    entry: &StrategyEntry,
    params: &crate::sdk::manifest::Params,
    id: &'static str,
    start: NaiveDate,
    end: NaiveDate,
    shared: &Arc<Mutex<Shared>>,
) -> Result<ReplayPlan> {
    let manifest = &entry.manifest;
    let mut per_symbol: BTreeMap<String, Vec<Bar>> = BTreeMap::new();
    let mut daily_context: BTreeMap<String, Arc<Vec<Bar>>> = BTreeMap::new();
    let mut warmup_from = start;
    // Files are read in parallel and trimmed to the requested window plus a generous
    // warm-up horizon, so thousands of symbols load quickly without holding full histories.
    let load_from = start
        - chrono::Duration::days(warmup_horizon_days(
            manifest.warmup_bars,
            config.data.resolution,
        ));
    // Refuse runs that cannot fit in memory before reading a single full file. Standard mode
    // holds every selected symbol's bars for the window; a 5-minute replay of all US stocks
    // over several years is hundreds of gigabytes and would only thrash swap.
    check_memory_budget(config, load_from, end)?;
    let load_counter = std::sync::atomic::AtomicUsize::new(0);
    let load_started = std::time::Instant::now();
    let load_last_report = Mutex::new(std::time::Instant::now());
    let total_symbols = config.data.symbols.len();
    let loaded = config
        .data
        .symbols
        .par_iter()
        .map(
            |symbol| -> Result<Option<(String, Vec<Bar>, Option<Arc<Vec<Bar>>>)>> {
                let path = config.symbol_file(symbol);
                anyhow::ensure!(
                    path.is_file(),
                    "{} data is not available for {symbol}",
                    config.data.resolution.label()
                );
                let done = load_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if let Ok(mut last) = load_last_report.try_lock() {
                    if last.elapsed().as_millis() >= 1000 {
                        *last = std::time::Instant::now();
                        eprintln!(
                            "progress: load {done}/{total_symbols} symbols elapsed={}s",
                            load_started.elapsed().as_secs()
                        );
                    }
                }
                let mut bars = match config.data.resolution {
                    Resolution::Daily => load_daily(&path)?,
                    Resolution::FiveMinute | Resolution::OneMinute => {
                        load_intraday(&path, config.data.session, None)?
                    }
                };
                bars.retain(|bar| bar.date >= load_from && bar.date <= end);
                if bars.is_empty() {
                    // Listed after the window or delisted before it: skip rather than abort,
                    // so universe-sized explicit lists run without hand-pruning.
                    eprintln!("warning: skipping {symbol}: no bars in the requested window");
                    return Ok(None);
                }
                let daily = if manifest.daily_context && config.data.resolution.is_intraday() {
                    let daily_path = config.daily_file(symbol);
                    anyhow::ensure!(
                        daily_path.is_file(),
                        "daily data is not available for {symbol} (required for daily context)"
                    );
                    Some(Arc::new(load_daily(&daily_path)?))
                } else {
                    None
                };
                Ok(Some((symbol.clone(), bars, daily)))
            },
        )
        .collect::<Result<Vec<_>>>()?;
    eprintln!(
        "progress: load {}/{} symbols loaded elapsed={}s",
        loaded.iter().filter(|entry| entry.is_some()).count(),
        loaded.len(),
        load_started.elapsed().as_secs()
    );
    let mut active: Vec<String> = Vec::new();
    for (symbol, bars, daily) in loaded.into_iter().flatten() {
        active.push(symbol.clone());
        let first_requested = bars.partition_point(|bar| bar.date < start);
        if manifest.warmup_bars > 0 {
            let seed_start = first_requested.saturating_sub(manifest.warmup_bars);
            if let Some(bar) = bars.get(seed_start) {
                if bar.date < warmup_from {
                    warmup_from = bar.date;
                }
            }
        }
        if let Some(daily) = daily {
            daily_context.insert(symbol.clone(), daily);
        }
        per_symbol.insert(symbol, bars);
    }

    let mut by_date: BTreeMap<NaiveDate, BTreeMap<NaiveTime, BTreeMap<String, MarketBar>>> =
        BTreeMap::new();
    for (symbol, bars) in &per_symbol {
        for bar in bars {
            if bar.date < warmup_from || bar.date > end {
                continue;
            }
            by_date
                .entry(bar.date)
                .or_default()
                .entry(bar.time)
                .or_default()
                .insert(symbol.clone(), market_bar(bar));
        }
    }
    let requested_dates = by_date
        .keys()
        .copied()
        .filter(|date| *date >= start)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !requested_dates.is_empty(),
        "no bars were available in the requested window"
    );
    let mut coverage = Vec::new();
    for date in &requested_dates {
        let times = &by_date[date];
        for symbol in &active {
            let present = times.values().any(|bars| bars.contains_key(symbol));
            coverage.push(CoverageRow {
                symbol: symbol.clone(),
                date: *date,
                status: if present {
                    "covered"
                } else {
                    "missing_session"
                },
            });
        }
    }
    anyhow::ensure!(
        !active.is_empty(),
        "no selected symbol has bars in the requested window"
    );
    let count = active.len();
    // Sessions take ownership of the bar tables (no per-session copies) and share one
    // symbol list.
    let shared_symbols: Arc<[String]> = Arc::from(active.clone());
    let sessions = by_date
        .into_iter()
        .map(|(date, times)| HistoricalSession {
            date,
            symbols: Arc::clone(&shared_symbols),
            bars: times.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    let mut instances = BTreeMap::new();
    for (index, symbol) in active.iter().enumerate() {
        let daily = daily_context
            .remove(symbol)
            .unwrap_or_else(|| Arc::new(Vec::new()));
        instances.insert(
            symbol.clone(),
            make_instance(
                entry,
                params,
                id,
                symbol,
                index,
                count,
                config,
                start,
                daily,
                shared,
                index == 0,
            )?,
        );
    }
    Ok(ReplayPlan {
        sessions,
        coverage,
        instances,
        equity_symbol: active[0].clone(),
        symbol_note: active.clone(),
    })
}

/// Screened mode: daily pass across the whole symbol list, then intraday bars only for the
/// symbol-days the strategy selected.
#[allow(clippy::too_many_arguments)]
fn plan_screened(
    config: &SdkRunConfig,
    entry: &StrategyEntry,
    params: &crate::sdk::manifest::Params,
    id: &'static str,
    start: NaiveDate,
    end: NaiveDate,
    shared: &Arc<Mutex<Shared>>,
) -> Result<ReplayPlan> {
    anyhow::ensure!(
        config.data.resolution.is_intraday(),
        "screened universe runs need an intraday resolution (5m or 1m); the daily bars are the screen"
    );
    let calendar_path = config.daily_file(&config.data.calendar_symbol);
    anyhow::ensure!(
        calendar_path.is_file(),
        "calendar symbol {} has no daily file",
        config.data.calendar_symbol
    );
    let calendar = load_daily(&calendar_path)?
        .into_iter()
        .map(|bar| bar.date)
        .collect::<Vec<_>>();
    let next_session: BTreeMap<NaiveDate, NaiveDate> =
        calendar.windows(2).map(|pair| (pair[0], pair[1])).collect();
    let session_dates = calendar
        .iter()
        .copied()
        .filter(|date| *date >= start && *date <= end)
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !session_dates.is_empty(),
        "no calendar sessions in the window"
    );

    // Phase 1: daily screen across every symbol, in parallel.
    let count = config.data.symbols.len();
    let screened: Vec<Result<Option<(String, SdkInstance, Vec<NaiveDate>)>>> = config
        .data
        .symbols
        .par_iter()
        .enumerate()
        .map(|(index, symbol)| {
            let path = config.daily_file(symbol);
            if !path.is_file() {
                return Ok(None);
            }
            let daily = load_daily(&path)?;
            if daily.is_empty() {
                return Ok(None);
            }
            let mut instance = make_instance(
                entry,
                params,
                id,
                symbol,
                index,
                count,
                config,
                start,
                Arc::new(Vec::new()),
                shared,
                false,
            )?;
            let mut selected = Vec::new();
            for bar in &daily {
                if bar.date > end {
                    break;
                }
                let next = next_session.get(&bar.date).copied();
                if instance.screen(bar, next)? {
                    if let Some(next) = next {
                        if next >= start && next <= end {
                            selected.push(next);
                        }
                    }
                }
            }
            Ok(Some((symbol.clone(), instance, selected)))
        })
        .collect();
    let mut instances = BTreeMap::new();
    let mut candidates: BTreeMap<String, Vec<NaiveDate>> = BTreeMap::new();
    let mut screened_symbols = 0usize;
    for result in screened {
        let Some((symbol, instance, selected)) = result? else {
            continue;
        };
        screened_symbols += 1;
        if selected.is_empty() {
            continue;
        }
        candidates.insert(symbol.clone(), selected);
        instances.insert(symbol, instance);
    }
    anyhow::ensure!(
        !candidates.is_empty(),
        "the daily screen selected no symbol-days in the requested window"
    );

    // Phase 2: intraday bars for candidate symbol-days only, in parallel.
    let loaded: Vec<Result<(String, Option<Vec<Bar>>)>> = candidates
        .par_iter()
        .map(|(symbol, dates)| {
            let path = config.symbol_file(symbol);
            if !path.is_file() {
                return Ok((symbol.clone(), None));
            }
            let wanted: HashSet<NaiveDate> = dates.iter().copied().collect();
            let bars = load_intraday(&path, config.data.session, Some(&wanted))?;
            Ok((symbol.clone(), Some(bars)))
        })
        .collect();
    let mut by_date: BTreeMap<NaiveDate, BTreeMap<NaiveTime, BTreeMap<String, MarketBar>>> =
        BTreeMap::new();
    let mut has_file: HashSet<String> = HashSet::new();
    let mut covered: HashSet<(NaiveDate, String)> = HashSet::new();
    for result in loaded {
        let (symbol, bars) = result?;
        let Some(bars) = bars else {
            continue;
        };
        has_file.insert(symbol.clone());
        for bar in bars {
            covered.insert((bar.date, symbol.clone()));
            by_date
                .entry(bar.date)
                .or_default()
                .entry(bar.time)
                .or_default()
                .insert(symbol.clone(), market_bar(&bar));
        }
    }
    let mut coverage = Vec::new();
    for (symbol, dates) in &candidates {
        for date in dates {
            coverage.push(CoverageRow {
                symbol: symbol.clone(),
                date: *date,
                status: if covered.contains(&(*date, symbol.clone())) {
                    "covered"
                } else if has_file.contains(symbol) {
                    "missing_session"
                } else {
                    "missing_file"
                },
            });
        }
    }
    let equity_symbol = instances
        .keys()
        .next()
        .cloned()
        .expect("at least one candidate instance");
    // Only the recorder needs session events every day; candidates get them on their days.
    let mut symbols_by_date: BTreeMap<NaiveDate, BTreeSet<String>> = BTreeMap::new();
    for (symbol, dates) in &candidates {
        for date in dates {
            symbols_by_date
                .entry(*date)
                .or_default()
                .insert(symbol.clone());
        }
    }
    let sessions = session_dates
        .iter()
        .map(|date| {
            let mut symbols = symbols_by_date.remove(date).unwrap_or_default();
            symbols.insert(equity_symbol.clone());
            HistoricalSession {
                date: *date,
                symbols: Arc::from(symbols.into_iter().collect::<Vec<_>>()),
                bars: by_date
                    .remove(date)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            }
        })
        .collect::<Vec<_>>();
    let mut plan = ReplayPlan {
        sessions,
        coverage,
        instances,
        equity_symbol: equity_symbol.clone(),
        symbol_note: vec![format!(
            "{screened_symbols} symbols screened · {} candidates traded intraday",
            candidates.len()
        )],
    };
    // Re-create the recorder instance flag on the chosen symbol.
    if let Some(instance) = plan.instances.remove(&equity_symbol) {
        let mut recorder = instance;
        recorder.set_records_equity(true);
        plan.instances.insert(equity_symbol, recorder);
    }
    Ok(plan)
}

/// Run one SDK strategy over the requested window and write the standard bundle.
pub fn run(
    config: &SdkRunConfig,
    entry: &StrategyEntry,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<SdkRunSummary> {
    config.validate()?;
    anyhow::ensure!(start <= end, "start must not be after end");
    let manifest: &Manifest = &entry.manifest;
    manifest.validate()?;
    let params = manifest.resolve(&config.parameters)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let id: &'static str = Box::leak(manifest.id.clone().into_boxed_str());
    let shared: Arc<Mutex<Shared>> = Arc::new(Mutex::new(Shared::default()));

    let plan = if manifest.screen_universe {
        plan_screened(config, entry, &params, id, start, end, &shared)?
    } else {
        plan_standard(config, entry, &params, id, start, end, &shared)?
    };
    let ReplayPlan {
        sessions,
        coverage,
        instances,
        equity_symbol,
        symbol_note,
    } = plan;

    let mut host = StrategyHost::per_instrument(instances)?;
    // Run-form limits win; otherwise the manifest's defaults apply.
    let mut effective_limits = config.limits.clone();
    if effective_limits.max_entries_per_day.is_none() {
        effective_limits.max_entries_per_day = manifest.default_max_entries_per_day;
    }
    if config.limits.tie_break.trim().is_empty() || config.limits.tie_break == "priority" {
        if let Some(tie_break) = &manifest.default_tie_break {
            effective_limits.tie_break = tie_break.clone();
            if effective_limits.seed == 0 {
                effective_limits.seed = manifest.default_seed;
            }
        }
    }
    if effective_limits.max_gross_exposure.is_none() {
        effective_limits.max_gross_exposure = Some(default_max_gross_exposure(
            manifest.default_max_gross_exposure,
            config.sizing.position_percent,
            effective_limits.max_open_positions,
        ));
    }
    let mut broker =
        SimulatedBroker::new(config.sizing.initial_capital, config.simulation_costs())?
            .with_limits(effective_limits.entry_limits()?);
    let mut reporter = ProgressReporter::new("replay");
    let session_count = sessions.len();
    HistoricalEventEngine::run_owned(
        &mut host,
        &mut broker,
        sessions,
        &mut |done, total, date| {
            reporter.report(done, total, &date.to_string());
        },
    )?;
    reporter.finish(session_count);

    // Daily equity is account-wide; fills are summed across symbols.
    let equity_curve = host
        .strategy(&equity_symbol)
        .context("equity recorder instance disappeared from its host")?
        .daily_equity
        .clone();
    let mut fills_by_day: BTreeMap<NaiveDate, usize> = BTreeMap::new();
    for instance in host.instances() {
        for (date, fills) in &instance.fills_by_day {
            *fills_by_day.entry(*date).or_default() += fills;
        }
    }
    let mut previous = config.sizing.initial_capital;
    let daily = equity_curve
        .iter()
        .map(|(date, equity)| {
            let row = DailyRow {
                date: *date,
                equity: *equity,
                return_percent: if previous > 0.0 {
                    (equity / previous - 1.0) * 100.0
                } else {
                    0.0
                },
                fills: fills_by_day.get(date).copied().unwrap_or(0),
            };
            previous = *equity;
            row
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        !daily.is_empty(),
        "no sessions were replayed in the requested window"
    );

    let trades = broker
        .completed_trades()
        .iter()
        .map(|trade| TradeRow {
            symbol: trade.symbol.clone(),
            direction: match trade.side {
                Side::Buy => "long",
                Side::Sell => "short",
            },
            entry_date: trade.entry_date,
            entry_time: trade.entry_time,
            exit_date: trade.exit_date,
            exit_time: trade.exit_time,
            entry_price: trade.entry_price,
            exit_price: trade.exit_price,
            quantity: trade.quantity,
            gross_pnl: trade.gross_pnl,
            commission: trade.commission,
            pnl: trade.pnl,
            exit_reason: trade.exit_reason.clone(),
        })
        .collect::<Vec<_>>();

    write_csv(output_dir.join("daily_returns.csv"), &daily)?;
    write_csv(output_dir.join("trades.csv"), &trades)?;
    // coverage.parquet is the record of truth; the CSV twin is a convenience that would run
    // to hundreds of megabytes for universe-sized symbol lists, so it is skipped past a cap.
    const COVERAGE_CSV_ROW_CAP: usize = 2_000_000;
    if coverage.len() <= COVERAGE_CSV_ROW_CAP {
        write_csv(output_dir.join("coverage.csv"), &coverage)?;
    } else {
        eprintln!(
            "coverage.csv skipped ({} rows exceed the {COVERAGE_CSV_ROW_CAP} row cap); coverage.parquet holds the full table",
            coverage.len()
        );
    }

    let mut parameters = params.display_map();
    parameters.insert(
        "Resolution".to_owned(),
        format!(
            "{} bars · {} session",
            config.data.resolution.label(),
            match config.data.session {
                SessionKind::Regular => "regular",
                SessionKind::Extended => "extended",
            }
        ),
    );
    parameters.insert(
        "Position size".to_owned(),
        format!(
            "{:.0}% of equity per entry",
            config.sizing.position_percent * 100.0
        ),
    );
    parameters.insert(
        "Costs".to_owned(),
        match config.costs.all_in_round_trip_bps {
            Some(bps) => format!("{bps:.1} bps all-in round trip"),
            None => format!(
                "{} entry ticks + {} exit ticks; ${:.4}/unit/fill",
                config.costs.entry_slippage_ticks,
                config.costs.exit_slippage_ticks,
                config.costs.commission_per_unit_per_fill
            ),
        },
    );
    if config.limits.max_entries_per_day.is_some() || config.limits.max_open_positions.is_some() {
        parameters.insert(
            "Entry limits".to_owned(),
            format!(
                "{} entries/day · {} open positions · {} tie-break",
                config
                    .limits
                    .max_entries_per_day
                    .map_or("unlimited".to_owned(), |v| v.to_string()),
                config
                    .limits
                    .max_open_positions
                    .map_or("unlimited".to_owned(), |v| v.to_string()),
                config.limits.tie_break
            ),
        );
    }
    let metadata = StandardRunMetadata {
        strategy_name: format!("{} {}", manifest.name, manifest.version),
        resolution: format!("{} bars", config.data.resolution.label()),
        initial_capital: config.sizing.initial_capital,
        currency: "USD".to_owned(),
        start,
        end,
        symbols: symbol_note,
        parameters,
        annualization_periods: 252.0,
    };
    let standard_daily = daily
        .iter()
        .map(|row| StandardDailyRecord {
            date: row.date,
            ending_equity: row.equity,
            fills: row.fills,
        })
        .collect::<Vec<_>>();
    let standard_trades = broker
        .completed_trades()
        .iter()
        .map(|trade| {
            let notional = trade.entry_price * trade.quantity as f64;
            StandardTradeRecord {
                symbol: trade.symbol.clone(),
                direction: match trade.side {
                    Side::Buy => "long".to_owned(),
                    Side::Sell => "short".to_owned(),
                },
                trade_date: trade.entry_date,
                entry_time: eastern_timestamp(trade.entry_date, trade.entry_time),
                exit_time: eastern_timestamp(trade.exit_date, trade.exit_time),
                pnl: trade.pnl,
                return_percent: if notional > 0.0 {
                    trade.pnl / notional * 100.0
                } else {
                    0.0
                },
                leverage: (trade.equity_at_entry > 0.0).then(|| notional / trade.equity_at_entry),
                entry_price: Some(trade.entry_price),
                exit_price: Some(trade.exit_price),
                quantity: Some(trade.quantity as f64),
            }
        })
        .collect::<Vec<_>>();
    let standard_coverage = coverage
        .iter()
        .map(|row| StandardCoverageRecord {
            trade_date: row.date,
            symbol: row.symbol.clone(),
            status: row.status.to_owned(),
        })
        .collect::<Vec<_>>();
    write_standard_artifacts(
        output_dir,
        &StandardArtifactBundle {
            metadata: &metadata,
            daily: &standard_daily,
            trades: &standard_trades,
            coverage: &standard_coverage,
        },
    )?;
    fs::write(
        output_dir.join("strategy_config.toml"),
        toml::to_string_pretty(config)?,
    )?;
    fs::write(
        output_dir.join("strategy_manifest.json"),
        serde_json::to_vec_pretty(manifest)?,
    )?;
    generate_report(output_dir, Some(&output_dir.join("report.html")))?;
    let report = load_report_view(output_dir)?;
    let summary = SdkRunSummary {
        strategy: manifest.id.clone(),
        symbols: config.data.symbols.len(),
        resolution: config.data.resolution.label().to_owned(),
        start,
        end,
        sessions: daily.len(),
        trades: report.trades.len(),
        ending_equity: report.metrics.ending_equity,
        cagr_percent: report.metrics.cagr_percent,
        annualized_volatility_percent: report.metrics.annual_volatility_percent,
        sharpe: report.metrics.sharpe,
        maximum_drawdown_percent: report.metrics.max_drawdown_percent,
        output: output_dir.join("report.html"),
    };
    fs::write(
        output_dir.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    Ok(summary)
}

pub fn print_summary(summary: &SdkRunSummary) {
    println!(
        "{} on {} symbols ({} bars) {} → {}",
        summary.strategy, summary.symbols, summary.resolution, summary.start, summary.end
    );
    println!("sessions: {}  trades: {}", summary.sessions, summary.trades);
    println!(
        "ending equity: {:.2}  CAGR: {:.2}%  vol: {:.2}%  sharpe: {}  max DD: {:.2}%",
        summary.ending_equity,
        summary.cagr_percent,
        summary.annualized_volatility_percent,
        summary
            .sharpe
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "—".to_owned()),
        summary.maximum_drawdown_percent
    );
    println!("report: {}", summary.output.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_default_covers_the_allowed_positions_but_never_below_cash() {
        // Ten positions at 10% each: a cash account.
        assert_eq!(default_max_gross_exposure(None, 0.1, Some(10)), 1.0);
        // One position at 100% with unlimited slots still means 1x.
        assert_eq!(default_max_gross_exposure(None, 1.0, None), 1.0);
        // Five positions at 50% each is a deliberate 2.5x.
        assert_eq!(default_max_gross_exposure(None, 0.5, Some(5)), 2.5);
        // A strategy that declares its leverage wins.
        assert_eq!(default_max_gross_exposure(Some(10.0), 0.1, Some(10)), 10.0);
    }
}
