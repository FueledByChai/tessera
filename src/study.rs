//! Feature studies: how a bar-level feature relates to forward returns at several horizons.
//!
//! This is the research step before a strategy. For each symbol the study reads a panel of
//! bars through the SDK loaders (daily, 5-minute, and 1-minute CSV bars) or builds
//! `step_secs` bars from the tick lake, computes the requested features at each bar close,
//! measures a forward target `h` bars ahead (the mid return by default; see [`Target`] for
//! realized variance, absolute move, spread change, and fair-value residuals), then reports
//! per feature and horizon:
//! Spearman rank correlation (information coefficient), the mean forward return by feature
//! decile, a t-statistic for top-minus-bottom decile, and a costless trading curve: the
//! feature's z-score (clipped, or just its sign) as the position, times the forward return,
//! with no costs. Its Sharpe, turnover, and breakeven cost say what the edge is worth before
//! any strategy is written. `decision_delay_bars` shifts every feature by that many bars
//! before measuring, which models the time between observing the book and being able to act
//! on it.
//!
//! Features are expressions (see [`crate::feature_expr`]): a base series such as `obi_l1`
//! or `trade_count`, optionally followed by streaming transforms, e.g.
//! `signed_volume | zscore 30` or `trade_count | rate 1 | ratio_to sma 300`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::feature_expr::{self, BarInput, Evaluator};
use crate::lake::{self, LakeSymbol};
use crate::sdk::runner::{SessionKind, load_daily, load_intraday, sanitize_daily};
use crate::sdk::strategy::Bar;
use crate::series::{Series, SeriesKind, SeriesSpec};

/// The default feature set: the plain base series a study runs when none are given.
pub const FEATURES: &[&str] = &[
    "obi_l1",
    "obi_l5",
    "obi_l10",
    "microprice_bps",
    "spread_bps",
    "trade_imbalance",
    "return_1",
    "signed_volume",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyConfig {
    /// Parquet tick lake, needed for the lake grid.
    #[serde(default)]
    pub lake_dir: PathBuf,
    /// `EXCHANGE:SYMBOL` lake instruments, or CSV symbols such as `SPY.US` on a CSV grid.
    pub symbols: Vec<String>,
    /// Sampling grid in seconds on the lake (also the unit of horizons and delay there).
    #[serde(default = "default_step")]
    pub step_secs: u32,
    /// Bar grid: `daily`, `5m`, or `1m` from the CSV library; absent means tick-built lake
    /// bars on `step_secs`. See [`Grid`].
    #[serde(default)]
    pub resolution: Option<String>,
    #[serde(default)]
    pub daily_dir: PathBuf,
    #[serde(default)]
    pub five_minute_dir: PathBuf,
    #[serde(default)]
    pub one_minute_dir: PathBuf,
    /// Daily file whose dates sanitize a daily panel (drops holiday rows); absent skips it.
    #[serde(default)]
    pub calendar_symbol: Option<String>,
    /// Regular or extended session for intraday CSV bars.
    #[serde(default = "default_session")]
    pub session: SessionKind,
    #[serde(default = "default_features")]
    pub features: Vec<String>,
    /// Forward horizons in bars.
    #[serde(default = "default_horizons")]
    pub horizons: Vec<usize>,
    /// Bars between observing a feature and acting on it (0 = same bar close).
    #[serde(default)]
    pub decision_delay_bars: usize,
    #[serde(default = "default_buckets")]
    pub buckets: usize,
    /// What every feature is scored against over the horizon (default `return`).
    #[serde(default)]
    pub target: Target,
    /// Exogenous series usable as bases, joined as-of availability (`[[data.series]]` in
    /// `local.toml`, or inline here). Relative paths resolve against the working directory.
    #[serde(default)]
    pub series: Vec<SeriesSpec>,
    /// On the lake grid, register funding and open interest for every symbol (default on).
    #[serde(default = "default_true")]
    pub lake_series: bool,
    /// `time_series` (default): each symbol's feature against its own forward target, plus a
    /// pooled cell. `cross_sectional`: per date, the feature ranked across symbols against
    /// their targets, IC per date, and a long-short decile portfolio. See [`StudyMode`].
    #[serde(default)]
    pub mode: StudyMode,
    /// On the daily grid, the intraday CSV grid (`1m` or `5m`) that `agg daily ...` features
    /// are computed on before lifting each day's final value onto the daily bar. Absent means
    /// such features are reported unavailable on the daily grid.
    #[serde(default)]
    pub intraday_source: Option<String>,
    /// Bars before and after an event in the event-study path (default 20).
    #[serde(default = "default_event_window")]
    pub event_window: usize,
    /// The accepted feature set: expressions regressed out of every studied feature before
    /// its incremental IC is scored (see [`Diagnostics`]).
    #[serde(default)]
    pub accepted: Vec<String>,
}

/// Bars of trailing realized variance that define the volatility regime.
const VOL_WINDOW: usize = 60;
/// Observations a day needs before it gets its own IC.
const MIN_DAY_OBS: usize = 30;

fn default_event_window() -> usize {
    20
}

/// How a study reads the panel: along time within each symbol, or across symbols per date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudyMode {
    #[default]
    TimeSeries,
    CrossSectional,
}

impl StudyMode {
    pub fn name(self) -> &'static str {
        match self {
            StudyMode::TimeSeries => "time_series",
            StudyMode::CrossSectional => "cross_sectional",
        }
    }
    pub fn parse(text: &str) -> Result<StudyMode> {
        match text.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "" | "time_series" | "timeseries" | "ts" => Ok(StudyMode::TimeSeries),
            "cross_sectional" | "crosssectional" | "cs" | "xs" => Ok(StudyMode::CrossSectional),
            other => bail!("unknown study mode {other:?}; use time_series or cross_sectional"),
        }
    }
}

fn default_true() -> bool {
    true
}

/// The side feeds every lake instrument registers as series when `lake_series` is on: the
/// base name, the lake feed, and its column. Availability is the receive timestamp.
pub const LAKE_SERIES: [(&str, &str, &str); 4] = [
    ("funding_rate", "funding", "fundingRate"),
    ("funding_annualized", "funding", "annualizedRate"),
    ("open_interest", "open_interest", "openInterest"),
    ("open_interest_usd", "open_interest", "openInterestUsd"),
];

/// Funding and open interest for the lake symbols, one series per name, as-of receipt.
pub fn lake_series(
    lake_dir: &Path,
    symbols: &[String],
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<Series>> {
    let mut out = Vec::new();
    for (name, feed, column) in LAKE_SERIES {
        let mut observations = Vec::new();
        for symbol in symbols {
            let Some(sym) = LakeSymbol::parse(symbol) else {
                continue;
            };
            for value in lake::read_feed_values(lake_dir, &sym, feed, column, start, end)? {
                observations.push(crate::series::Observation {
                    nominal_us: value.event_us,
                    available_us: value.recv_us,
                    symbol: Some(symbol.clone()),
                    value: value.value,
                });
            }
        }
        if !observations.is_empty() {
            out.push(Series::new(name, SeriesKind::Level, observations));
        }
    }
    Ok(out)
}

/// The instant a bar closes, microseconds UTC: lake bars are stamped at their start in UTC;
/// CSV bars carry New York wall time (daily bars close at 16:00).
pub fn bar_close_us(bar: &lake::LakeBar, grid: Grid) -> i64 {
    use chrono::TimeZone;
    match grid {
        Grid::Lake { step_secs } => {
            bar.date.and_time(bar.time).and_utc().timestamp_micros()
                + i64::from(step_secs) * 1_000_000
        }
        Grid::Daily => {
            let close = bar.date.and_hms_opt(16, 0, 0).expect("valid time");
            chrono_tz::America::New_York
                .from_local_datetime(&close)
                .single()
                .map(|t| t.timestamp_micros())
                .unwrap_or_else(|| close.and_utc().timestamp_micros())
        }
        Grid::FiveMinute | Grid::OneMinute => {
            let start = bar.date.and_time(bar.time);
            chrono_tz::America::New_York
                .from_local_datetime(&start)
                .single()
                .map(|t| t.timestamp_micros())
                .unwrap_or_else(|| start.and_utc().timestamp_micros())
                + i64::from(grid.step_secs()) * 1_000_000
        }
    }
}

/// Every registered series aligned to one symbol's bars, series-major, as the evaluator
/// reads them.
pub fn align_series(
    grid: Grid,
    symbol: &str,
    bars: &[lake::LakeBar],
    series: &[Series],
) -> Vec<Vec<f64>> {
    let closes: Vec<i64> = bars.iter().map(|bar| bar_close_us(bar, grid)).collect();
    series.iter().map(|s| s.align(symbol, &closes)).collect()
}

/// The forward quantity a feature is scored against: IC, deciles, and the costless curve all
/// run on it. Values are per acting bar `a` and horizon `h`, in the unit [`Target::unit`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Mid-price return from `a` to `a + h`, bps.
    #[default]
    Return,
    /// Sum of squared bar-to-bar mid returns (bps) over `a + 1 ..= a + h`, bps squared.
    RealizedVariance,
    /// Absolute mid-price return from `a` to `a + h`, bps.
    AbsMove,
    /// Quoted spread at `a + h` minus the spread at `a`, bps.
    SpreadChange,
    /// Mid minus its 60-second EMA at `a + h`, in bps of mid: where the price sits against a
    /// slow fair value once the horizon has passed.
    FairValueResidual,
    /// Mid minus microprice at `a + h`, in bps of mid: where the price sits against the
    /// size-weighted touch.
    MicropriceResidual,
}

impl Target {
    pub const ALL: [Target; 6] = [
        Target::Return,
        Target::RealizedVariance,
        Target::AbsMove,
        Target::SpreadChange,
        Target::FairValueResidual,
        Target::MicropriceResidual,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Target::Return => "return",
            Target::RealizedVariance => "realized_variance",
            Target::AbsMove => "abs_move",
            Target::SpreadChange => "spread_change",
            Target::FairValueResidual => "fair_value_residual",
            Target::MicropriceResidual => "microprice_residual",
        }
    }
    /// The unit every target-valued figure carries (decile means, top-bottom, P&L).
    pub fn unit(self) -> &'static str {
        match self {
            Target::RealizedVariance => "bps²",
            _ => "bps",
        }
    }
    pub fn parse(text: &str) -> Result<Target> {
        let wanted = text.trim().to_ascii_lowercase();
        Target::ALL
            .into_iter()
            .find(|t| t.name() == wanted)
            .with_context(|| {
                format!(
                    "unknown study target {text:?}; use one of {}",
                    Target::ALL.map(Target::name).join(", ")
                )
            })
    }
}

/// Seconds of mid history the fair-value EMA averages over.
const FAIR_VALUE_EMA_SECS: u32 = 60;

/// The per-bar book series targets are computed from (`NaN` where the bar has no book).
#[derive(Debug, Clone, Default)]
pub struct TargetSeries {
    pub mids: Vec<f64>,
    pub spreads: Vec<f64>,
    pub microprices: Vec<f64>,
    /// EMA of the mid over [`FAIR_VALUE_EMA_SECS`], carried across bars without a book.
    pub fair_values: Vec<f64>,
}

impl TargetSeries {
    /// Mid, spread, and microprice per bar plus the fair-value EMA. On a grid without an
    /// order book the close stands in for the mid; spread and microprice stay `NaN`.
    pub fn from_bars(bars: &[lake::LakeBar], grid: Grid) -> Self {
        let read = |f: fn(&crate::lake::BookFeatures) -> f64| -> Vec<f64> {
            bars.iter()
                .map(|bar| bar.book.as_ref().map(f).unwrap_or(f64::NAN))
                .collect()
        };
        let mids = if grid.has_book() {
            read(|b| b.mid)
        } else {
            bars.iter().map(|bar| bar.close).collect()
        };
        let window = grid.fair_value_window_bars() as f64;
        let alpha = 2.0 / (window + 1.0);
        let mut ema = f64::NAN;
        let fair_values = mids
            .iter()
            .map(|&mid| {
                if mid.is_finite() {
                    ema = if ema.is_finite() {
                        ema + alpha * (mid - ema)
                    } else {
                        mid
                    };
                    ema
                } else {
                    f64::NAN
                }
            })
            .collect();
        Self {
            mids,
            spreads: read(|b| b.spread_bps),
            microprices: read(|b| b.microprice),
            fair_values,
        }
    }
}

/// The target value for acting at each bar and holding `horizon` bars: `NaN` where the window
/// runs past the data or a bar it needs has no book.
pub fn target_series(series: &TargetSeries, target: Target, horizon: usize) -> Vec<f64> {
    let mids = &series.mids;
    let n = mids.len();
    let mut out = vec![f64::NAN; n];
    let ret = |a: usize| -> f64 {
        let (entry, exit) = (mids[a], mids[a + horizon]);
        if entry.is_finite() && exit.is_finite() && entry > 0.0 {
            (exit / entry - 1.0) * 1e4
        } else {
            f64::NAN
        }
    };
    let residual = |a: usize, reference: &[f64]| -> f64 {
        let (mid, fair) = (mids[a + horizon], reference[a + horizon]);
        if mid.is_finite() && fair.is_finite() && mid > 0.0 {
            (mid - fair) / mid * 1e4
        } else {
            f64::NAN
        }
    };
    match target {
        Target::Return => {
            for a in 0..n.saturating_sub(horizon) {
                out[a] = ret(a);
            }
        }
        Target::AbsMove => {
            for a in 0..n.saturating_sub(horizon) {
                out[a] = ret(a).abs();
            }
        }
        Target::RealizedVariance => {
            // Prefix sums of squared bar returns, with a count of unusable bars so any gap in
            // the window makes the whole window NaN.
            let mut sum = vec![0.0; n + 1];
            let mut gaps = vec![0usize; n + 1];
            for k in 0..n {
                let r2 =
                    if k > 0 && mids[k].is_finite() && mids[k - 1].is_finite() && mids[k - 1] > 0.0
                    {
                        let r = (mids[k] / mids[k - 1] - 1.0) * 1e4;
                        r * r
                    } else {
                        f64::NAN
                    };
                sum[k + 1] = sum[k] + if r2.is_finite() { r2 } else { 0.0 };
                gaps[k + 1] = gaps[k] + usize::from(!r2.is_finite());
            }
            if horizon > 0 {
                for a in 0..n.saturating_sub(horizon) {
                    if gaps[a + horizon + 1] == gaps[a + 1] {
                        out[a] = sum[a + horizon + 1] - sum[a + 1];
                    }
                }
            }
        }
        Target::SpreadChange => {
            for a in 0..n.saturating_sub(horizon) {
                let (s0, s1) = (series.spreads[a], series.spreads[a + horizon]);
                if s0.is_finite() && s1.is_finite() {
                    out[a] = s1 - s0;
                }
            }
        }
        Target::FairValueResidual => {
            for a in 0..n.saturating_sub(horizon) {
                out[a] = residual(a, &series.fair_values);
            }
        }
        Target::MicropriceResidual => {
            for a in 0..n.saturating_sub(horizon) {
                out[a] = residual(a, &series.microprices);
            }
        }
    }
    out
}

fn default_step() -> u32 {
    1
}
fn default_session() -> SessionKind {
    SessionKind::Regular
}

/// The bar grid a study runs on: tick-built lake bars carry an order book; CSV bars from the
/// SDK loaders carry open, high, low, close, and volume only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grid {
    Lake { step_secs: u32 },
    Daily,
    FiveMinute,
    OneMinute,
}

