//! Feature studies: how a bar-level feature relates to forward returns at several horizons.
//!
//! This is the research step before a strategy. For each symbol the study builds
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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::feature_expr::{self, BarInput, Evaluator};
use crate::lake::{self, LakeSymbol};

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
    pub lake_dir: std::path::PathBuf,
    /// `EXCHANGE:SYMBOL` instruments.
    pub symbols: Vec<String>,
    /// Sampling grid in seconds (also the unit of horizons and delay).
    #[serde(default = "default_step")]
    pub step_secs: u32,
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
    /// Mid, spread, and microprice per bar plus the fair-value EMA on the grid's step.
    pub fn from_bars(bars: &[lake::LakeBar], step_secs: u32) -> Self {
        let read = |f: fn(&crate::lake::BookFeatures) -> f64| -> Vec<f64> {
            bars.iter()
                .map(|bar| bar.book.as_ref().map(f).unwrap_or(f64::NAN))
                .collect()
        };
        let mids = read(|b| b.mid);
        let window = (FAIR_VALUE_EMA_SECS / step_secs.max(1)).max(1) as f64;
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
    /// Costless curves: `zscore` first, then `sign`.
    pub curves: Vec<CostlessCurve>,
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

/// Independent periods per year for annualizing a horizon on a grid, assuming a market that
/// never closes (the lake holds crypto perpetuals).
fn periods_per_year(step_secs: u32, horizon: usize) -> f64 {
    365.25 * 86_400.0 / (step_secs as f64 * horizon.max(1) as f64)
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
    let curve = if n <= CURVE_POINTS {
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
    };
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
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub symbols: Vec<SymbolCoverage>,
    pub cells: Vec<StudyCell>,
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
    bars: usize,
    bars_with_book: usize,
}

fn load_symbol(
    config: &StudyConfig,
    symbol: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<SymbolSeries> {
    let sym =
        LakeSymbol::parse(symbol).with_context(|| format!("{symbol} is not EXCHANGE:SYMBOL"))?;
    let bars = lake::build_bars(&config.lake_dir, &sym, config.step_secs, start, end)?;
    let (_, features, bars_with_book) =
        evaluate_features(&config.features, config.step_secs, &bars)?;
    Ok(SymbolSeries {
        symbol: symbol.to_owned(),
        book: TargetSeries::from_bars(&bars, config.step_secs),
        features,
        bars: bars.len(),
        bars_with_book,
    })
}

/// Runs every feature expression over the bars in one streaming pass. Returns the mid series
/// (`NaN` where the bar has no book), the feature matrix, and the count of bars with a book.
fn evaluate_features(
    expressions: &[String],
    step_secs: u32,
    bars: &[lake::LakeBar],
) -> Result<(Vec<f64>, BTreeMap<String, Vec<f64>>, usize)> {
    let mut evaluators = Vec::with_capacity(expressions.len());
    for text in expressions {
        let expr =
            feature_expr::parse(text).with_context(|| format!("feature expression {text:?}"))?;
        evaluators.push((text.clone(), Evaluator::new(&expr, step_secs)));
    }
    let mut mids = Vec::with_capacity(bars.len());
    let mut features: BTreeMap<String, Vec<f64>> = expressions
        .iter()
        .map(|f| (f.clone(), Vec::with_capacity(bars.len())))
        .collect();
    let mut bars_with_book = 0;
    let mut previous_mid = None;
    for bar in bars {
        let input = BarInput { bar, previous_mid };
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

fn cell(
    symbol: &str,
    feature: &str,
    horizon: usize,
    step: u32,
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
    let per_year = periods_per_year(step, horizon);
    let curves = vec![
        costless_curve("zscore", xs, ys, per_year),
        costless_curve("sign", xs, ys, per_year),
    ];
    StudyCell {
        symbol: symbol.to_owned(),
        feature: feature.to_owned(),
        horizon_bars: horizon,
        horizon_secs: horizon as u32 * step,
        observations: xs.len(),
        ic,
        top_minus_bottom_bps: diff,
        top_minus_bottom_t: if se > 0.0 { diff / se } else { f64::NAN },
        sharpe: curves[0].sharpe,
        turnover: curves[0].turnover,
        breakeven_bps: curves[0].breakeven_bps,
        buckets: rows,
        curves,
    }
}

pub fn run(
    config: &StudyConfig,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<StudyResult> {
    if config.symbols.is_empty() {
        bail!("the study needs at least one EXCHANGE:SYMBOL");
    }
    for feature in &config.features {
        feature_expr::parse(feature).with_context(|| format!("feature expression {feature:?}"))?;
    }
    fs::create_dir_all(output_dir)?;
    eprintln!(
        "progress: load 0/{} symbols elapsed=0s",
        config.symbols.len()
    );
    let started = std::time::Instant::now();
    let series: Vec<SymbolSeries> = config
        .symbols
        .par_iter()
        .map(|symbol| load_symbol(config, symbol, start, end))
        .collect::<Result<Vec<_>>>()?;
    eprintln!(
        "progress: load {}/{} symbols loaded elapsed={}s",
        series.len(),
        config.symbols.len(),
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
    let total = (series.len() + 1) * config.features.len() * config.horizons.len();
    let mut done = 0usize;
    for feature in &config.features {
        for (hi, &horizon) in config.horizons.iter().enumerate() {
            let mut pooled_x = Vec::new();
            let mut pooled_y = Vec::new();
            for (s, symbol_targets) in series.iter().zip(&targets) {
                let (xs, ys) = pairs(
                    &s.features[feature],
                    &symbol_targets[hi],
                    config.decision_delay_bars,
                );
                if xs.len() >= 100 {
                    cells.push(cell(
                        &s.symbol,
                        feature,
                        horizon,
                        config.step_secs,
                        config.buckets,
                        &xs,
                        &ys,
                    ));
                }
                pooled_x.extend(xs);
                pooled_y.extend(ys);
                done += 1;
            }
            if series.len() > 1 && pooled_x.len() >= 100 {
                cells.push(cell(
                    "ALL",
                    feature,
                    horizon,
                    config.step_secs,
                    config.buckets,
                    &pooled_x,
                    &pooled_y,
                ));
            }
            done += 1;
            eprintln!(
                "progress: study {done}/{total} cells elapsed={}s",
                started.elapsed().as_secs()
            );
        }
    }
    let result = StudyResult {
        config: config.clone(),
        target: config.target,
        target_unit: config.target.unit().to_owned(),
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
    };
    fs::write(
        output_dir.join("study.json"),
        serde_json::to_string_pretty(&result)?,
    )?;
    let mut csv = String::from(
        "symbol,feature,horizon_secs,observations,ic,top_minus_bottom_bps,top_minus_bottom_t,\
         sharpe,turnover,breakeven_bps,sign_sharpe,sign_turnover,sign_breakeven_bps\n",
    );
    let mut curves =
        String::from("symbol,feature,horizon_secs,variant,observation,cumulative_bps\n");
    for c in &result.cells {
        let sign = &c.curves[1];
        csv.push_str(&format!(
            "{},{},{},{},{:.5},{:.3},{:.2},{:.3},{:.4},{:.4},{:.3},{:.4},{:.4}\n",
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
            sign.sharpe,
            sign.turnover,
            sign.breakeven_bps
        ));
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
    Ok(result)
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
    let _ = writeln!(out, "target: {} ({unit})", result.target.name());
    let _ = writeln!(
        out,
        "{:<16} {:>8} {:>10} {:>9} {:>12} {:>8} {:>8} {:>10}",
        "feature",
        "horizon",
        "obs",
        "IC",
        format!("top-bot {unit}"),
        "t",
        "sharpe",
        "brkeven"
    );
    for c in result.cells.iter().filter(|c| c.symbol == symbol) {
        let _ = writeln!(
            out,
            "{:<16} {:>7}s {:>10} {:>+9.4} {:>+12.3} {:>+8.1} {:>+8.2} {:>+10.4}",
            c.feature,
            c.horizon_secs,
            c.observations,
            c.ic,
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
        let series = TargetSeries::from_bars(&bars, 1);
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
        let series = TargetSeries::from_bars(&bars, 1);
        let (_, features, _) = evaluate_features(&["spread_bps".to_owned()], 1, &bars).unwrap();
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
        let c = cell("SOL", "spread_bps", 30, 1, 10, &xs, &ys);
        assert!((c.ic - ic).abs() < 1e-12 && c.curves[0].curve.len() > 0);
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
        let mut seed: u64 = 0x5eed;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 10_000) as f64 / 10_000.0
        };
        let mut mid = 150.0;
        (0..3_000)
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
                    date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
                    time: chrono::NaiveTime::from_num_seconds_from_midnight_opt(i, 0).unwrap(),
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
        let (mids, features, with_book) = evaluate_features(&names, 1, &bars).unwrap();
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
            evaluate_features(&["obi_l1 | zscore 30".to_owned()], 1, &bars).unwrap();
        assert!(
            derived["obi_l1 | zscore 30"]
                .iter()
                .filter(|v| v.is_finite())
                .count()
                > 2_000
        );
        let err = evaluate_features(&["obi_l1 | smooth 3".to_owned()], 1, &bars).unwrap_err();
        assert!(format!("{err:#}").contains("unknown transform"));
    }

    #[test]
    fn buckets_split_by_feature_rank_and_find_a_monotone_edge() {
        let xs: Vec<f64> = (0..1000).map(|i| (i % 100) as f64).collect();
        let ys: Vec<f64> = xs.iter().map(|x| x * 0.1).collect();
        let c = cell("T", "obi_l1", 1, 1, 10, &xs, &ys);
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
        assert!((periods_per_year(1, 60) - 525_960.0).abs() < 1e-6);
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
