//! Feature studies: how a bar-level feature relates to forward returns at several horizons.
//!
//! This is the research step before a strategy. For each symbol the study builds
//! `step_secs` bars from the tick lake, computes the requested features at each bar close,
//! measures forward mid-price returns `h` bars ahead, then reports per feature and horizon:
//! Spearman rank correlation (information coefficient), the mean forward return by feature
//! decile, and a t-statistic for top-minus-bottom decile. `decision_delay_bars` shifts every
//! feature by that many bars before measuring, which models the time between observing the
//! book and being able to act on it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::lake::{self, BookFeatures, LakeBar, LakeSymbol};

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
    pub buckets: Vec<BucketRow>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StudyResult {
    pub config: StudyConfig,
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
    mids: Vec<f64>,
    features: BTreeMap<String, Vec<f64>>,
    bars: usize,
    bars_with_book: usize,
}

fn feature_value(name: &str, book: &BookFeatures, bars: &[LakeBar], index: usize) -> Option<f64> {
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

fn load_symbol(
    config: &StudyConfig,
    symbol: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<SymbolSeries> {
    let sym =
        LakeSymbol::parse(symbol).with_context(|| format!("{symbol} is not EXCHANGE:SYMBOL"))?;
    let bars = lake::build_bars(&config.lake_dir, &sym, config.step_secs, start, end)?;
    let mut mids = Vec::with_capacity(bars.len());
    let mut features: BTreeMap<String, Vec<f64>> = config
        .features
        .iter()
        .map(|f| (f.clone(), Vec::with_capacity(bars.len())))
        .collect();
    let mut bars_with_book = 0;
    for (index, bar) in bars.iter().enumerate() {
        match bar.book {
            Some(book) => {
                bars_with_book += 1;
                mids.push(book.mid);
                for (name, values) in &mut features {
                    values.push(feature_value(name, &book, &bars, index).unwrap_or(f64::NAN));
                }
            }
            None => {
                mids.push(f64::NAN);
                for values in features.values_mut() {
                    values.push(f64::NAN);
                }
            }
        }
    }
    Ok(SymbolSeries {
        symbol: symbol.to_owned(),
        mids,
        features,
        bars: bars.len(),
        bars_with_book,
    })
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

/// Aligned (feature, forward return in bps) pairs for one horizon and delay.
fn pairs(feature: &[f64], mids: &[f64], horizon: usize, delay: usize) -> (Vec<f64>, Vec<f64>) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for i in 0..feature.len() {
        let x = feature[i];
        let act = i + delay;
        let Some(&entry) = mids.get(act) else { break };
        let Some(&exit) = mids.get(act + horizon) else {
            break;
        };
        if !(x.is_finite() && entry.is_finite() && exit.is_finite() && entry > 0.0) {
            continue;
        }
        xs.push(x);
        ys.push((exit / entry - 1.0) * 1e4);
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
    StudyCell {
        symbol: symbol.to_owned(),
        feature: feature.to_owned(),
        horizon_bars: horizon,
        horizon_secs: horizon as u32 * step,
        observations: xs.len(),
        ic,
        top_minus_bottom_bps: diff,
        top_minus_bottom_t: if se > 0.0 { diff / se } else { f64::NAN },
        buckets: rows,
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
        if !FEATURES.contains(&feature.as_str()) {
            bail!(
                "unknown feature {feature:?}; choose from {}",
                FEATURES.join(", ")
            );
        }
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
    let mut cells = Vec::new();
    let total = (series.len() + 1) * config.features.len() * config.horizons.len();
    let mut done = 0usize;
    for feature in &config.features {
        for &horizon in &config.horizons {
            let mut pooled_x = Vec::new();
            let mut pooled_y = Vec::new();
            for s in &series {
                let (xs, ys) = pairs(
                    &s.features[feature],
                    &s.mids,
                    horizon,
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
        "symbol,feature,horizon_secs,observations,ic,top_minus_bottom_bps,top_minus_bottom_t\n",
    );
    for c in &result.cells {
        csv.push_str(&format!(
            "{},{},{},{},{:.5},{:.3},{:.2}\n",
            c.symbol,
            c.feature,
            c.horizon_secs,
            c.observations,
            c.ic,
            c.top_minus_bottom_bps,
            c.top_minus_bottom_t
        ));
    }
    fs::write(output_dir.join("study.csv"), csv)?;
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
    let _ = writeln!(
        out,
        "{:<16} {:>8} {:>10} {:>9} {:>12} {:>8}",
        "feature", "horizon", "obs", "IC", "top-bot bps", "t"
    );
    for c in result.cells.iter().filter(|c| c.symbol == symbol) {
        let _ = writeln!(
            out,
            "{:<16} {:>7}s {:>10} {:>+9.4} {:>+12.3} {:>+8.1}",
            c.feature,
            c.horizon_secs,
            c.observations,
            c.ic,
            c.top_minus_bottom_bps,
            c.top_minus_bottom_t
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

    #[test]
    fn forward_returns_respect_horizon_and_delay() {
        // Mid climbs 1 bp per bar; feature is the bar index.
        let mids: Vec<f64> = (0..20).map(|i| 100.0 * (1.0 + 1e-4 * i as f64)).collect();
        let feature: Vec<f64> = (0..20).map(|i| i as f64).collect();
        let (xs, ys) = pairs(&feature, &mids, 5, 0);
        assert_eq!(xs.len(), 15);
        assert!(
            (ys[0] - 5.0).abs() < 1e-6,
            "5 bars ahead is ~5 bps, got {}",
            ys[0]
        );
        let (xd, _) = pairs(&feature, &mids, 5, 2);
        assert_eq!(xd.len(), 13, "delay consumes bars at the end");
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
    }
}