impl Grid {
    pub fn parse(resolution: Option<&str>, step_secs: u32) -> Result<Grid> {
        Ok(
            match resolution.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
                None | Some("") | Some("lake") => {
                    anyhow::ensure!(step_secs > 0, "step_secs must be positive");
                    Grid::Lake { step_secs }
                }
                Some("daily" | "eod" | "1d" | "d") => Grid::Daily,
                Some("5m" | "five_minute") => Grid::FiveMinute,
                Some("1m" | "one_minute") => Grid::OneMinute,
                Some(other) => {
                    bail!("unknown study resolution {other:?}; use daily, 5m, 1m, or lake")
                }
            },
        )
    }
    pub fn label(self) -> String {
        match self {
            Grid::Lake { step_secs } => format!("{step_secs}s"),
            Grid::Daily => "daily".to_owned(),
            Grid::FiveMinute => "5m".to_owned(),
            Grid::OneMinute => "1m".to_owned(),
        }
    }
    /// Seconds per bar, the unit `horizon_secs` is reported in.
    pub fn step_secs(self) -> u32 {
        match self {
            Grid::Lake { step_secs } => step_secs,
            Grid::Daily => 86_400,
            Grid::FiveMinute => 300,
            Grid::OneMinute => 60,
        }
    }
    /// Bars in a year, for annualizing: the lake never closes; the CSV grids trade 252
    /// sessions of 6.5 hours.
    pub fn bars_per_year(self) -> f64 {
        match self {
            Grid::Lake { step_secs } => 365.25 * 86_400.0 / f64::from(step_secs.max(1)),
            Grid::Daily => 252.0,
            Grid::FiveMinute => 78.0 * 252.0,
            Grid::OneMinute => 390.0 * 252.0,
        }
    }
    /// A horizon in this grid's natural unit: `30s`, `5d`, `25m`.
    pub fn horizon_label(self, horizon: usize) -> String {
        match self {
            Grid::Lake { step_secs } => format!("{}s", horizon as u32 * step_secs),
            Grid::Daily => format!("{horizon}d"),
            Grid::FiveMinute => format!("{}m", horizon * 5),
            Grid::OneMinute => format!("{horizon}m"),
        }
    }
    pub fn has_book(self) -> bool {
        matches!(self, Grid::Lake { .. })
    }
    /// Bars the fair-value EMA averages over: 60 seconds on the lake, 20 bars on CSV grids.
    pub fn fair_value_window_bars(self) -> usize {
        match self {
            Grid::Lake { step_secs } => (FAIR_VALUE_EMA_SECS / step_secs.max(1)).max(1) as usize,
            _ => 20,
        }
    }
}

impl StudyConfig {
    pub fn grid(&self) -> Result<Grid> {
        Grid::parse(self.resolution.as_deref(), self.step_secs)
    }
}
fn default_features() -> Vec<String> {
    FEATURES.iter().map(|f| (*f).to_owned()).collect()
}
fn default_horizons() -> Vec<usize> {
    vec![1, 5, 30, 60]
}
fn default_buckets() -> usize {
    10
}

#[derive(Debug, Clone, Serialize)]
pub struct BucketRow {
    pub bucket: usize,
    pub count: usize,
    pub feature_mean: f64,
    /// Mean forward return in basis points.
    pub forward_bps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StudyCell {
    pub symbol: String,
    pub feature: String,
    pub horizon_bars: usize,
    pub horizon_secs: u32,
    pub observations: usize,
    /// Spearman rank correlation between the feature and the forward return.
    pub ic: f64,
    /// Top-minus-bottom bucket mean forward return, basis points.
    pub top_minus_bottom_bps: f64,
    pub top_minus_bottom_t: f64,
    /// Annualized Sharpe of the clipped z-score position (the `zscore` curve).
    pub sharpe: f64,
    /// Mean absolute position change per bar of the `zscore` curve.
    pub turnover: f64,
    /// Cost per unit traded, in bps, at which the `zscore` curve's mean P&L is zero.
    pub breakeven_bps: f64,
    pub buckets: Vec<BucketRow>,
    /// Costless curves: `zscore` first, then `sign`; a single `long_short` curve on a
    /// cross-sectional cell.
    pub curves: Vec<CostlessCurve>,
    /// Present on cross-sectional cells: the per-date IC statistics.
    pub cross_section: Option<CrossSection>,
    /// Time-series cells: incremental IC, stability, regimes, autocorrelation.
    pub diagnostics: Option<Diagnostics>,
}

/// Where a feature's edge comes from and how stable it is.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostics {
    /// The accepted expressions regressed out (with an intercept) before scoring.
    pub accepted: Vec<String>,
    /// Spearman IC of the feature's OLS residual on the accepted set; `None` without one.
    pub incremental_ic: Option<f64>,
    /// IC per day, for days with at least [`MIN_DAY_OBS`] observations.
    pub daily_ic: Vec<DailyIc>,
    /// Share of those days whose IC carries the cell's sign.
    pub sign_consistency: f64,
    /// IC by spread tercile, trailing-realized-variance tercile, and hour of the bar's clock.
    pub regimes: Vec<RegimeIc>,
    /// Lag-1 autocorrelation of the feature along time (observation-weighted over symbols).
    pub autocorrelation_1: f64,
    /// Autocorrelation at a lag of the horizon.
    pub autocorrelation_horizon: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DailyIc {
    pub date: NaiveDate,
    pub ic: f64,
    pub n: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegimeIc {
    /// `spread`, `vol`, or `hour`.
    pub regime: String,
    /// `low`, `mid`, `high`, or the hour.
    pub bucket: String,
    pub ic: f64,
    pub n: usize,
}

/// One aligned observation with what the diagnostics need to know about its bar.
#[derive(Debug, Clone)]
struct DiagObs {
    x: f64,
    y: f64,
    date: NaiveDate,
    hour: u32,
    spread: f64,
    vol: f64,
    accepted: Vec<f64>,
}

/// Trailing realized variance per bar: the sum of squared bar returns (bps) over the last
/// `window` bars, `NaN` when a bar in the window has no price.
fn trailing_realized_variance(prices: &[f64], window: usize) -> Vec<f64> {
    let n = prices.len();
    let mut sum = vec![0.0; n + 1];
    let mut gaps = vec![0usize; n + 1];
    for k in 0..n {
        let r2 =
            if k > 0 && prices[k].is_finite() && prices[k - 1].is_finite() && prices[k - 1] > 0.0 {
                let r = (prices[k] / prices[k - 1] - 1.0) * 1e4;
                r * r
            } else {
                f64::NAN
            };
        sum[k + 1] = sum[k] + if r2.is_finite() { r2 } else { 0.0 };
        gaps[k + 1] = gaps[k] + usize::from(!r2.is_finite());
    }
    (0..n)
        .map(|i| {
            if i + 1 < window {
                return f64::NAN;
            }
            let from = i + 1 - window;
            if gaps[i + 1] == gaps[from] {
                sum[i + 1] - sum[from]
            } else {
                f64::NAN
            }
        })
        .collect()
}

/// Pearson correlation of a series with itself `lag` bars back, over finite pairs.
fn autocorrelation(values: &[f64], lag: usize) -> f64 {
    if lag == 0 || values.len() <= lag {
        return f64::NAN;
    }
    let mut now = Vec::new();
    let mut then = Vec::new();
    for i in lag..values.len() {
        if values[i].is_finite() && values[i - lag].is_finite() {
            now.push(values[i]);
            then.push(values[i - lag]);
        }
    }
    if now.len() < 3 {
        return f64::NAN;
    }
    pearson(&now, &then)
}

/// OLS residual of `x` on the regressors plus an intercept, by Gaussian elimination on the
/// normal equations. Rows with a non-finite regressor keep `NaN`.
fn ols_residual(x: &[f64], regressors: &[Vec<f64>]) -> Vec<f64> {
    let k = regressors.len() + 1;
    let usable: Vec<usize> = (0..x.len())
        .filter(|&i| x[i].is_finite() && regressors.iter().all(|r| r[i].is_finite()))
        .collect();
    if usable.len() <= k {
        return vec![f64::NAN; x.len()];
    }
    let row = |i: usize| -> Vec<f64> {
        let mut r = Vec::with_capacity(k);
        r.push(1.0);
        r.extend(regressors.iter().map(|reg| reg[i]));
        r
    };
    let mut xtx = vec![vec![0.0; k]; k];
    let mut xty = vec![0.0; k];
    for &i in &usable {
        let r = row(i);
        for a in 0..k {
            xty[a] += r[a] * x[i];
            for b in 0..k {
                xtx[a][b] += r[a] * r[b];
            }
        }
    }
    // Solve xtx * beta = xty with partial pivoting; a singular system (a regressor that is
    // constant or collinear) drops to the intercept-only fit.
    let mut m = xtx;
    let mut v = xty;
    for col in 0..k {
        let pivot = (col..k)
            .max_by(|&a, &b| m[a][col].abs().partial_cmp(&m[b][col].abs()).unwrap())
            .unwrap();
        if m[pivot][col].abs() < 1e-12 {
            let mean = usable.iter().map(|&i| x[i]).sum::<f64>() / usable.len() as f64;
            return x
                .iter()
                .map(|xi| if xi.is_finite() { xi - mean } else { f64::NAN })
                .collect();
        }
        m.swap(col, pivot);
        v.swap(col, pivot);
        for r in col + 1..k {
            let factor = m[r][col] / m[col][col];
            for c in col..k {
                m[r][c] -= factor * m[col][c];
            }
            v[r] -= factor * v[col];
        }
    }
    let mut beta = vec![0.0; k];
    for col in (0..k).rev() {
        let mut acc = v[col];
        for c in col + 1..k {
            acc -= m[col][c] * beta[c];
        }
        beta[col] = acc / m[col][col];
    }
    let mut residual = vec![f64::NAN; x.len()];
    for &i in &usable {
        let fitted: f64 = row(i).iter().zip(&beta).map(|(a, b)| a * b).sum();
        residual[i] = x[i] - fitted;
    }
    residual
}

/// IC within each tercile of `by` (low, mid, high by rank), for the named regime.
fn tercile_regimes(regime: &str, obs: &[DiagObs], by: impl Fn(&DiagObs) -> f64) -> Vec<RegimeIc> {
    let mut order: Vec<usize> = (0..obs.len())
        .filter(|&i| by(&obs[i]).is_finite())
        .collect();
    if order.len() < 3 * MIN_DAY_OBS {
        return Vec::new();
    }
    order.sort_by(|a, b| {
        by(&obs[*a])
            .partial_cmp(&by(&obs[*b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let third = order.len() / 3;
    [
        ("low", 0..third),
        ("mid", third..2 * third),
        ("high", 2 * third..order.len()),
    ]
    .into_iter()
    .map(|(bucket, range)| {
        let xs: Vec<f64> = order[range.clone()].iter().map(|&i| obs[i].x).collect();
        let ys: Vec<f64> = order[range].iter().map(|&i| obs[i].y).collect();
        RegimeIc {
            regime: regime.to_owned(),
            bucket: bucket.to_owned(),
            ic: spearman(&xs, &ys),
            n: xs.len(),
        }
    })
    .collect()
}

/// The diagnostics of one time-series cell.
fn diagnostics(
    cell_ic: f64,
    horizon: usize,
    accepted: &[String],
    obs: &[DiagObs],
    feature_series: &[(&[f64], usize)],
) -> Diagnostics {
    let xs: Vec<f64> = obs.iter().map(|o| o.x).collect();
    let ys: Vec<f64> = obs.iter().map(|o| o.y).collect();
    let incremental_ic = if accepted.is_empty() {
        None
    } else {
        let regressors: Vec<Vec<f64>> = (0..accepted.len())
            .map(|k| obs.iter().map(|o| o.accepted[k]).collect())
            .collect();
        let residual = ols_residual(&xs, &regressors);
        let (rx, ry): (Vec<f64>, Vec<f64>) = residual
            .iter()
            .zip(&ys)
            .filter(|(r, _)| r.is_finite())
            .map(|(r, y)| (*r, *y))
            .unzip();
        Some(if rx.len() >= 3 {
            spearman(&rx, &ry)
        } else {
            f64::NAN
        })
    };
    // Per day.
    let mut by_day: BTreeMap<NaiveDate, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for o in obs {
        let entry = by_day.entry(o.date).or_default();
        entry.0.push(o.x);
        entry.1.push(o.y);
    }
    let daily_ic: Vec<DailyIc> = by_day
        .into_iter()
        .filter(|(_, (x, _))| x.len() >= MIN_DAY_OBS)
        .map(|(date, (x, y))| DailyIc {
            date,
            ic: spearman(&x, &y),
            n: x.len(),
        })
        .filter(|d| d.ic.is_finite())
        .collect();
    let sign_consistency = if daily_ic.is_empty() || cell_ic == 0.0 || !cell_ic.is_finite() {
        f64::NAN
    } else {
        daily_ic
            .iter()
            .filter(|d| (d.ic > 0.0) == (cell_ic > 0.0))
            .count() as f64
            / daily_ic.len() as f64
    };
    // Regimes.
    let mut regimes = tercile_regimes("spread", obs, |o| o.spread);
    regimes.extend(tercile_regimes("vol", obs, |o| o.vol));
    let mut by_hour: BTreeMap<u32, (Vec<f64>, Vec<f64>)> = BTreeMap::new();
    for o in obs {
        let entry = by_hour.entry(o.hour).or_default();
        entry.0.push(o.x);
        entry.1.push(o.y);
    }
    if by_hour.len() >= 2 {
        regimes.extend(
            by_hour
                .into_iter()
                .filter(|(_, (x, _))| x.len() >= MIN_DAY_OBS)
                .map(|(hour, (x, y))| RegimeIc {
                    regime: "hour".to_owned(),
                    bucket: format!("{hour:02}"),
                    ic: spearman(&x, &y),
                    n: x.len(),
                }),
        );
    }
    // Autocorrelation, observation-weighted over the symbols that make up the cell.
    let weighted = |lag: usize| -> f64 {
        let mut total = 0.0;
        let mut weight = 0.0;
        for (values, n) in feature_series {
            let rho = autocorrelation(values, lag);
            if rho.is_finite() && *n > 0 {
                total += rho * *n as f64;
                weight += *n as f64;
            }
        }
        if weight > 0.0 {
            total / weight
        } else {
            f64::NAN
        }
    };
    Diagnostics {
        accepted: accepted.to_vec(),
        incremental_ic,
        daily_ic,
        sign_consistency,
        regimes,
        autocorrelation_1: weighted(1),
        autocorrelation_horizon: weighted(horizon.max(1)),
    }
}

/// Per-date statistics of a cross-sectional cell. `ic` on the cell is the mean of the daily
/// rank correlations; the long-short figures are the top decile long, bottom decile short,
/// rebalanced every date and paid the target over the horizon.
#[derive(Debug, Clone, Serialize)]
pub struct CrossSection {
    /// Dates with at least `buckets` symbols, so every decile is populated.
    pub dates: usize,
    /// Mean daily IC over its standard error across dates.
    pub ic_t: f64,
    /// Share of dates with a positive IC.
    pub ic_positive_share: f64,
    pub symbols_per_date: f64,
}

/// One costless trading rule on a cell: a position taken from the feature at every bar, paid
/// the forward return over the horizon, with no costs. Overlapping holds are averaged, so the
/// P&L per bar is that of one tranche held for the horizon.
#[derive(Debug, Clone, Serialize)]
pub struct CostlessCurve {
    /// `zscore`: the feature's z-score clipped to +-3; `sign`: the sign of that z-score.
    pub variant: String,
    /// Mean P&L per bar, basis points per unit of position.
    pub mean_bps: f64,
    /// Sharpe of the per-bar P&L annualized as one independent period per horizon.
    pub sharpe: f64,
    /// Mean absolute position change per bar.
    pub turnover: f64,
    /// `mean_bps / turnover`: the cost per unit traded, in bps, that would zero the mean P&L.
    pub breakeven_bps: f64,
    /// Cumulative P&L at the last observation, bps.
    pub final_bps: f64,
    /// Cumulative P&L sampled at up to [`CURVE_POINTS`] observations.
    pub curve: Vec<CurvePoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CurvePoint {
    /// Index into the cell's aligned (feature, forward return) pairs.
    pub observation: usize,
    pub cumulative_bps: f64,
}

/// Points kept per curve; enough for a chart, small enough for thousands of cells.
pub const CURVE_POINTS: usize = 400;

/// Z-scores beyond this many standard deviations hold the same position as at the clip.
const Z_CLIP: f64 = 3.0;

/// Independent periods per year for annualizing a horizon on a grid.
fn periods_per_year(grid: Grid, horizon: usize) -> f64 {
    grid.bars_per_year() / horizon.max(1) as f64
}

/// A cumulative series sampled at up to [`CURVE_POINTS`] observations, first and last kept.
fn curve_points(cumulative: &[f64]) -> Vec<CurvePoint> {
    let n = cumulative.len();
    if n <= CURVE_POINTS {
        cumulative
            .iter()
            .enumerate()
            .map(|(observation, &cumulative_bps)| CurvePoint {
                observation,
                cumulative_bps,
            })
            .collect()
    } else {
        (0..CURVE_POINTS)
            .map(|k| {
                let observation = k * (n - 1) / (CURVE_POINTS - 1);
                CurvePoint {
                    observation,
                    cumulative_bps: cumulative[observation],
                }
            })
            .collect()
    }
}

/// The costless curve for one variant over aligned (feature, forward return in bps) pairs.
pub fn costless_curve(
    variant: &str,
    xs: &[f64],
    ys: &[f64],
    periods_per_year: f64,
) -> CostlessCurve {
    let n = xs.len();
    let positions: Vec<f64> = if n < 2 {
        vec![0.0; n]
    } else {
        let mean = xs.iter().sum::<f64>() / n as f64;
        let sd = (xs.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n as f64).sqrt();
        xs.iter()
            .map(|x| {
                if sd <= 0.0 {
                    return 0.0;
                }
                let z = (x - mean) / sd;
                match variant {
                    "sign" => {
                        if z > 0.0 {
                            1.0
                        } else if z < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    _ => z.clamp(-Z_CLIP, Z_CLIP),
                }
            })
            .collect()
    };
    let pnl: Vec<f64> = positions.iter().zip(ys).map(|(p, y)| p * y).collect();
    let (mean_bps, se) = mean_and_se(&pnl);
    let sd = se * (n as f64).sqrt();
    let sharpe = if sd > 0.0 {
        mean_bps / sd * periods_per_year.sqrt()
    } else {
        f64::NAN
    };
    let turnover = if n < 2 {
        0.0
    } else {
        positions
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .sum::<f64>()
            / (n - 1) as f64
    };
    let breakeven_bps = if turnover > 0.0 {
        mean_bps / turnover
    } else {
        f64::NAN
    };
    let mut cumulative = Vec::with_capacity(n);
    let mut total = 0.0;
    for value in &pnl {
        total += value;
        cumulative.push(total);
    }
    let curve = curve_points(&cumulative);
    CostlessCurve {
        variant: variant.to_owned(),
        mean_bps: if n == 0 { f64::NAN } else { mean_bps },
        sharpe,
        turnover,
        breakeven_bps,
        final_bps: cumulative.last().copied().unwrap_or(f64::NAN),
        curve,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StudyResult {
    pub config: StudyConfig,
    /// The target every cell was scored against (also in `config`), and its unit.
    pub target: Target,
    pub target_unit: String,
    /// The bar grid: `1s`, `daily`, `5m`, or `1m`.
    pub grid: String,
    pub mode: StudyMode,
    /// Exogenous series that were registered for this study, usable as bases.
    pub series: Vec<String>,
    /// Features that could not run on this grid, with the reason, instead of failing.
    pub unavailable: Vec<UnavailableFeature>,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub symbols: Vec<SymbolCoverage>,
    pub cells: Vec<StudyCell>,
    /// One event study per registered `event` series: the mean price path around its events.
    pub events: Vec<EventStudy>,
    /// The file next to the study holding the accepted features and the targets for model
    /// fitting (`accepted.parquet`), when the study has an accepted set.
    pub accepted_export: Option<String>,
}

/// The average cumulative return path around the events of one series, normalised to zero
/// at the event bar: offset -5 is the return from five bars before the event to the event
/// bar, so a run-up shows as negative values climbing to zero; offset +10 is the return from
/// the event bar to ten bars later.
#[derive(Debug, Clone, Serialize)]
pub struct EventStudy {
    pub series: String,
    /// Bars either side of the event.
    pub window: usize,
    /// Events with a full window of bars on both sides, across all symbols.
    pub events: usize,
    pub points: Vec<EventPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EventPoint {
    pub offset: i64,
    /// Mean cumulative return from the event bar to this offset, bps.
    pub mean_bps: f64,
    /// Mean over its standard error across events.
    pub t: f64,
    pub count: usize,
}

/// The event study of one `event` series over the panel: each symbol's events are the bars
/// on which a row first became visible; the path uses the symbol's mid (close on OHLCV grids).
pub fn event_study(
    grid: Grid,
    panel: &[(String, Vec<lake::LakeBar>)],
    series: &Series,
    window: usize,
) -> EventStudy {
    let offsets: Vec<i64> = (-(window as i64)..=window as i64).collect();
    let mut paths: Vec<Vec<f64>> = vec![Vec::new(); offsets.len()];
    let mut events = 0usize;
    for (symbol, bars) in panel {
        let landed = series.align(
            symbol,
            &bars
                .iter()
                .map(|b| bar_close_us(b, grid))
                .collect::<Vec<_>>(),
        );
        let prices = TargetSeries::from_bars(bars, grid).mids;
        for (e, value) in landed.iter().enumerate() {
            if !(value.is_finite() && *value != 0.0) {
                continue;
            }
            if e < window || e + window >= bars.len() {
                continue;
            }
            let anchor = prices[e];
            if !(anchor.is_finite() && anchor > 0.0) {
                continue;
            }
            events += 1;
            for (k, offset) in offsets.iter().enumerate() {
                let index = (e as i64 + offset) as usize;
                let price = prices[index];
                if price.is_finite() && price > 0.0 {
                    paths[k].push((price / anchor - 1.0) * 1e4);
                }
            }
        }
    }
    let points = offsets
        .iter()
        .zip(&paths)
        .map(|(&offset, values)| {
            let (mean, se) = mean_and_se(values);
            EventPoint {
                offset,
                mean_bps: if values.is_empty() { f64::NAN } else { mean },
                t: if se > 0.0 { mean / se } else { f64::NAN },
                count: values.len(),
            }
        })
        .collect();
    EventStudy {
        series: series.name.clone(),
        window,
        events,
        points,
    }
}

/// Each day's final value of an intraday series, keyed by date: what `agg daily` features
/// lift onto a daily panel.
pub fn lift_to_daily(intraday: &[lake::LakeBar], values: &[f64]) -> BTreeMap<NaiveDate, f64> {
    let mut out = BTreeMap::new();
    for (bar, &value) in intraday.iter().zip(values) {
        out.insert(bar.date, value);
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct UnavailableFeature {
    pub feature: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolCoverage {
    pub symbol: String,
    pub bars: usize,
    pub bars_with_book: usize,
}

/// One symbol's aligned feature matrix and mid-price series.
struct SymbolSeries {
    symbol: String,
    book: TargetSeries,
    features: BTreeMap<String, Vec<f64>>,
    /// The accepted set evaluated on the same bars, in config order.
    accepted: Vec<Vec<f64>>,
    /// Each bar's close instant (microseconds UTC): the cross-section groups symbols by it.
    keys: Vec<i64>,
    dates: Vec<NaiveDate>,
    hours: Vec<u32>,
    trailing_vol: Vec<f64>,
    bars: usize,
    bars_with_book: usize,
}

/// One symbol's bars on the study's grid: tick-built from the lake, or CSV bars through the
/// SDK loaders (sanitized daily prints, session-filtered intraday prints).
pub fn load_panel_symbol(
    config: &StudyConfig,
    grid: Grid,
    symbol: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<lake::LakeBar>> {
    match grid {
        Grid::Lake { step_secs } => {
            anyhow::ensure!(
                config.lake_dir.as_os_str().len() > 0,
                "the lake grid needs lake_dir"
            );
            let sym = LakeSymbol::parse(symbol)
                .with_context(|| format!("{symbol} is not EXCHANGE:SYMBOL"))?;
            lake::build_bars(&config.lake_dir, &sym, step_secs, start, end)
        }
        Grid::Daily => {
            let path = config.daily_dir.join(format!("{symbol}.csv"));
            let mut bars = load_daily(&path)?;
            let calendar: Option<BTreeSet<NaiveDate>> = config
                .calendar_symbol
                .as_deref()
                .filter(|c| !c.is_empty())
                .and_then(|c| load_daily(&config.daily_dir.join(format!("{c}.csv"))).ok())
                .map(|bars| bars.iter().map(|b| b.date).collect());
            sanitize_daily(&mut bars, calendar.as_ref())
                .map_err(|reason| anyhow::anyhow!("{symbol}: {reason}"))?;
            bars.retain(|bar| bar.date >= start && bar.date <= end);
            Ok(bars.iter().map(lake_bar).collect())
        }
        Grid::FiveMinute | Grid::OneMinute => {
            let dir = if grid == Grid::FiveMinute {
                &config.five_minute_dir
            } else {
                &config.one_minute_dir
            };
            let path = dir.join(format!("{symbol}.csv"));
            let dates: HashSet<NaiveDate> = start.iter_days().take_while(|d| *d <= end).collect();
            let bars = load_intraday(&path, config.session, Some(&dates))?;
            Ok(bars.iter().map(lake_bar).collect())
        }
    }
}

/// A CSV bar as the study sees it: prices and volume, no order book.
fn lake_bar(bar: &Bar) -> lake::LakeBar {
    lake::LakeBar {
        date: bar.date,
        time: bar.time,
        open: bar.open,
        high: bar.high,
        low: bar.low,
        close: bar.close,
        volume: bar.volume,
        book: None,
    }
}

fn evaluate_symbol(
    features: &[String],
    accepted: &[String],
    grid: Grid,
    symbol: &str,
    bars: &[lake::LakeBar],
    series: &[Series],
    lifted: Option<&BTreeMap<String, BTreeMap<NaiveDate, f64>>>,
) -> Result<SymbolSeries> {
    let names: Vec<String> = series.iter().map(|s| s.name.clone()).collect();
    let exogenous = align_series(grid, symbol, bars, series);
    let (_, accepted_values, _) = evaluate_features_with(
        accepted,
        grid.step_secs(),
        bars,
        grid.has_book(),
        &names,
        &exogenous,
    )?;
    let accepted: Vec<Vec<f64>> = accepted
        .iter()
        .map(|name| accepted_values.get(name).cloned().unwrap_or_default())
        .collect();
    let (_, mut features, bars_with_book) = evaluate_features_with(
        features,
        grid.step_secs(),
        bars,
        grid.has_book(),
        &names,
        &exogenous,
    )?;
    // Features computed on the intraday source replace their daily-grid evaluation.
    if let Some(lifted) = lifted {
        for (name, by_date) in lifted {
            if let Some(values) = features.get_mut(name) {
                for (value, bar) in values.iter_mut().zip(bars) {
                    *value = by_date.get(&bar.date).copied().unwrap_or(f64::NAN);
                }
            }
        }
    }
    let book = TargetSeries::from_bars(bars, grid);
    let trailing_vol = trailing_realized_variance(&book.mids, VOL_WINDOW);
    Ok(SymbolSeries {
        symbol: symbol.to_owned(),
        book,
        features,
        accepted,
        keys: bars.iter().map(|bar| bar_close_us(bar, grid)).collect(),
        dates: bars.iter().map(|bar| bar.date).collect(),
        hours: bars
            .iter()
            .map(|bar| chrono::Timelike::hour(&bar.time))
            .collect(),
        trailing_vol,
        bars: bars.len(),
        bars_with_book,
    })
}

/// Runs every feature expression over the bars in one streaming pass. Returns the mid series
/// (`NaN` where the bar has no book), the feature matrix, and the count of bars with a book.
/// `book_grid` says whether returns read the mid (a bar without a book is a gap) or the close.
#[cfg(test)]
fn evaluate_features(
    expressions: &[String],
    step_secs: u32,
    bars: &[lake::LakeBar],
    book_grid: bool,
) -> Result<(Vec<f64>, BTreeMap<String, Vec<f64>>, usize)> {
    evaluate_features_with(expressions, step_secs, bars, book_grid, &[], &[])
}

/// As [`evaluate_features`], with exogenous series (names, and values aligned to the bars,
/// series-major) available as bases.
fn evaluate_features_with(
    expressions: &[String],
    step_secs: u32,
    bars: &[lake::LakeBar],
    book_grid: bool,
    series_names: &[String],
    exogenous: &[Vec<f64>],
) -> Result<(Vec<f64>, BTreeMap<String, Vec<f64>>, usize)> {
    let mut evaluators = Vec::with_capacity(expressions.len());
    for text in expressions {
        let expr = feature_expr::parse_with(text, series_names)
            .with_context(|| format!("feature expression {text:?}"))?;
        evaluators.push((
            text.clone(),
            Evaluator::for_panel(&expr, step_secs, book_grid, series_names),
        ));
    }
    let mut row = vec![f64::NAN; exogenous.len()];
    let mut mids = Vec::with_capacity(bars.len());
    let mut features: BTreeMap<String, Vec<f64>> = expressions
        .iter()
        .map(|f| (f.clone(), Vec::with_capacity(bars.len())))
        .collect();
    let mut bars_with_book = 0;
    let mut previous_mid = None;
    for (index, bar) in bars.iter().enumerate() {
        for (slot, values) in row.iter_mut().zip(exogenous) {
            *slot = values.get(index).copied().unwrap_or(f64::NAN);
        }
        let input = BarInput {
            bar,
            previous_mid,
            exogenous: &row,
        };
        for (name, evaluator) in &mut evaluators {
            let value = evaluator.next(input);
            if let Some(values) = features.get_mut(name) {
                values.push(value);
            }
        }
        match bar.book {
            Some(book) => {
                bars_with_book += 1;
                mids.push(book.mid);
                previous_mid = Some(book.mid);
            }
            None => {
                mids.push(f64::NAN);
                previous_mid = None;
            }
        }
    }
    Ok((mids, features, bars_with_book))
}

/// Average rank with ties sharing the mean rank.
fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|a, b| {
        values[*a]
            .partial_cmp(&values[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![0.0; values.len()];
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && values[order[j + 1]] == values[order[i]] {
            j += 1;
        }
        let rank = (i + j) as f64 / 2.0 + 1.0;
        for k in i..=j {
            out[order[k]] = rank;
        }
        i = j + 1;
    }
    out
}

fn pearson(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    if n < 3.0 {
        return f64::NAN;
    }
    let mx = x.iter().sum::<f64>() / n;
    let my = y.iter().sum::<f64>() / n;
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (a, b) in x.iter().zip(y) {
        sxy += (a - mx) * (b - my);
        sxx += (a - mx) * (a - mx);
        syy += (b - my) * (b - my);
    }
    if sxx <= 0.0 || syy <= 0.0 {
        0.0
    } else {
        sxy / (sxx * syy).sqrt()
    }
}

/// Spearman rank correlation.
pub fn spearman(x: &[f64], y: &[f64]) -> f64 {
    pearson(&ranks(x), &ranks(y))
}

fn mean_and_se(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    if n < 2.0 {
        return (values.first().copied().unwrap_or(f64::NAN), f64::NAN);
    }
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0);
    (mean, (var / n).sqrt())
}

/// Aligned (feature, target) pairs: the feature observed at bar `i`, the target from acting
/// `delay` bars later (see [`target_series`] for the target per acting bar).
#[cfg(test)]
fn pairs(feature: &[f64], targets: &[f64], delay: usize) -> (Vec<f64>, Vec<f64>) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (i, &x) in feature.iter().enumerate() {
        let Some(&y) = targets.get(i + delay) else {
            break;
        };
        if x.is_finite() && y.is_finite() {
            xs.push(x);
            ys.push(y);
        }
    }
    (xs, ys)
}

/// As [`pairs`], keeping the feature bar's index for grouping across symbols.
fn pairs_indexed(feature: &[f64], targets: &[f64], delay: usize) -> Vec<(usize, f64, f64)> {
    let mut out = Vec::new();
    for (i, &x) in feature.iter().enumerate() {
        let Some(&y) = targets.get(i + delay) else {
            break;
        };
        if x.is_finite() && y.is_finite() {
            out.push((i, x, y));
        }
    }
    out
}

/// A cross-sectional cell over (date key, symbol index, feature, target) observations: per
/// date with at least `buckets` symbols, the rank correlation across symbols, decile means,
/// and the long-short decile portfolio (long the top decile, short the bottom, equal weights
/// on each side, rebalanced every date). Turnover is the sum of absolute weight changes per
/// date (a full swap of both sides is 4); breakeven is the mean long-short P&L over it.
/// `None` with fewer than two usable dates.
fn cross_section_cell(
    feature: &str,
    horizon: usize,
    grid: Grid,
    buckets: usize,
    observations: &[(i64, usize, f64, f64)],
) -> Option<StudyCell> {
    let buckets = buckets.max(2);
    let mut by_date: BTreeMap<i64, Vec<(usize, f64, f64)>> = BTreeMap::new();
    for &(key, symbol, x, y) in observations {
        by_date.entry(key).or_default().push((symbol, x, y));
    }
    let mut ics = Vec::new();
    let mut pnls = Vec::new();
    let mut turnovers = Vec::new();
    let mut bucket_x = vec![0.0; buckets];
    let mut bucket_y = vec![0.0; buckets];
    let mut bucket_n = vec![0usize; buckets];
    let mut previous: HashMap<usize, f64> = HashMap::new();
    let mut used = 0usize;
    for rows in by_date.values() {
        let n = rows.len();
        if n < buckets {
            continue;
        }
        let xs: Vec<f64> = rows.iter().map(|r| r.1).collect();
        let ys: Vec<f64> = rows.iter().map(|r| r.2).collect();
        let ic = spearman(&xs, &ys);
        if !ic.is_finite() {
            continue;
        }
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|a, b| {
            xs[*a]
                .partial_cmp(&xs[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut top = Vec::new();
        let mut bottom = Vec::new();
        for (rank, &i) in order.iter().enumerate() {
            let b = rank * buckets / n;
            bucket_x[b] += xs[i];
            bucket_y[b] += ys[i];
            bucket_n[b] += 1;
            if b == 0 {
                bottom.push(i);
            }
            if b + 1 == buckets {
                top.push(i);
            }
        }
        let mean = |set: &[usize]| set.iter().map(|&i| ys[i]).sum::<f64>() / set.len() as f64;
        let mut weights: HashMap<usize, f64> = HashMap::with_capacity(top.len() + bottom.len());
        for &i in &top {
            weights.insert(rows[i].0, 1.0 / top.len() as f64);
        }
        for &i in &bottom {
            weights.insert(rows[i].0, -1.0 / bottom.len() as f64);
        }
        if !previous.is_empty() {
            let mut turnover = 0.0;
            for (symbol, weight) in &weights {
                turnover += (weight - previous.get(symbol).copied().unwrap_or(0.0)).abs();
            }
            for (symbol, weight) in &previous {
                if !weights.contains_key(symbol) {
                    turnover += weight.abs();
                }
            }
            turnovers.push(turnover);
        }
        previous = weights;
        ics.push(ic);
        pnls.push(mean(&top) - mean(&bottom));
        used += n;
    }
    if ics.len() < 2 {
        return None;
    }
    let (ic_mean, ic_se) = mean_and_se(&ics);
    let (pnl_mean, pnl_se) = mean_and_se(&pnls);
    let sd = pnl_se * (pnls.len() as f64).sqrt();
    let sharpe = if sd > 0.0 {
        pnl_mean / sd * periods_per_year(grid, horizon).sqrt()
    } else {
        f64::NAN
    };
    let turnover = if turnovers.is_empty() {
        0.0
    } else {
        turnovers.iter().sum::<f64>() / turnovers.len() as f64
    };
    let breakeven_bps = if turnover > 0.0 {
        pnl_mean / turnover
    } else {
        f64::NAN
    };
    let mut cumulative = Vec::with_capacity(pnls.len());
    let mut total = 0.0;
    for pnl in &pnls {
        total += pnl;
        cumulative.push(total);
    }
    let rows = (0..buckets)
        .filter(|&b| bucket_n[b] > 0)
        .map(|b| BucketRow {
            bucket: b + 1,
            count: bucket_n[b],
            feature_mean: bucket_x[b] / bucket_n[b] as f64,
            forward_bps: bucket_y[b] / bucket_n[b] as f64,
        })
        .collect();
    Some(StudyCell {
        symbol: "ALL".to_owned(),
        feature: feature.to_owned(),
        horizon_bars: horizon,
        horizon_secs: horizon as u32 * grid.step_secs(),
        observations: used,
        ic: ic_mean,
        top_minus_bottom_bps: pnl_mean,
        top_minus_bottom_t: if pnl_se > 0.0 {
            pnl_mean / pnl_se
        } else {
            f64::NAN
        },
        sharpe,
        turnover,
        breakeven_bps,
        buckets: rows,
        curves: vec![CostlessCurve {
            variant: "long_short".to_owned(),
            mean_bps: pnl_mean,
            sharpe,
            turnover,
            breakeven_bps,
            final_bps: total,
            curve: curve_points(&cumulative),
        }],
        cross_section: Some(CrossSection {
            dates: ics.len(),
            ic_t: if ic_se > 0.0 {
                ic_mean / ic_se
            } else {
                f64::NAN
            },
            ic_positive_share: ics.iter().filter(|ic| **ic > 0.0).count() as f64 / ics.len() as f64,
            symbols_per_date: used as f64 / ics.len() as f64,
        }),
        diagnostics: None,
    })
}

fn cell(
    symbol: &str,
    feature: &str,
    horizon: usize,
    grid: Grid,
    buckets: usize,
    xs: &[f64],
    ys: &[f64],
) -> StudyCell {
    let ic = spearman(xs, ys);
    // Bucket by feature rank.
    let order = {
        let mut o: Vec<usize> = (0..xs.len()).collect();
        o.sort_by(|a, b| {
            xs[*a]
                .partial_cmp(&xs[*b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        o
    };
    let mut rows = Vec::with_capacity(buckets);
    let mut top: Vec<f64> = Vec::new();
    let mut bottom: Vec<f64> = Vec::new();
    for b in 0..buckets {
        let lo = b * order.len() / buckets;
        let hi = (b + 1) * order.len() / buckets;
        let slice = &order[lo..hi];
        if slice.is_empty() {
            continue;
        }
        let fx: Vec<f64> = slice.iter().map(|&i| xs[i]).collect();
        let fy: Vec<f64> = slice.iter().map(|&i| ys[i]).collect();
        if b == 0 {
            bottom = fy.clone();
        }
        if b + 1 == buckets {
            top = fy.clone();
        }
        rows.push(BucketRow {
            bucket: b + 1,
            count: slice.len(),
            feature_mean: fx.iter().sum::<f64>() / fx.len() as f64,
            forward_bps: fy.iter().sum::<f64>() / fy.len() as f64,
        });
    }
    let (top_mean, top_se) = mean_and_se(&top);
    let (bottom_mean, bottom_se) = mean_and_se(&bottom);
    let diff = top_mean - bottom_mean;
    let se = (top_se * top_se + bottom_se * bottom_se).sqrt();
    let per_year = periods_per_year(grid, horizon);
    let curves = vec![
        costless_curve("zscore", xs, ys, per_year),
        costless_curve("sign", xs, ys, per_year),
    ];
    StudyCell {
        symbol: symbol.to_owned(),
        feature: feature.to_owned(),
        horizon_bars: horizon,
        horizon_secs: horizon as u32 * grid.step_secs(),
        observations: xs.len(),
        ic,
        top_minus_bottom_bps: diff,
        top_minus_bottom_t: if se > 0.0 { diff / se } else { f64::NAN },
        sharpe: curves[0].sharpe,
        turnover: curves[0].turnover,
        breakeven_bps: curves[0].breakeven_bps,
        buckets: rows,
        curves,
        cross_section: None,
        diagnostics: None,
    }
}

pub fn run(
    config: &StudyConfig,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<StudyResult> {
    if config.symbols.is_empty() {
        bail!("the study needs at least one symbol");
    }
    let grid = config.grid()?;
    let mut series = declared_series(config)?;
    if grid.has_book() && config.lake_series && !config.lake_dir.as_os_str().is_empty() {
        series.extend(lake_series(&config.lake_dir, &config.symbols, start, end)?);
    }
    let names: Vec<String> = series.iter().map(|s| s.name.clone()).collect();
    for feature in config.features.iter().chain(&config.accepted) {
        feature_expr::parse_with(feature, &names)
            .with_context(|| format!("feature expression {feature:?}"))?;
    }
    eprintln!(
        "progress: load 0/{} symbols elapsed=0s",
        config.symbols.len()
    );
    let panel: Vec<(String, Vec<lake::LakeBar>)> = config
        .symbols
        .par_iter()
        .map(|symbol| {
            Ok((
                symbol.clone(),
                load_panel_symbol(config, grid, symbol, start, end)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    // On the daily grid, `agg daily` features are computed on the intraday source and lifted.
    let lifted = match (grid, config.intraday_source.as_deref()) {
        (Grid::Daily, Some(source)) => {
            let agg_features: Vec<String> = config
                .features
                .iter()
                .filter(|f| {
                    feature_expr::parse_with(f, &names)
                        .is_ok_and(|e| feature_expr::has_daily_agg(&e))
                })
                .cloned()
                .collect();
            if agg_features.is_empty() {
                Vec::new()
            } else {
                let source_grid = Grid::parse(Some(source), 1)?;
                anyhow::ensure!(
                    !source_grid.has_book() && source_grid != Grid::Daily,
                    "intraday_source must be 1m or 5m"
                );
                let source_config = StudyConfig {
                    resolution: Some(source.to_owned()),
                    ..config.clone()
                };
                config
                    .symbols
                    .par_iter()
                    .map(|symbol| {
                        let bars =
                            load_panel_symbol(&source_config, source_grid, symbol, start, end)?;
                        let exogenous = align_series(source_grid, symbol, &bars, &series);
                        let (_, values, _) = evaluate_features_with(
                            &agg_features,
                            source_grid.step_secs(),
                            &bars,
                            false,
                            &names,
                            &exogenous,
                        )?;
                        Ok(values
                            .into_iter()
                            .map(|(name, v)| (name, lift_to_daily(&bars, &v)))
                            .collect::<BTreeMap<_, _>>())
                    })
                    .collect::<Result<Vec<_>>>()?
            }
        }
        _ => Vec::new(),
    };
    run_study(config, panel, series, lifted, start, end, output_dir)
}

/// The series declared in the config, loaded from their files.
fn declared_series(config: &StudyConfig) -> Result<Vec<Series>> {
    let root = std::env::current_dir().unwrap_or_default();
    config
        .series
        .iter()
        .map(|spec| Series::load(spec, &root))
        .collect()
}

/// The study over bars already in memory, one series per symbol, on the config's grid. Every
/// feature that reads the order book on a grid without one is reported as unavailable rather
/// than failing the study.
pub fn run_on_panel(
    config: &StudyConfig,
    panel: Vec<(String, Vec<lake::LakeBar>)>,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<StudyResult> {
    let series = declared_series(config)?;
    run_on_panel_with_series(config, panel, series, start, end, output_dir)
}

/// [`run_on_panel`] with the exogenous series already loaded (declared files, lake side
/// feeds, or anything built in memory).
pub fn run_on_panel_with_series(
    config: &StudyConfig,
    panel: Vec<(String, Vec<lake::LakeBar>)>,
    series: Vec<Series>,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<StudyResult> {
    run_study(config, panel, series, Vec::new(), start, end, output_dir)
}

/// The study proper. `lifted` holds, per panel symbol, the `agg daily` features already
/// computed on an intraday source and keyed by date (empty when there are none).
fn run_study(
    config: &StudyConfig,
    panel: Vec<(String, Vec<lake::LakeBar>)>,
    series: Vec<Series>,
    lifted: Vec<BTreeMap<String, BTreeMap<NaiveDate, f64>>>,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<StudyResult> {
    let grid = config.grid()?;
    let series_names: Vec<String> = series.iter().map(|s| s.name.clone()).collect();
    let started = std::time::Instant::now();
    fs::create_dir_all(output_dir)?;
    let has_book = grid.has_book();
    if !has_book
        && matches!(
            config.target,
            Target::SpreadChange | Target::MicropriceResidual
        )
    {
        bail!(
            "target {} needs order-book bars; unavailable on the {} grid",
            config.target.name(),
            grid.label()
        );
    }
    let mut features = Vec::new();
    let mut unavailable = Vec::new();
    for text in &config.features {
        let expr = feature_expr::parse_with(text, &series_names)
            .with_context(|| format!("feature expression {text:?}"))?;
        if !has_book && feature_expr::needs_book(&expr) {
            unavailable.push(UnavailableFeature {
                feature: text.clone(),
                reason: format!(
                    "unavailable on this grid: needs order-book bars, and the {} grid has none",
                    grid.label()
                ),
            });
        } else if grid == Grid::Daily
            && feature_expr::has_daily_agg(&expr)
            && !lifted.iter().any(|l| l.contains_key(text))
        {
            unavailable.push(UnavailableFeature {
                feature: text.clone(),
                reason: "unavailable on this grid: agg daily needs an intraday_source (1m or 5m) to lift from".to_owned(),
            });
        } else {
            features.push(text.clone());
        }
    }
    let exogenous = series;
    let series: Vec<SymbolSeries> = panel
        .par_iter()
        .enumerate()
        .map(|(i, (symbol, bars))| {
            evaluate_symbol(
                &features,
                &config.accepted,
                grid,
                symbol,
                bars,
                &exogenous,
                lifted.get(i),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    // Event studies for every event-kind series.
    let events: Vec<EventStudy> = exogenous
        .iter()
        .filter(|s| s.kind == SeriesKind::Event)
        .map(|s| event_study(grid, &panel, s, config.event_window.max(1)))
        .collect();
    eprintln!(
        "progress: load {}/{} symbols loaded elapsed={}s",
        series.len(),
        panel.len(),
        started.elapsed().as_secs()
    );
    // The target per symbol and horizon, shared by every feature.
    let targets: Vec<Vec<Vec<f64>>> = series
        .par_iter()
        .map(|s| {
            config
                .horizons
                .iter()
                .map(|&h| target_series(&s.book, config.target, h))
                .collect()
        })
        .collect();
    let mut cells = Vec::new();
    let total = (series.len() + 1) * features.len() * config.horizons.len();
    let mut done = 0usize;
    for feature in &features {
        for (hi, &horizon) in config.horizons.iter().enumerate() {
            if config.mode == StudyMode::CrossSectional {
                let mut observations = Vec::new();
                for (si, (s, symbol_targets)) in series.iter().zip(&targets).enumerate() {
                    for (i, x, y) in pairs_indexed(
                        &s.features[feature],
                        &symbol_targets[hi],
                        config.decision_delay_bars,
                    ) {
                        observations.push((s.keys[i], si, x, y));
                    }
                    done += 1;
                }
                if let Some(c) =
                    cross_section_cell(feature, horizon, grid, config.buckets, &observations)
                {
                    cells.push(c);
                }
                done += 1;
                eprintln!(
                    "progress: study {done}/{total} cells elapsed={}s",
                    started.elapsed().as_secs()
                );
                continue;
            }
            let mut pooled_x = Vec::new();
            let mut pooled_y = Vec::new();
            let mut pooled_obs: Vec<DiagObs> = Vec::new();
            let mut pooled_series: Vec<(&[f64], usize)> = Vec::new();
            for (s, symbol_targets) in series.iter().zip(&targets) {
                let indexed = pairs_indexed(
                    &s.features[feature],
                    &symbol_targets[hi],
                    config.decision_delay_bars,
                );
                let obs: Vec<DiagObs> = indexed
                    .iter()
                    .map(|&(i, x, y)| DiagObs {
                        x,
                        y,
                        date: s.dates[i],
                        hour: s.hours[i],
                        spread: s.book.spreads.get(i).copied().unwrap_or(f64::NAN),
                        vol: s.trailing_vol.get(i).copied().unwrap_or(f64::NAN),
                        accepted: s
                            .accepted
                            .iter()
                            .map(|a| a.get(i).copied().unwrap_or(f64::NAN))
                            .collect(),
                    })
                    .collect();
                let xs: Vec<f64> = indexed.iter().map(|o| o.1).collect();
                let ys: Vec<f64> = indexed.iter().map(|o| o.2).collect();
                if xs.len() >= 100 {
                    let mut c = cell(&s.symbol, feature, horizon, grid, config.buckets, &xs, &ys);
                    c.diagnostics = Some(diagnostics(
                        c.ic,
                        horizon,
                        &config.accepted,
                        &obs,
                        &[(s.features[feature].as_slice(), xs.len())],
                    ));
                    cells.push(c);
                }
                pooled_series.push((s.features[feature].as_slice(), xs.len()));
                pooled_x.extend(xs);
                pooled_y.extend(ys);
                pooled_obs.extend(obs);
                done += 1;
            }
            if series.len() > 1 && pooled_x.len() >= 100 {
                let mut c = cell(
                    "ALL",
                    feature,
                    horizon,
                    grid,
                    config.buckets,
                    &pooled_x,
                    &pooled_y,
                );
                c.diagnostics = Some(diagnostics(
                    c.ic,
                    horizon,
                    &config.accepted,
                    &pooled_obs,
                    &pooled_series,
                ));
                cells.push(c);
            }
            done += 1;
            eprintln!(
                "progress: study {done}/{total} cells elapsed={}s",
                started.elapsed().as_secs()
            );
        }
    }
    let accepted_export = if config.accepted.is_empty() {
        None
    } else {
        write_accepted_frame(
            &output_dir.join("accepted.parquet"),
            config,
            grid,
            &series,
            &targets,
        )?;
        Some("accepted.parquet".to_owned())
    };
    let result = StudyResult {
        config: config.clone(),
        target: config.target,
        target_unit: config.target.unit().to_owned(),
        grid: grid.label(),
        mode: config.mode,
        series: series_names,
        unavailable,
        start,
        end,
        symbols: series
            .iter()
            .map(|s| SymbolCoverage {
                symbol: s.symbol.clone(),
                bars: s.bars,
                bars_with_book: s.bars_with_book,
            })
            .collect(),
        cells,
        events,
        accepted_export,
    };
    fs::write(
        output_dir.join("study.json"),
        serde_json::to_string_pretty(&result)?,
    )?;
    let mut csv = String::from(
        "symbol,feature,horizon_secs,observations,ic,top_minus_bottom_bps,top_minus_bottom_t,\
         sharpe,turnover,breakeven_bps,sign_sharpe,sign_turnover,sign_breakeven_bps,\
         dates,ic_t,ic_positive_share,incremental_ic,sign_consistency,autocorrelation_1\n",
    );
    let mut daily_csv = String::from("symbol,feature,horizon_secs,date,ic,n\n");
    let mut regime_csv = String::from("symbol,feature,horizon_secs,regime,bucket,ic,n\n");
    let mut curves =
        String::from("symbol,feature,horizon_secs,variant,observation,cumulative_bps\n");
    for c in &result.cells {
        let sign = c.curves.get(1);
        let cross = c.cross_section.as_ref();
        csv.push_str(&format!(
            "{},{},{},{},{:.5},{:.3},{:.2},{:.3},{:.4},{:.4},{},{},{},{},{},{},{},{},{}\n",
            c.symbol,
            c.feature,
            c.horizon_secs,
            c.observations,
            c.ic,
            c.top_minus_bottom_bps,
            c.top_minus_bottom_t,
            c.sharpe,
            c.turnover,
            c.breakeven_bps,
            sign.map(|s| format!("{:.3}", s.sharpe)).unwrap_or_default(),
            sign.map(|s| format!("{:.4}", s.turnover))
                .unwrap_or_default(),
            sign.map(|s| format!("{:.4}", s.breakeven_bps))
                .unwrap_or_default(),
            cross.map(|x| x.dates.to_string()).unwrap_or_default(),
            cross.map(|x| format!("{:.2}", x.ic_t)).unwrap_or_default(),
            cross
                .map(|x| format!("{:.3}", x.ic_positive_share))
                .unwrap_or_default(),
            c.diagnostics
                .as_ref()
                .and_then(|d| d.incremental_ic)
                .map(|v| format!("{v:.5}"))
                .unwrap_or_default(),
            c.diagnostics
                .as_ref()
                .map(|d| format!("{:.3}", d.sign_consistency))
                .unwrap_or_default(),
            c.diagnostics
                .as_ref()
                .map(|d| format!("{:.4}", d.autocorrelation_1))
                .unwrap_or_default(),
        ));
        if let Some(d) = &c.diagnostics {
            for day in &d.daily_ic {
                daily_csv.push_str(&format!(
                    "{},{},{},{},{:.5},{}\n",
                    c.symbol, c.feature, c.horizon_secs, day.date, day.ic, day.n
                ));
            }
            for r in &d.regimes {
                regime_csv.push_str(&format!(
                    "{},{},{},{},{},{:.5},{}\n",
                    c.symbol, c.feature, c.horizon_secs, r.regime, r.bucket, r.ic, r.n
                ));
            }
        }
        for curve in &c.curves {
            for point in &curve.curve {
                curves.push_str(&format!(
                    "{},{},{},{},{},{:.4}\n",
                    c.symbol,
                    c.feature,
                    c.horizon_secs,
                    curve.variant,
                    point.observation,
                    point.cumulative_bps
                ));
            }
        }
    }
    fs::write(output_dir.join("study.csv"), csv)?;
    fs::write(output_dir.join("curves.csv"), curves)?;
    fs::write(output_dir.join("daily_ic.csv"), daily_csv)?;
    fs::write(output_dir.join("regimes.csv"), regime_csv)?;
    if !result.events.is_empty() {
        let mut events = String::from("series,offset,mean_bps,t,count\n");
        for study in &result.events {
            for point in &study.points {
                events.push_str(&format!(
                    "{},{},{:.4},{:.2},{}\n",
                    study.series, point.offset, point.mean_bps, point.t, point.count
                ));
            }
        }
        fs::write(output_dir.join("events.csv"), events)?;
    }
    Ok(result)
}

/// The accepted features and the targets, one row per bar per symbol, for model fitting outside
/// the study: `symbol`, `time_us` (the bar's close instant, microseconds UTC), `date`, one
/// Float64 column per accepted expression, and `target_<horizon>` per horizon holding what the
/// study scores that bar against (the target `decision_delay_bars` later, over the horizon).
/// Bars where an accepted feature is not yet defined (warm-up) are left out; a target that
/// runs past the data is NaN. Returns the row count.
fn write_accepted_frame(
    path: &Path,
    config: &StudyConfig,
    grid: Grid,
    series: &[SymbolSeries],
    targets: &[Vec<Vec<f64>>],
) -> Result<usize> {
    use polars::prelude::*;
    let mut symbols: Vec<String> = Vec::new();
    let mut times: Vec<i64> = Vec::new();
    let mut dates: Vec<String> = Vec::new();
    let mut accepted: Vec<Vec<f64>> = vec![Vec::new(); config.accepted.len()];
    let mut forward: Vec<Vec<f64>> = vec![Vec::new(); config.horizons.len()];
    for (s, symbol_targets) in series.iter().zip(targets) {
        for i in 0..s.bars {
            let defined = s
                .accepted
                .iter()
                .all(|a| a.get(i).is_some_and(|v| v.is_finite()));
            if !defined {
                continue;
            }
            symbols.push(s.symbol.clone());
            times.push(s.keys[i]);
            dates.push(s.dates[i].to_string());
            for (column, values) in accepted.iter_mut().zip(&s.accepted) {
                column.push(values[i]);
            }
            for (column, values) in forward.iter_mut().zip(symbol_targets) {
                column.push(
                    values
                        .get(i + config.decision_delay_bars)
                        .copied()
                        .unwrap_or(f64::NAN),
                );
            }
        }
    }
    let symbols_len = symbols.len();
    let mut columns = vec![
        Column::new("symbol".into(), symbols),
        Column::new("time_us".into(), times),
        Column::new("date".into(), dates),
    ];
    let mut seen: HashSet<String> = HashSet::new();
    for (name, values) in config.accepted.iter().zip(accepted) {
        if seen.insert(name.clone()) {
            columns.push(Column::new(name.as_str().into(), values));
        }
    }
    for (&horizon, values) in config.horizons.iter().zip(forward) {
        let name = format!("target_{}", grid.horizon_label(horizon));
        if seen.insert(name.clone()) {
            columns.push(Column::new(name.as_str().into(), values));
        }
    }
    let mut frame = DataFrame::new(symbols_len, columns)?;
    let file = fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
    ParquetWriter::new(file).finish(&mut frame)?;
    Ok(frame.height())
}

/// A compact text table of the pooled (or single-symbol) results.
pub fn summary_table(result: &StudyResult) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let symbol = if result.symbols.len() > 1 {
        "ALL"
    } else {
        result.symbols[0].symbol.as_str()
    };
    let unit = result.target.unit();
    let _ = writeln!(
        out,
        "grid: {} · mode: {} · target: {} ({unit})",
        result.grid,
        result.mode.name(),
        result.target.name()
    );
    if !result.series.is_empty() {
        let _ = writeln!(out, "series: {}", result.series.join(", "));
    }
    for skipped in &result.unavailable {
        let _ = writeln!(out, "{}: {}", skipped.feature, skipped.reason);
    }
    if let Some(file) = &result.accepted_export {
        let _ = writeln!(
            out,
            "accepted set: {} → {file}",
            result.config.accepted.join(", ")
        );
    }
    for study in &result.events {
        let _ = writeln!(
            out,
            "event study {}: {} events, ±{} bars",
            study.series, study.events, study.window
        );
        for point in study.points.iter().filter(|p| p.offset % 5 == 0) {
            let _ = writeln!(
                out,
                "  {:>+4}  {:>+9.2} bps  t {:>+6.1}  n {}",
                point.offset, point.mean_bps, point.t, point.count
            );
        }
    }
    let _ = writeln!(
        out,
        "{:<16} {:>8} {:>10} {:>9} {:>9} {:>12} {:>8} {:>8} {:>10}",
        "feature",
        "horizon",
        "obs",
        "IC",
        "incr IC",
        format!("top-bot {unit}"),
        "t",
        "sharpe",
        "brkeven"
    );
    let horizon_text = |c: &StudyCell| match result.grid.as_str() {
        "daily" => format!("{}d", c.horizon_bars),
        "5m" => format!("{}m", c.horizon_bars * 5),
        "1m" => format!("{}m", c.horizon_bars),
        _ => format!("{}s", c.horizon_secs),
    };
    for c in result.cells.iter().filter(|c| c.symbol == symbol) {
        let incremental = c
            .diagnostics
            .as_ref()
            .and_then(|d| d.incremental_ic)
            .map(|v| format!("{v:>+9.4}"))
            .unwrap_or_else(|| format!("{:>9}", "-"));
        let _ = writeln!(
            out,
            "{:<16} {:>8} {:>10} {:>+9.4} {} {:>+12.3} {:>+8.1} {:>+8.2} {:>+10.4}",
            c.feature,
            horizon_text(c),
            c.observations,
            c.ic,
            incremental,
            c.top_minus_bottom_bps,
            c.top_minus_bottom_t,
            c.sharpe,
            c.breakeven_bps
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spearman_is_rank_based() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0];
        let y = [10.0, 100.0, 1000.0, 1e4, 1e5];
        assert!((spearman(&x, &y) - 1.0).abs() < 1e-12);
        let z = [5.0, 4.0, 3.0, 2.0, 1.0];
        assert!((spearman(&x, &z) + 1.0).abs() < 1e-12);
        let ties = [1.0, 1.0, 2.0, 2.0];
        assert_eq!(ranks(&ties), vec![1.5, 1.5, 3.5, 3.5]);
    }

    /// Feature against the forward mid return, the pre-target behaviour.
    fn return_pairs(
        feature: &[f64],
        mids: &[f64],
        horizon: usize,
        delay: usize,
    ) -> (Vec<f64>, Vec<f64>) {
        let series = TargetSeries {
            mids: mids.to_vec(),
            ..TargetSeries::default()
        };
        pairs(
            feature,
            &target_series(&series, Target::Return, horizon),
            delay,
        )
    }

    #[test]
    fn forward_returns_respect_horizon_and_delay() {
        // Mid climbs 1 bp per bar; feature is the bar index.
        let mids: Vec<f64> = (0..20).map(|i| 100.0 * (1.0 + 1e-4 * i as f64)).collect();
        let feature: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let (xs, ys) = return_pairs(&feature, &mids, 5, 0);
        assert_eq!(xs.len(), 15);
        assert!(
            (ys[0] - 5.0).abs() < 1e-6,
            "5 bars ahead is ~5 bps, got {}",
            ys[0]
        );
        let (xd, _) = return_pairs(&feature, &mids, 5, 2);
        assert_eq!(xd.len(), 13, "delay consumes bars at the end");
    }

    #[test]
    fn every_target_computes_on_a_hand_fixture() {
        let mids = vec![100.0, 101.0, 99.0, 102.0, 102.0, f64::NAN, 103.0];
        let series = TargetSeries {
            spreads: (1..=7).map(|s| s as f64).collect(),
            microprices: mids.iter().map(|m| m - 0.5).collect(),
            fair_values: vec![100.0; 7],
            mids,
        };
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let h = 2;
        let ret = target_series(&series, Target::Return, h);
        assert!(close(ret[0], -100.0), "99/100 is -100 bps, got {}", ret[0]);
        assert!(close(ret[2], (102.0 / 99.0 - 1.0) * 1e4));
        assert!(
            ret[3].is_nan() && ret[5].is_nan() && ret[6].is_nan(),
            "gap or end of data"
        );
        let abs = target_series(&series, Target::AbsMove, h);
        assert!(close(abs[0], 100.0) && close(abs[2], ret[2]));
        let rv = target_series(&series, Target::RealizedVariance, h);
        let r1 = (101.0f64 / 100.0 - 1.0) * 1e4;
        let r2 = (99.0f64 / 101.0 - 1.0) * 1e4;
        assert!(
            close(rv[0], r1 * r1 + r2 * r2),
            "bars 1 and 2, got {}",
            rv[0]
        );
        assert!(rv[3].is_nan(), "a gap inside the window");
        assert!(rv[4].is_nan() && rv[5].is_nan() && rv[6].is_nan());
        let spread = target_series(&series, Target::SpreadChange, h);
        assert!(close(spread[0], 2.0) && close(spread[4], 2.0));
        let fair = target_series(&series, Target::FairValueResidual, h);
        assert!(close(fair[0], (99.0 - 100.0) / 99.0 * 1e4));
        let micro = target_series(&series, Target::MicropriceResidual, h);
        assert!(close(micro[0], 0.5 / 99.0 * 1e4));
        assert!(
            fair[3].is_nan() && micro[3].is_nan(),
            "no book at the horizon"
        );
        // Zero horizon: returns are zero, variance has no bars.
        assert!(close(target_series(&series, Target::Return, 0)[0], 0.0));
        assert!(target_series(&series, Target::RealizedVariance, 0)[0].is_nan());
        // Names round-trip; unknown names list the choices.
        for target in Target::ALL {
            assert_eq!(Target::parse(target.name()).unwrap(), target);
        }
        let err = Target::parse("variance").unwrap_err();
        assert!(format!("{err:#}").contains("realized_variance"));
        assert_eq!(Target::RealizedVariance.unit(), "bps²");
        assert_eq!(Target::default(), Target::Return);
        let config: StudyConfig =
            toml::from_str("lake_dir = \"x\"\nsymbols = [\"A:B\"]\ntarget = \"spread_change\"")
                .unwrap();
        assert_eq!(config.target, Target::SpreadChange);
    }

    #[test]
    fn fair_value_ema_carries_across_gaps_and_tracks_the_mid() {
        let bars = synthetic_grid();
        let series = TargetSeries::from_bars(&bars, Grid::Lake { step_secs: 1 });
        assert_eq!(series.fair_values.len(), bars.len());
        let with_book: Vec<usize> = (0..bars.len())
            .filter(|&i| bars[i].book.is_some())
            .collect();
        assert!(
            series.fair_values[with_book[0]] == series.mids[with_book[0]],
            "seeded at the first mid"
        );
        assert!(
            series.fair_values[97].is_nan() && series.mids[97].is_nan(),
            "no book, no fair value"
        );
        // A slow average stays inside the mid's range and lags it.
        let last = *with_book.last().unwrap();
        let window: Vec<f64> = with_book
            .iter()
            .rev()
            .take(200)
            .map(|&i| series.mids[i])
            .collect();
        let lo = window.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = window.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!(series.fair_values[last] >= lo && series.fair_values[last] <= hi);
    }

    /// The done line: on the SOL-like fixture (volatility regimes that widen the spread), the
    /// spread predicts realized variance but not the signed return.
    #[test]
    fn spread_predicts_realized_variance_on_the_sol_fixture() {
        let bars = synthetic_grid();
        let series = TargetSeries::from_bars(&bars, Grid::Lake { step_secs: 1 });
        let (_, features, _) =
            evaluate_features(&["spread_bps".to_owned()], 1, &bars, true).unwrap();
        let spread = &features["spread_bps"];
        let (xs, ys) = pairs(
            spread,
            &target_series(&series, Target::RealizedVariance, 30),
            1,
        );
        assert!(xs.len() > 2_000);
        let ic = spearman(&xs, &ys);
        assert!(
            ic > 0.3,
            "spread vs realized variance IC should be clearly positive, got {ic}"
        );
        let (xr, yr) = pairs(spread, &target_series(&series, Target::Return, 30), 1);
        assert!(
            spearman(&xr, &yr).abs() < 0.1,
            "the spread says nothing about direction"
        );
        // The cell carries the same IC and its curves run on the target.
        let c = cell(
            "SOL",
            "spread_bps",
            30,
            Grid::Lake { step_secs: 1 },
            10,
            &xs,
            &ys,
        );
        assert!((c.ic - ic).abs() < 1e-12 && c.curves[0].curve.len() > 0);
    }

    /// Daily OHLCV bars with no book, as the CSV loaders produce them.
    fn daily_bars(rows: &[(f64, f64, f64, f64, f64)]) -> Vec<lake::LakeBar> {
        rows.iter()
            .enumerate()
            .map(|(i, &(open, high, low, close, volume))| lake::LakeBar {
                date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap() + chrono::Days::new(i as u64),
                time: chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap(),
                open,
                high,
                low,
                close,
                volume,
                book: None,
            })
            .collect()
    }

    #[test]
    fn ohlcv_bases_compute_on_csv_bars() {
        let bars = daily_bars(&[
            (100.0, 101.0, 99.0, 100.0, 1000.0),
            (100.5, 103.0, 100.0, 102.0, 1100.0),
            (101.0, 102.0, 100.0, 101.0, 900.0),
            (101.5, 105.0, 101.0, 104.0, 1200.0),
            (103.0, 104.0, 102.0, 103.0, 1000.0),
        ]);
        let names: Vec<String> = [
            "return_1",
            "return_2",
            "range_bps",
            "gap_bps",
            "high_3_distance",
            "volume | zscore 3",
        ]
        .iter()
        .map(|f| (*f).to_owned())
        .collect();
        let (mids, features, with_book) = evaluate_features(&names, 86_400, &bars, false).unwrap();
        assert_eq!(with_book, 0);
        assert!(mids.iter().all(|m| m.is_nan()), "no book, no mid series");
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        let r1 = &features["return_1"];
        assert!(r1[0].is_nan() && close(r1[1], 200.0) && close(r1[2], (101.0 / 102.0 - 1.0) * 1e4));
        let r2 = &features["return_2"];
        assert!(r2[1].is_nan() && close(r2[2], 100.0) && close(r2[3], (104.0 / 102.0 - 1.0) * 1e4));
        let range = &features["range_bps"];
        assert!(close(range[0], 200.0) && close(range[1], 3.0 / 102.0 * 1e4));
        let gap = &features["gap_bps"];
        assert!(
            gap[0].is_nan() && close(gap[1], 50.0) && close(gap[4], (103.0 / 104.0 - 1.0) * 1e4)
        );
        let high = &features["high_3_distance"];
        assert!(
            high[1].is_nan()
                && close(high[2], (101.0 / 103.0 - 1.0) * 1e4)
                && close(high[3], (104.0 / 105.0 - 1.0) * 1e4)
        );
        let vz = &features["volume | zscore 3"];
        assert!(
            vz[1].is_nan() && vz[2].is_finite() && vz[2] < 0.0,
            "900 is below the 3-bar mean"
        );
        // The close stands in for the mid on an OHLCV grid.
        let series = TargetSeries::from_bars(&bars, Grid::Daily);
        assert_eq!(series.mids, vec![100.0, 102.0, 101.0, 104.0, 103.0]);
        assert!(series.spreads.iter().all(|s| s.is_nan()));
        assert!(close(target_series(&series, Target::Return, 1)[0], 200.0));
        // Grammar: windows must be positive, and book-only bases are known as such.
        assert!(feature_expr::parse("return_0").is_err());
        assert!(feature_expr::parse("high_252_distance | zscore 20").is_ok());
        assert!(!feature_expr::needs_book(
            &feature_expr::parse("return_5 | times volume").unwrap()
        ));
        assert!(feature_expr::needs_book(
            &feature_expr::parse("volume | times obi_l1").unwrap()
        ));
        assert!(feature_expr::needs_book(
            &feature_expr::parse("spread_bps").unwrap()
        ));
        assert_eq!(Grid::parse(Some("daily"), 1).unwrap(), Grid::Daily);
        assert_eq!(Grid::parse(None, 5).unwrap(), Grid::Lake { step_secs: 5 });
        assert!(Grid::parse(Some("weekly"), 1).is_err());
        assert_eq!(Grid::Daily.horizon_label(5), "5d");
        assert_eq!(Grid::FiveMinute.horizon_label(3), "15m");
    }

    /// The done line: a daily study of `return_1 | zscore 20` on examples/data runs end to end
    /// through the CSV loader, and an in-memory panel built the same way gives the same IC.
    #[test]
    fn daily_study_matches_an_in_memory_panel() {
        let data = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/examples/data/eod"));
        let config = StudyConfig {
            lake_dir: PathBuf::new(),
            symbols: vec!["DEMO.US".to_owned()],
            step_secs: 1,
            resolution: Some("daily".to_owned()),
            daily_dir: data.to_path_buf(),
            five_minute_dir: PathBuf::new(),
            one_minute_dir: PathBuf::new(),
            calendar_symbol: Some("DEMO.US".to_owned()),
            session: SessionKind::Regular,
            features: vec!["return_1 | zscore 20".to_owned(), "obi_l1".to_owned()],
            horizons: vec![1, 5],
            decision_delay_bars: 1,
            buckets: 10,
            target: Target::Return,
            series: Vec::new(),
            lake_series: true,
            mode: StudyMode::TimeSeries,
            intraday_source: None,
            event_window: 20,
            accepted: Vec::new(),
        };
        let start = NaiveDate::from_ymd_opt(2019, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2025, 12, 31).unwrap();
        let out = std::env::temp_dir().join(format!("tessera-wb04-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out);
        let result = run(&config, start, end, &out.join("csv")).unwrap();
        assert_eq!(result.grid, "daily");
        assert_eq!(result.unavailable.len(), 1);
        assert_eq!(result.unavailable[0].feature, "obi_l1");
        assert!(
            result.unavailable[0]
                .reason
                .contains("unavailable on this grid")
        );
        assert!(
            result
                .cells
                .iter()
                .all(|c| c.feature == "return_1 | zscore 20")
        );
        let csv_cell = |h: usize| result.cells.iter().find(|c| c.horizon_bars == h).unwrap();
        assert!(csv_cell(1).observations > 1_000 && csv_cell(1).horizon_secs == 86_400);
        assert!(csv_cell(1).ic.is_finite() && csv_cell(5).ic.is_finite());
        let summary = summary_table(&result);
        assert!(summary.contains("grid: daily") && summary.contains("obi_l1: unavailable"));

        // The same panel in memory: the SDK's daily loader, sanitized on the same calendar.
        let mut bars = load_daily(&data.join("DEMO.US.csv")).unwrap();
        let calendar: BTreeSet<NaiveDate> = bars.iter().map(|b| b.date).collect();
        sanitize_daily(&mut bars, Some(&calendar)).unwrap();
        bars.retain(|b| b.date >= start && b.date <= end);
        let panel: Vec<lake::LakeBar> = bars.iter().map(lake_bar).collect();
        assert_eq!(panel.len(), result.symbols[0].bars);
        let again = run_on_panel(
            &config,
            vec![("DEMO.US".to_owned(), panel.clone())],
            start,
            end,
            &out.join("panel"),
        )
        .unwrap();
        for h in [1, 5] {
            let a = csv_cell(h);
            let b = again.cells.iter().find(|c| c.horizon_bars == h).unwrap();
            assert_eq!(a.ic, b.ic, "IC at horizon {h}");
            assert_eq!(a.observations, b.observations);
        }
        // And straight from the evaluator, without the study machinery.
        let (_, features, _) =
            evaluate_features(&["return_1 | zscore 20".to_owned()], 86_400, &panel, false).unwrap();
        let series = TargetSeries::from_bars(&panel, Grid::Daily);
        let (xs, ys) = pairs(
            &features["return_1 | zscore 20"],
            &target_series(&series, Target::Return, 5),
            1,
        );
        assert_eq!(spearman(&xs, &ys), csv_cell(5).ic);
        let _ = fs::remove_dir_all(&out);
    }

    /// A series joins a daily panel as-of availability: the bar before the release cannot see
    /// the value, the bar after can, and expressions read it like any other base.
    #[test]
    fn series_join_as_of_availability_on_a_daily_panel() {
        let bars = daily_bars(&[
            (100.0, 101.0, 99.0, 100.0, 1000.0),
            (100.5, 103.0, 100.0, 102.0, 1100.0),
            (101.0, 102.0, 100.0, 101.0, 900.0),
            (101.5, 105.0, 101.0, 104.0, 1200.0),
            (103.0, 104.0, 102.0, 103.0, 1000.0),
            (103.0, 104.0, 102.0, 103.5, 1000.0),
        ]);
        // Bars are 2024-01-01 .. 2024-01-06; the value is for the 3rd, published on the 5th.
        let published = NaiveDate::from_ymd_opt(2024, 1, 5)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_micros();
        let macro_series = Series::new(
            "macro",
            SeriesKind::Level,
            vec![crate::series::Observation {
                nominal_us: published - 2 * 86_400 * 1_000_000,
                available_us: published,
                symbol: None,
                value: 7.0,
            }],
        );
        let aligned = align_series(
            Grid::Daily,
            "DEMO.US",
            &bars,
            std::slice::from_ref(&macro_series),
        );
        assert_eq!(aligned.len(), 1);
        assert!(
            aligned[0][2].is_nan() && aligned[0][3].is_nan(),
            "the 3rd and 4th cannot see it"
        );
        assert_eq!(aligned[0][4], 7.0, "the 5th's close can");
        assert_eq!(aligned[0][5], 7.0, "and it carries forward");
        // Daily bars close at 16:00 New York, after a midnight-UTC release the same day.
        let close = bar_close_us(&bars[4], Grid::Daily);
        assert!(close > published && close - published == 21 * 3_600 * 1_000_000);
        let names = vec!["macro".to_owned()];
        let (_, features, _) = evaluate_features_with(
            &["macro".to_owned(), "macro | times close".to_owned()],
            86_400,
            &bars,
            false,
            &names,
            &aligned,
        )
        .unwrap();
        assert!(features["macro"][3].is_nan() && features["macro"][4] == 7.0);
        assert_eq!(features["macro | times close"][4], 7.0 * 103.0);
        assert!(
            feature_expr::parse("macro").is_err(),
            "unregistered names still fail"
        );
        assert!(feature_expr::parse_with("macro | zscore 3", &names).is_ok());
        assert!(!feature_expr::needs_book(
            &feature_expr::parse_with("macro", &names).unwrap()
        ));
    }

    /// The done line's second half: a funding rate registered for a lake instrument is a base
    /// in a SOL study and produces cells.
    #[test]
    fn funding_rate_is_a_base_in_a_sol_study() {
        let bars = synthetic_grid();
        let grid = Grid::Lake { step_secs: 1 };
        // Funding ticks every 300 bars, received 5 s after its event time, for this symbol.
        let observations: Vec<crate::series::Observation> = (0..bars.len())
            .step_by(300)
            .enumerate()
            .map(|(k, i)| {
                let event_us = bar_close_us(&bars[i], grid) - 1_000_000;
                crate::series::Observation {
                    nominal_us: event_us,
                    available_us: event_us + 5_000_000,
                    symbol: Some("BINANCE_FUTURES:SOLUSDT".to_owned()),
                    value: 1e-4 * (if k % 2 == 0 { 1.0 } else { -1.0 }),
                }
            })
            .collect();
        let funding = Series::new("funding_rate", SeriesKind::Level, observations);
        let config = StudyConfig {
            lake_dir: PathBuf::new(),
            symbols: vec!["BINANCE_FUTURES:SOLUSDT".to_owned()],
            step_secs: 1,
            resolution: None,
            daily_dir: PathBuf::new(),
            five_minute_dir: PathBuf::new(),
            one_minute_dir: PathBuf::new(),
            calendar_symbol: None,
            session: SessionKind::Regular,
            features: vec![
                "funding_rate".to_owned(),
                "funding_rate | diff 1".to_owned(),
            ],
            horizons: vec![5],
            decision_delay_bars: 1,
            buckets: 10,
            target: Target::Return,
            series: Vec::new(),
            lake_series: false,
            mode: StudyMode::TimeSeries,
            intraday_source: None,
            event_window: 20,
            accepted: Vec::new(),
        };
        let out = std::env::temp_dir().join(format!("tessera-wb05-study-{}", std::process::id()));
        let result = run_on_panel_with_series(
            &config,
            vec![("BINANCE_FUTURES:SOLUSDT".to_owned(), bars.clone())],
            vec![funding],
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            &out,
        )
        .unwrap();
        assert_eq!(result.series, vec!["funding_rate"]);
        let cell = result
            .cells
            .iter()
            .find(|c| c.feature == "funding_rate")
            .unwrap();
        assert!(cell.observations > 2_000 && cell.ic.is_finite(), "{cell:?}");
        assert!(summary_table(&result).contains("series: funding_rate"));
        // A different symbol sees nothing of it: every value NaN, so no cell.
        let other = run_on_panel_with_series(
            &config,
            vec![("PARADEX:SOL-USD-PERP".to_owned(), bars.clone())],
            vec![Series::new(
                "funding_rate",
                SeriesKind::Level,
                result_observations_for_binance(),
            )],
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            &out.join("other"),
        )
        .unwrap();
        assert!(other.cells.is_empty());
        let _ = fs::remove_dir_all(&out);
    }

    /// Two sessions of 1-minute bars with a deterministic walk.
    fn minute_bars(days: u32, per_day: u32) -> Vec<lake::LakeBar> {
        let mut seed: u64 = 0x1234_5678;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let mut close = 100.0;
        let mut bars = Vec::new();
        for day in 0..days {
            for minute in 0..per_day {
                close *= 1.0 + (rand() - 0.5) * 4e-3;
                bars.push(lake::LakeBar {
                    date: NaiveDate::from_ymd_opt(2024, 3, 4).unwrap()
                        + chrono::Days::new(u64::from(day)),
                    time: chrono::NaiveTime::from_hms_opt(9, 30, 0).unwrap()
                        + chrono::Duration::minutes(i64::from(minute)),
                    open: close,
                    high: close * 1.001,
                    low: close * 0.999,
                    close,
                    volume: 100.0 + rand() * 50.0,
                    book: None,
                });
            }
        }
        bars
    }

    /// The done line, first half: `return_1 | agg daily realized_var` on 1-minute bars equals
    /// the sum of squared minute returns of each session, computed directly, and lifts onto
    /// a daily panel by date; `sum`, `mean`, and `last` fold the same way.
    #[test]
    fn daily_realized_variance_from_minute_bars_matches_a_direct_computation() {
        let bars = minute_bars(3, 390);
        let names = [
            "return_1 | agg daily realized_var",
            "volume | agg daily sum",
            "volume | agg daily mean",
            "close | agg daily last",
        ]
        .map(str::to_owned);
        let (_, features, _) = evaluate_features(&names, 60, &bars, false).unwrap();
        let lifted = lift_to_daily(&bars, &features["return_1 | agg daily realized_var"]);
        assert_eq!(lifted.len(), 3);
        for day in 0..3 {
            let session: Vec<&lake::LakeBar> = bars.iter().skip(day * 390).take(390).collect();
            // Direct: squared bar-to-bar returns within the session, skipping the first bar
            // of the day (its return crosses the overnight, and `return_1` on a fresh
            // session still has a previous close, so both sides include it consistently).
            let direct: f64 = (0..390)
                .filter_map(|i| {
                    let previous = if i == 0 {
                        if day == 0 {
                            return None;
                        }
                        bars[day * 390 - 1].close
                    } else {
                        session[i - 1].close
                    };
                    let r = (session[i].close / previous - 1.0) * 1e4;
                    Some(r * r)
                })
                .sum();
            let date = session[0].date;
            assert!(
                (lifted[&date] - direct).abs() < 1e-6 * direct.max(1.0),
                "day {day}: lifted {} vs direct {direct}",
                lifted[&date]
            );
            let volumes: Vec<f64> = session.iter().map(|b| b.volume).collect();
            let sum = lift_to_daily(&bars, &features["volume | agg daily sum"])[&date];
            let mean = lift_to_daily(&bars, &features["volume | agg daily mean"])[&date];
            let last = lift_to_daily(&bars, &features["close | agg daily last"])[&date];
            assert!((sum - volumes.iter().sum::<f64>()).abs() < 1e-6);
            assert!((mean - volumes.iter().sum::<f64>() / 390.0).abs() < 1e-6);
            assert_eq!(last, session[389].close);
        }
        // Mid-session the fold is the day so far: the first bar of day two holds one term.
        let rv = &features["return_1 | agg daily realized_var"];
        let r = (bars[390].close / bars[389].close - 1.0) * 1e4;
        assert!((rv[390] - r * r).abs() < 1e-9);
        // Grammar and the daily-grid rule.
        let expr = feature_expr::parse("return_1 | agg daily realized_var | zscore 20").unwrap();
        assert!(feature_expr::has_daily_agg(&expr));
        assert_eq!(
            expr.to_string(),
            "return_1 | agg daily realized_var | zscore 20"
        );
        assert!(feature_expr::parse("volume | agg weekly sum").is_err());
        assert!(feature_expr::parse("volume | agg daily median").is_err());
        assert!(!feature_expr::has_daily_agg(
            &feature_expr::parse("volume | zscore 20").unwrap()
        ));
    }

    /// The done line, second half: events followed by a known drift reproduce it in the
    /// event-study path, and nothing shows before them.
    #[test]
    fn event_study_reproduces_a_known_post_event_drift() {
        let days = 600;
        let mut seed: u64 = 0xabcd;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let mut panel = Vec::new();
        let mut observations = Vec::new();
        for k in 0..3 {
            let name = format!("E{k}.US");
            let mut close = 100.0;
            let mut rows = Vec::with_capacity(days);
            for d in 0..days {
                // Noise of ~10 bps; +5 bps a bar for the ten bars after each event (every
                // 50 bars from bar 40).
                let since_event = (d + 10) % 50;
                let drift = if d >= 40 && (1..=10).contains(&since_event) {
                    5e-4
                } else {
                    0.0
                };
                close *= 1.0 + drift + (rand() - 0.5) * 2e-3 * 1.7;
                rows.push((close, close * 1.001, close * 0.999, close, 1000.0));
            }
            let bars = daily_bars(&rows);
            for d in (40..days).step_by(50) {
                observations.push(crate::series::Observation {
                    nominal_us: bar_close_us(&bars[d], Grid::Daily),
                    available_us: bar_close_us(&bars[d], Grid::Daily),
                    symbol: Some(name.clone()),
                    value: 1.0,
                });
            }
            panel.push((name, bars));
        }
        let announcements = Series::new("announcement", SeriesKind::Event, observations);
        let study = event_study(Grid::Daily, &panel, &announcements, 20);
        assert_eq!(study.series, "announcement");
        assert_eq!(study.points.len(), 41);
        assert!(study.events >= 30, "{} events", study.events);
        let at = |offset: i64| study.points.iter().find(|p| p.offset == offset).unwrap();
        assert_eq!(at(0).mean_bps, 0.0, "normalised to the event bar");
        assert!(
            at(-10).mean_bps.abs() < 20.0 && at(-10).t.abs() < 2.5,
            "no run-up: {:?}",
            at(-10)
        );
        let after = at(10);
        assert!(
            (after.mean_bps - 50.0).abs() < 20.0,
            "ten bars of +5 bps: {after:?}"
        );
        assert!(after.t > 5.0 && after.count == study.events);
        assert!(
            (at(20).mean_bps - 50.0).abs() < 25.0,
            "the drift stops: {:?}",
            at(20)
        );
        // The study carries it when the series is registered.
        let config = StudyConfig {
            lake_dir: PathBuf::new(),
            symbols: panel.iter().map(|(s, _)| s.clone()).collect(),
            step_secs: 1,
            resolution: Some("daily".to_owned()),
            daily_dir: PathBuf::new(),
            five_minute_dir: PathBuf::new(),
            one_minute_dir: PathBuf::new(),
            calendar_symbol: None,
            session: SessionKind::Regular,
            features: vec!["announcement".to_owned()],
            horizons: vec![5],
            decision_delay_bars: 0,
            buckets: 10,
            target: Target::Return,
            series: Vec::new(),
            lake_series: false,
            mode: StudyMode::TimeSeries,
            intraday_source: None,
            event_window: 20,
            accepted: Vec::new(),
        };
        let out = std::env::temp_dir().join(format!("tessera-wb07-{}", std::process::id()));
        let result = run_on_panel_with_series(
            &config,
            panel,
            vec![announcements],
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2025, 12, 31).unwrap(),
            &out,
        )
        .unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].events, study.events);
        assert!(
            fs::read_to_string(out.join("events.csv"))
                .unwrap()
                .starts_with("series,offset")
        );
        assert!(summary_table(&result).contains("event study announcement"));
        let _ = fs::remove_dir_all(&out);
    }

    /// The done line: on a panel where every symbol's feature is its own next-day return (an
    /// oracle series available at each bar's close), the cross-section ranks symbols
    /// perfectly every date, so the mean IC is 1 and the long-short curve only climbs.
    #[test]
    fn cross_sectional_oracle_gives_ic_one_and_a_monotone_curve() {
        let symbols = 30;
        let days = 300;
        let mut seed: u64 = 0xc5;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let mut panel = Vec::new();
        let mut oracle = Vec::new();
        for k in 0..symbols {
            let name = format!("S{k:02}.US");
            let mut close = 50.0 + k as f64;
            let mut rows = Vec::with_capacity(days);
            for _ in 0..days {
                close *= 1.0 + (rand() - 0.5) * 0.04;
                rows.push((close, close * 1.01, close * 0.99, close, 1000.0));
            }
            let bars = daily_bars(&rows);
            for d in 0..days - 1 {
                // The next day's return plus a whisker of noise (a bp against moves of
                // hundreds), so the daily ICs sit near 1 rather than exactly on it.
                let next_return = (bars[d + 1].close / bars[d].close - 1.0) * 1e4;
                oracle.push(crate::series::Observation {
                    nominal_us: bar_close_us(&bars[d], Grid::Daily),
                    available_us: bar_close_us(&bars[d], Grid::Daily),
                    symbol: Some(name.clone()),
                    value: next_return + (rand() - 0.5) * 2.0,
                });
            }
            panel.push((name, bars));
        }
        let oracle = Series::new("oracle", SeriesKind::Level, oracle);
        let config = StudyConfig {
            lake_dir: PathBuf::new(),
            symbols: panel.iter().map(|(s, _)| s.clone()).collect(),
            step_secs: 1,
            resolution: Some("daily".to_owned()),
            daily_dir: PathBuf::new(),
            five_minute_dir: PathBuf::new(),
            one_minute_dir: PathBuf::new(),
            calendar_symbol: None,
            session: SessionKind::Regular,
            features: vec!["oracle".to_owned()],
            horizons: vec![1],
            decision_delay_bars: 0,
            buckets: 10,
            target: Target::Return,
            series: Vec::new(),
            lake_series: false,
            mode: StudyMode::CrossSectional,
            intraday_source: None,
            event_window: 20,
            accepted: Vec::new(),
        };
        let out = std::env::temp_dir().join(format!("tessera-wb06-{}", std::process::id()));
        let start = NaiveDate::from_ymd_opt(2024, 1, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2024, 12, 31).unwrap();
        let result = run_on_panel_with_series(
            &config,
            panel.clone(),
            vec![oracle.clone()],
            start,
            end,
            &out.join("xs"),
        )
        .unwrap();
        assert_eq!(result.mode, StudyMode::CrossSectional);
        assert_eq!(
            result.cells.len(),
            1,
            "one pooled cell, no per-symbol cells"
        );
        let c = &result.cells[0];
        assert_eq!(c.symbol, "ALL");
        assert!(c.ic > 0.99 && c.ic < 1.0, "mean daily IC {}", c.ic);
        let cross = c.cross_section.as_ref().unwrap();
        assert_eq!(cross.dates, days - 1);
        assert!((cross.symbols_per_date - symbols as f64).abs() < 1e-9);
        assert!(cross.ic_t > 50.0 && cross.ic_positive_share == 1.0);
        assert!(c.top_minus_bottom_bps > 0.0 && c.sharpe > 0.0);
        assert!(c.turnover > 0.0 && c.turnover <= 4.0 && c.breakeven_bps > 0.0);
        assert_eq!(c.curves.len(), 1);
        assert_eq!(c.curves[0].variant, "long_short");
        let points = &c.curves[0].curve;
        assert!(points.len() == days - 1);
        assert!(
            points
                .windows(2)
                .all(|w| w[1].cumulative_bps > w[0].cumulative_bps),
            "the long-short curve climbs every date"
        );
        assert_eq!(c.buckets.len(), 10);
        assert!(c.buckets[9].forward_bps > c.buckets[0].forward_bps);
        assert!(summary_table(&result).contains("mode: cross_sectional"));
        // The same panel along time also sees the oracle, per symbol and pooled.
        let ts = run_on_panel_with_series(
            &StudyConfig {
                mode: StudyMode::TimeSeries,
                ..config.clone()
            },
            panel,
            vec![oracle],
            start,
            end,
            &out.join("ts"),
        )
        .unwrap();
        assert_eq!(ts.cells.len(), symbols + 1);
        assert!(ts.cells.iter().all(|c| c.ic > 0.99));
        assert_eq!(
            StudyMode::parse("cross-sectional").unwrap(),
            StudyMode::CrossSectional
        );
        assert_eq!(
            StudyMode::parse("time_series").unwrap(),
            StudyMode::TimeSeries
        );
        assert!(StudyMode::parse("panel").is_err());
        let _ = fs::remove_dir_all(&out);
    }

    fn result_observations_for_binance() -> Vec<crate::series::Observation> {
        vec![crate::series::Observation {
            nominal_us: 0,
            available_us: 0,
            symbol: Some("BINANCE_FUTURES:SOLUSDT".to_owned()),
            value: 1e-4,
        }]
    }

    /// The done line: a feature that is a linear copy of an accepted one keeps its plain IC
    /// but has no incremental IC once the accepted set is regressed out; an unrelated
    /// feature keeps its IC. Daily ICs, regimes, and autocorrelation come along.
    #[test]
    fn linear_copy_of_an_accepted_feature_has_no_incremental_ic() {
        let bars = synthetic_grid_days(2);
        let config = StudyConfig {
            lake_dir: PathBuf::new(),
            symbols: vec!["BINANCE_FUTURES:SOLUSDT".to_owned()],
            step_secs: 1,
            resolution: None,
            daily_dir: PathBuf::new(),
            five_minute_dir: PathBuf::new(),
            one_minute_dir: PathBuf::new(),
            calendar_symbol: None,
            session: SessionKind::Regular,
            // clip -1 1 leaves obi_l1 untouched: an exact copy of the accepted feature.
            features: vec!["spread_bps | clip 0 1000".to_owned(), "obi_l5".to_owned()],
            horizons: vec![30],
            decision_delay_bars: 1,
            buckets: 10,
            target: Target::RealizedVariance,
            series: Vec::new(),
            lake_series: false,
            mode: StudyMode::TimeSeries,
            intraday_source: None,
            event_window: 20,
            accepted: vec!["spread_bps".to_owned()],
        };
        let out = std::env::temp_dir().join(format!("tessera-wb08-{}", std::process::id()));
        let result = run_on_panel_with_series(
            &config,
            vec![("BINANCE_FUTURES:SOLUSDT".to_owned(), bars)],
            Vec::new(),
            NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 11).unwrap(),
            &out,
        )
        .unwrap();
        let copy = result
            .cells
            .iter()
            .find(|c| c.feature == "spread_bps | clip 0 1000")
            .unwrap();
        let d = copy.diagnostics.as_ref().unwrap();
        assert!(
            copy.ic > 0.3,
            "the copy keeps the spread's edge: {}",
            copy.ic
        );
        assert_eq!(d.accepted, vec!["spread_bps"]);
        let incremental = d.incremental_ic.unwrap();
        assert!(
            incremental.abs() < 0.02,
            "nothing left once spread_bps is regressed out: {incremental}"
        );
        let other = result.cells.iter().find(|c| c.feature == "obi_l5").unwrap();
        let od = other.diagnostics.as_ref().unwrap();
        // A random book imbalance says nothing about variance, before or after the spread
        // is regressed out (both sit within sampling noise of zero).
        assert!(
            other.ic.abs() < 0.05 && od.incremental_ic.unwrap().abs() < 0.05,
            "obi_l5 is unrelated: plain {} incremental {}",
            other.ic,
            od.incremental_ic.unwrap()
        );
        // Two dates, an IC each, and the sign count against the cell.
        assert_eq!(d.daily_ic.len(), 2);
        assert!(d.daily_ic.iter().all(|day| day.n > 1_000 && day.ic > 0.2));
        assert_eq!(d.sign_consistency, 1.0);
        // Spread and volatility terciles are populated; one hour of bars gives no hour rows.
        let spread_rows: Vec<&RegimeIc> =
            d.regimes.iter().filter(|r| r.regime == "spread").collect();
        let vol_rows: Vec<&RegimeIc> = d.regimes.iter().filter(|r| r.regime == "vol").collect();
        assert_eq!(spread_rows.len(), 3);
        assert_eq!(vol_rows.len(), 3);
        assert!(spread_rows.iter().all(|r| r.n > 1_000 && r.ic.is_finite()));
        assert!(d.regimes.iter().all(|r| r.regime != "hour"));
        // A regime-driven spread is persistent; the random book imbalance is not.
        assert!(
            d.autocorrelation_1 > 0.9,
            "spread autocorrelation {}",
            d.autocorrelation_1
        );
        assert!(
            od.autocorrelation_1.abs() < 0.1,
            "obi_l5 autocorrelation {}",
            od.autocorrelation_1
        );
        assert!(d.autocorrelation_horizon.is_finite());
        // The files and the summary carry it.
        assert!(
            fs::read_to_string(out.join("daily_ic.csv"))
                .unwrap()
                .lines()
                .count()
                >= 5
        );
        assert!(
            fs::read_to_string(out.join("regimes.csv"))
                .unwrap()
                .contains("spread,low")
        );
        assert!(summary_table(&result).contains("incr IC"));
        // OLS on the fixture directly: a copy leaves a zero residual.
        let x = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let r = ols_residual(&x, &[vec![2.0, 4.0, 6.0, 8.0, 10.0]]);
        assert!(r.iter().all(|v| v.abs() < 1e-9), "{r:?}");
        let r = ols_residual(&x, &[vec![1.0, 1.0, 1.0, 1.0, 1.0]]);
        assert!(
            (r[0] + 2.0).abs() < 1e-9,
            "a constant regressor falls back to demeaning: {r:?}"
        );
        let _ = fs::remove_dir_all(&out);
    }

    /// WB-09: with an accepted set, the study writes the accepted features and the targets to
    /// parquet for model fitting, and the file reads back through the `parquet-schema` path with
    /// the same columns and the values the study scored.
    #[test]
    fn accepted_export_round_trips_through_parquet() {
        let bars = synthetic_grid();
        let day = NaiveDate::from_ymd_opt(2026, 7, 10).unwrap();
        let mut config = StudyConfig {
            lake_dir: PathBuf::new(),
            symbols: vec!["BINANCE_FUTURES:SOLUSDT".to_owned()],
            step_secs: 1,
            resolution: None,
            daily_dir: PathBuf::new(),
            five_minute_dir: PathBuf::new(),
            one_minute_dir: PathBuf::new(),
            calendar_symbol: None,
            session: SessionKind::Regular,
            features: vec!["obi_l5".to_owned()],
            horizons: vec![5, 30],
            decision_delay_bars: 1,
            buckets: 10,
            target: Target::Return,
            series: Vec::new(),
            lake_series: false,
            mode: StudyMode::TimeSeries,
            intraday_source: None,
            event_window: 20,
            accepted: vec!["spread_bps".to_owned(), "obi_l1 | zscore 60".to_owned()],
        };
        let out = std::env::temp_dir().join(format!("tessera-wb09-{}", std::process::id()));
        let panel = || vec![("BINANCE_FUTURES:SOLUSDT".to_owned(), bars.clone())];
        let result =
            run_on_panel_with_series(&config, panel(), Vec::new(), day, day, &out).unwrap();
        assert_eq!(result.accepted_export.as_deref(), Some("accepted.parquet"));
        let path = out.join("accepted.parquet");
        let expected_columns = [
            "symbol",
            "time_us",
            "date",
            "spread_bps",
            "obi_l1 | zscore 60",
            "target_5s",
            "target_30s",
        ];
        // The schema as `tessera parquet-schema` prints it.
        let described = crate::lake::describe_parquet(&path, 3, None).unwrap();
        for column in expected_columns {
            assert!(
                described.contains(column),
                "{column} missing from:\n{described}"
            );
        }
        assert!(described.contains("target_5s: Float64"), "{described}");
        assert!(described.contains("time_us: Int64"), "{described}");
        // And the values: every bar where the accepted set is defined, the feature as the
        // study evaluated it, and the target the study scored that bar against.
        use polars::prelude::*;
        let frame = ParquetReader::new(fs::File::open(&path).unwrap())
            .finish()
            .unwrap();
        let names: Vec<&str> = frame
            .get_column_names()
            .iter()
            .map(|n| n.as_str())
            .collect();
        assert_eq!(names, expected_columns);
        let (_, values, _) = evaluate_features(&config.accepted, 1, &bars, true).unwrap();
        let spread = &values["spread_bps"];
        let z = &values["obi_l1 | zscore 60"];
        let exported: Vec<usize> = (0..bars.len())
            .filter(|&i| spread[i].is_finite() && z[i].is_finite())
            .collect();
        assert!(exported.len() > 1_000 && exported.len() < bars.len());
        assert_eq!(frame.height(), exported.len());
        let grid = Grid::Lake { step_secs: 1 };
        let book = TargetSeries::from_bars(&bars, grid);
        let target_5 = target_series(&book, Target::Return, 5);
        let column = |name: &str| frame.column(name).unwrap().as_materialized_series().clone();
        let times = column("time_us");
        let times = times.i64().unwrap();
        let spreads = column("spread_bps");
        let spreads = spreads.f64().unwrap();
        let targets = column("target_5s");
        let targets = targets.f64().unwrap();
        for (row, &i) in exported.iter().enumerate().step_by(97) {
            assert_eq!(times.get(row).unwrap(), bar_close_us(&bars[i], grid));
            assert_eq!(spreads.get(row).unwrap(), spread[i]);
            let scored = target_5[i + config.decision_delay_bars];
            let written = targets.get(row).unwrap();
            assert!(
                (scored.is_nan() && written.is_nan()) || scored == written,
                "row {row}: scored {scored} written {written}"
            );
        }
        let symbols = column("symbol");
        assert_eq!(
            symbols.str().unwrap().get(0).unwrap(),
            "BINANCE_FUTURES:SOLUSDT"
        );
        assert!(summary_table(&result).contains("accepted.parquet"));
        // The CSV side of parquet-schema writes the same table.
        let csv = out.join("accepted.csv");
        crate::lake::describe_parquet(&path, 1, Some(&csv)).unwrap();
        assert_eq!(
            fs::read_to_string(&csv).unwrap().lines().count(),
            exported.len() + 1
        );
        // Without an accepted set there is nothing to export.
        let _ = fs::remove_file(&path);
        config.accepted.clear();
        let bare = run_on_panel_with_series(&config, panel(), Vec::new(), day, day, &out).unwrap();
        assert!(bare.accepted_export.is_none());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&out);
    }

    /// The pre-expression feature switch, kept as the parity reference for the eight names.
    fn legacy_feature_value(
        name: &str,
        book: &crate::lake::BookFeatures,
        bars: &[lake::LakeBar],
        index: usize,
    ) -> Option<f64> {
        Some(match name {
            "obi_l1" => book.obi_l1,
            "obi_l5" => book.obi_l5,
            "obi_l10" => book.obi_l10,
            "microprice_bps" => book.microprice_bps(),
            "spread_bps" => book.spread_bps,
            "trade_imbalance" => book.trade_imbalance(),
            "signed_volume" => book.buy_volume - book.sell_volume,
            "return_1" => {
                let previous = bars.get(index.checked_sub(1)?)?.book?.mid;
                if previous > 0.0 {
                    (book.mid / previous - 1.0) * 1e4
                } else {
                    return None;
                }
            }
            _ => return None,
        })
    }

    /// A deterministic day of 1-second bars with a moving book and occasional gaps.
    fn synthetic_grid() -> Vec<lake::LakeBar> {
        synthetic_grid_days(1)
    }

    /// The same grid over several dates, 3,000 one-second bars a day.
    fn synthetic_grid_days(days: u32) -> Vec<lake::LakeBar> {
        let mut seed: u64 = 0x5eed;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let mut mid = 150.0;
        (0..3_000 * days)
            .map(|i| {
                // Volatility alternates every 300 bars, and the quoted spread widens with it,
                // as it does on the SOL perpetuals: half a bp in the quiet regime, two in the
                // busy one.
                let vol = if (i / 300) % 2 == 0 { 1e-4 } else { 4e-4 };
                mid *= 1.0 + (rand() - 0.5) * 2.0 * vol;
                let half_spread = mid * vol * 0.25;
                let bid_size = 1.0 + rand() * 20.0;
                let ask_size = 1.0 + rand() * 20.0;
                let buy = rand() * 5.0;
                let sell = rand() * 5.0;
                let book = (i % 97 != 0).then(|| crate::lake::BookFeatures {
                    bid: mid - half_spread,
                    ask: mid + half_spread,
                    bid_size,
                    ask_size,
                    mid,
                    microprice: (mid - half_spread) * ask_size / (bid_size + ask_size)
                        + (mid + half_spread) * bid_size / (bid_size + ask_size),
                    spread_bps: 2.0 * half_spread / mid * 1e4,
                    obi_l1: (bid_size - ask_size) / (bid_size + ask_size),
                    obi_l5: rand() - 0.5,
                    obi_l10: rand() - 0.5,
                    bid_depth_l5: bid_size * 4.0,
                    ask_depth_l5: ask_size * 4.0,
                    trade_count: (rand() * 8.0) as usize,
                    buy_volume: buy,
                    sell_volume: sell,
                });
                lake::LakeBar {
                    date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap()
                        + chrono::Days::new(u64::from(i / 3_000)),
                    time: chrono::NaiveTime::from_num_seconds_from_midnight_opt(i % 3_000, 0)
                        .unwrap(),
                    open: mid,
                    high: mid,
                    low: mid,
                    close: mid,
                    volume: buy + sell,
                    book,
                }
            })
            .collect()
    }

    #[test]
    fn expressions_reproduce_the_original_eight_features_and_their_ic() {
        let bars = synthetic_grid();
        let names: Vec<String> = FEATURES.iter().map(|f| (*f).to_owned()).collect();
        let (mids, features, with_book) = evaluate_features(&names, 1, &bars, true).unwrap();
        assert_eq!(with_book, bars.iter().filter(|b| b.book.is_some()).count());
        for name in FEATURES {
            let legacy: Vec<f64> = bars
                .iter()
                .enumerate()
                .map(|(i, bar)| match bar.book {
                    Some(book) => legacy_feature_value(name, &book, &bars, i).unwrap_or(f64::NAN),
                    None => f64::NAN,
                })
                .collect();
            let fresh = &features[*name];
            assert_eq!(fresh.len(), legacy.len());
            for (a, b) in fresh.iter().zip(&legacy) {
                assert!(
                    (a.is_nan() && b.is_nan()) || a == b,
                    "{name}: expression {a} vs legacy {b}"
                );
            }
            for horizon in [1, 5, 30] {
                let (xf, yf) = return_pairs(fresh, &mids, horizon, 1);
                let (xl, yl) = return_pairs(&legacy, &mids, horizon, 1);
                assert_eq!(xf, xl);
                assert_eq!(spearman(&xf, &yf), spearman(&xl, &yl), "{name} h{horizon}");
            }
        }
        // And a transformed expression runs through the same path.
        let (_, derived, _) =
            evaluate_features(&["obi_l1 | zscore 30".to_owned()], 1, &bars, true).unwrap();
        assert!(
            derived["obi_l1 | zscore 30"]
                .iter()
                .filter(|v| v.is_finite())
                .count()
                > 2_000
        );
        let err = evaluate_features(&["obi_l1 | smooth 3".to_owned()], 1, &bars, true).unwrap_err();
        assert!(format!("{err:#}").contains("unknown transform"));
    }

    #[test]
    fn buckets_split_by_feature_rank_and_find_a_monotone_edge() {
        let xs: Vec<f64> = (0..1000).map(|i| (i % 100) as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| x * 0.1).collect();
        let c = cell("T", "obi_l1", 1, Grid::Lake { step_secs: 1 }, 10, &xs, &ys);
        assert_eq!(c.buckets.len(), 10);
        assert!(c.buckets[9].forward_bps > c.buckets[0].forward_bps);
        assert!(c.top_minus_bottom_bps > 0.0 && c.top_minus_bottom_t > 5.0);
        assert!((c.ic - 1.0).abs() < 1e-9);
        // The costless curves ride on the cell: a monotone edge pays, in both variants.
        assert_eq!(c.curves.len(), 2);
        assert_eq!(c.curves[0].variant, "zscore");
        assert_eq!(c.curves[1].variant, "sign");
        assert!(c.breakeven_bps > 0.0 && c.sharpe > 0.0 && c.turnover > 0.0);
        assert!(c.curves[1].final_bps > 0.0);
        assert_eq!(c.breakeven_bps, c.curves[0].breakeven_bps);
    }

    /// Standard normal draws from a seeded xorshift and Box-Muller.
    fn gaussians(mut seed: u64, count: usize) -> Vec<f64> {
        let mut uniform = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        (0..count)
            .map(|_| {
                let u = uniform();
                let v = uniform();
                (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
            })
            .collect()
    }

    #[test]
    fn noisy_copy_of_the_forward_return_earns_the_analytic_sharpe() {
        // y ~ N(0, 1) bps, feature x = y + N(0, 1). With position = z(x) the per-period Sharpe
        // is E[zy] / sd(zy) = (1/sqrt2) / sqrt(2 - 1/2) = 1/sqrt3; clipping at 3 sd barely
        // moves it. With position = sign(x): E[y sign(x)] = rho sqrt(2/pi), rho = 1/sqrt2.
        let n = 40_000;
        let ys = gaussians(0x9e37_79b9_7f4a_7c15, n);
        let noise = gaussians(0xd1b5_4a32_d192_ed03, n);
        let xs: Vec<f64> = ys.iter().zip(&noise).map(|(y, e)| y + e).collect();

        let z = costless_curve("zscore", &xs, &ys, 1.0);
        let expected_z = 1.0 / 3f64.sqrt();
        assert!(
            (z.sharpe - expected_z).abs() < 0.04,
            "zscore sharpe {} vs analytic {expected_z}",
            z.sharpe
        );
        let sign = costless_curve("sign", &xs, &ys, 1.0);
        let mean_sign = (1.0 / 2f64.sqrt()) * (2.0 / std::f64::consts::PI).sqrt();
        let expected_sign = mean_sign / (1.0 - mean_sign * mean_sign).sqrt();
        assert!(
            (sign.sharpe - expected_sign).abs() < 0.04,
            "sign sharpe {} vs analytic {expected_sign}",
            sign.sharpe
        );
        assert!((sign.mean_bps - mean_sign).abs() < 0.03);
        // Both rules pay, so both tolerate a positive cost, and the curve ends where it says.
        assert!(z.breakeven_bps > 0.0 && sign.breakeven_bps > 0.0);
        assert!(z.final_bps > 0.0 && (z.final_bps - z.mean_bps * n as f64).abs() < 1e-6);
        assert_eq!(z.curve.len(), CURVE_POINTS);
        assert_eq!(z.curve[0].observation, 0);
        assert_eq!(z.curve[CURVE_POINTS - 1].observation, n - 1);
        assert_eq!(z.curve[CURVE_POINTS - 1].cumulative_bps, z.final_bps);
        // Annualization scales with the square root of the periods per year.
        let yearly = costless_curve("zscore", &xs, &ys, 4.0);
        assert!((yearly.sharpe - 2.0 * z.sharpe).abs() < 1e-9);
        assert!((periods_per_year(Grid::Lake { step_secs: 1 }, 60) - 525_960.0).abs() < 1e-6);
        assert_eq!(periods_per_year(Grid::Daily, 5), 252.0 / 5.0);
    }

    #[test]
    fn pure_noise_feature_has_breakeven_near_zero() {
        let n = 40_000;
        let ys = gaussians(0x2545_f491_4f6c_dd1d, n);
        let xs = gaussians(0x1234_5678_9abc_def1, n);
        for variant in ["zscore", "sign"] {
            let curve = costless_curve(variant, &xs, &ys, 1.0);
            assert!(
                curve.breakeven_bps.abs() < 0.03,
                "{variant} breakeven {} should be near zero",
                curve.breakeven_bps
            );
            assert!(
                curve.sharpe.abs() < 0.03,
                "{variant} sharpe {}",
                curve.sharpe
            );
            assert!(curve.turnover > 0.5, "{variant} trades every bar");
        }
        // An iid z-score changes by N(0, 2) per bar: mean absolute change 2/sqrt(pi).
        let z = costless_curve("zscore", &xs, &ys, 1.0);
        assert!((z.turnover - 2.0 / std::f64::consts::PI.sqrt()).abs() < 0.03);
        // A constant feature takes no position and reports no breakeven.
        let flat = costless_curve("zscore", &vec![1.0; 50], &ys[..50], 1.0);
        assert_eq!(flat.turnover, 0.0);
        assert!(flat.breakeven_bps.is_nan() && flat.sharpe.is_nan());
        assert_eq!(flat.curve.len(), 50);
    }
}
