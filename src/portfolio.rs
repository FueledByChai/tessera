use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use polars::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PortfolioConfig {
    #[serde(default = "default_initial_capital")]
    pub initial_capital: f64,
    #[serde(default)]
    pub rebalance: RebalanceMethod,
    #[serde(default)]
    pub capital_mode: CapitalMode,
    pub components: Vec<PortfolioComponentConfig>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RebalanceMethod {
    #[default]
    Daily,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapitalMode {
    #[default]
    NormalizedWeights,
    SequentialFullCapital,
    SequentialGroups,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PortfolioComponentConfig {
    pub name: String,
    pub results_dir: PathBuf,
    pub weight: f64,
    #[serde(default)]
    pub capital_group: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SourceManifest {
    #[serde(default)]
    initial_capital: Option<f64>,
    #[serde(default)]
    config: Option<toml::Value>,
}

impl SourceManifest {
    fn initial_capital(&self) -> f64 {
        self.initial_capital
            .or_else(|| {
                self.config
                    .as_ref()?
                    .get("strategy")?
                    .get("initial_capital")?
                    .as_float()
            })
            .unwrap_or(default_initial_capital())
    }
}

#[derive(Debug, Clone, Copy)]
struct SourceDay {
    return_value: f64,
    fills: usize,
    entry_time: Option<DateTime<FixedOffset>>,
    exit_time: Option<DateTime<FixedOffset>>,
}

#[derive(Debug, Clone, Copy)]
struct SourceInterval {
    entry_time: DateTime<FixedOffset>,
    exit_time: DateTime<FixedOffset>,
}

#[derive(Debug)]
struct SourcePortfolio {
    name: String,
    results_dir: PathBuf,
    configured_weight: f64,
    normalized_weight: f64,
    capital_group: String,
    group_weight: f64,
    days: BTreeMap<NaiveDate, SourceDay>,
    coverage_statuses: Vec<String>,
}

#[derive(Debug)]
struct PortfolioDay {
    date: NaiveDate,
    ending_equity: f64,
    pnl: f64,
    fills: usize,
    return_value: f64,
}

#[derive(Debug)]
struct SleeveDay {
    date: NaiveDate,
    entry_time: DateTime<FixedOffset>,
    exit_time: DateTime<FixedOffset>,
    name: String,
    pnl: f64,
    return_percent: f64,
}

#[derive(Debug)]
struct GroupComponentEvent {
    name: String,
    weight: f64,
    return_value: f64,
    fills: usize,
    entry_time: DateTime<FixedOffset>,
    exit_time: DateTime<FixedOffset>,
}

#[derive(Debug)]
struct GroupEvent {
    group: String,
    entry_time: DateTime<FixedOffset>,
    exit_time: DateTime<FixedOffset>,
    components: Vec<GroupComponentEvent>,
}

#[derive(Debug, Serialize)]
struct PortfolioManifest<'a> {
    strategy_name: &'static str,
    resolution: &'static str,
    initial_capital: f64,
    start: NaiveDate,
    end: NaiveDate,
    symbols: Vec<String>,
    parameters: BTreeMap<String, String>,
    correlation_matrices: Vec<CorrelationMatrix>,
    config: &'a PortfolioConfig,
}

#[derive(Debug)]
pub struct PortfolioSummary {
    pub components: usize,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub sessions: usize,
    pub starting_equity: f64,
    pub ending_equity: f64,
    pub total_return_percent: f64,
    pub max_drawdown_percent: f64,
    pub component_summaries: Vec<ComponentSummary>,
    pub pairwise_correlations: Vec<PairwiseCorrelation>,
}

#[derive(Debug)]
pub struct ComponentSummary {
    pub name: String,
    pub total_return_percent: f64,
    pub cagr_percent: f64,
    pub annualized_volatility_percent: f64,
    pub sharpe: Option<f64>,
    pub max_drawdown_percent: f64,
}

#[derive(Debug)]
pub struct PairwiseCorrelation {
    pub first: String,
    pub second: String,
    pub correlation: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CorrelationMatrix {
    pub frequency: String,
    pub observations: usize,
    pub labels: Vec<String>,
    pub values: Vec<Vec<f64>>,
}

#[derive(Debug, Clone, Copy)]
enum CorrelationFrequency {
    Daily,
    Weekly,
    Monthly,
}

fn default_initial_capital() -> f64 {
    100_000.0
}

impl PortfolioConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read portfolio config {}", path.display()))?;
        let config: Self = toml::from_str(&text)
            .with_context(|| format!("failed to parse portfolio config {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.initial_capital.is_finite() && self.initial_capital > 0.0,
            "initial_capital must be positive"
        );
        anyhow::ensure!(
            self.components.len() >= 2,
            "a combined portfolio requires at least two components"
        );
        for component in &self.components {
            anyhow::ensure!(!component.name.trim().is_empty(), "component name is empty");
            anyhow::ensure!(
                component.weight.is_finite() && component.weight > 0.0,
                "component {} must have a positive weight",
                component.name
            );
            if self.capital_mode == CapitalMode::SequentialFullCapital {
                anyhow::ensure!(
                    component.weight <= 1.0,
                    "sequential component {} cannot use more than 100% of available capital",
                    component.name
                );
            }
            if self.capital_mode == CapitalMode::SequentialGroups {
                anyhow::ensure!(
                    component
                        .capital_group
                        .as_deref()
                        .is_some_and(|group| !group.trim().is_empty()),
                    "sequential-groups component {} requires capital_group",
                    component.name
                );
                anyhow::ensure!(
                    component.weight <= 1.0,
                    "sequential-groups component {} cannot use more than 100% of its group",
                    component.name
                );
            }
        }
        Ok(())
    }
}

pub fn combine_portfolio(config_path: &Path, output_dir: &Path) -> Result<PortfolioSummary> {
    let config = PortfolioConfig::load(config_path)?;
    let weight_sum: f64 = config
        .components
        .iter()
        .map(|component| component.weight)
        .sum();
    let mut sources = Vec::with_capacity(config.components.len());
    for component in &config.components {
        let manifest: SourceManifest = toml::from_str(
            &fs::read_to_string(component.results_dir.join("run_config.toml")).with_context(
                || {
                    format!(
                        "missing run_config.toml for component {} in {}",
                        component.name,
                        component.results_dir.display()
                    )
                },
            )?,
        )?;
        let intervals = load_source_intervals(&component.results_dir.join("trades.parquet"))?;
        let days = load_source_days(
            &component.results_dir.join("daily_equity.parquet"),
            manifest.initial_capital(),
            &intervals,
        )?;
        anyhow::ensure!(
            !days.is_empty(),
            "component {} has no daily equity rows",
            component.name
        );
        sources.push(SourcePortfolio {
            name: component.name.clone(),
            results_dir: component.results_dir.clone(),
            configured_weight: component.weight,
            normalized_weight: component.weight / weight_sum,
            capital_group: component
                .capital_group
                .clone()
                .unwrap_or_else(|| component.name.clone()),
            group_weight: component.weight,
            days,
            coverage_statuses: load_coverage_statuses(
                &component.results_dir.join("coverage.parquet"),
            )?,
        });
    }

    let shared_dates = shared_dates(&sources)?;
    let start = *shared_dates
        .first()
        .context("portfolio has no shared dates")?;
    let end = *shared_dates
        .last()
        .context("portfolio has no shared dates")?;
    let (portfolio_days, sleeve_days) = match config.capital_mode {
        CapitalMode::NormalizedWeights => {
            build_daily_rebalanced_portfolio(config.initial_capital, &sources, &shared_dates)
        }
        CapitalMode::SequentialFullCapital => {
            validate_non_overlapping_intervals(&sources, &shared_dates)?;
            build_sequential_full_capital_portfolio(
                config.initial_capital,
                &sources,
                &shared_dates,
            )?
        }
        CapitalMode::SequentialGroups => {
            build_sequential_group_portfolio(config.initial_capital, &sources, &shared_dates)?
        }
    };
    let component_summaries: Vec<ComponentSummary> = sources
        .iter()
        .map(|source| summarize_component(source, &shared_dates))
        .collect();
    let correlation_matrices = vec![
        correlation_matrix(&sources, &shared_dates, CorrelationFrequency::Daily),
        correlation_matrix(&sources, &shared_dates, CorrelationFrequency::Weekly),
        correlation_matrix(&sources, &shared_dates, CorrelationFrequency::Monthly),
    ];
    let pairwise_correlations = pairwise_correlations(&correlation_matrices[0]);

    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let mut parameters = BTreeMap::new();
    parameters.insert(
        "Allocation".to_owned(),
        match config.capital_mode {
            CapitalMode::NormalizedWeights => sources
                .iter()
                .map(|source| format!("{} {:.1}%", source.name, 100.0 * source.normalized_weight))
                .collect::<Vec<_>>()
                .join("; "),
            CapitalMode::SequentialFullCapital => sources
                .iter()
                .map(|source| {
                    format!(
                        "{} {:.1}% of then-available capital",
                        source.name,
                        100.0 * source.configured_weight
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
            CapitalMode::SequentialGroups => {
                let mut groups = BTreeMap::<&str, Vec<String>>::new();
                for source in &sources {
                    groups
                        .entry(source.capital_group.as_str())
                        .or_default()
                        .push(format!(
                            "{} {:.1}%",
                            source.name,
                            100.0 * source.group_weight
                        ));
                }
                groups
                    .into_iter()
                    .map(|(group, allocations)| format!("{group}: {}", allocations.join(", ")))
                    .collect::<Vec<_>>()
                    .join("; ")
            }
        },
    );
    parameters.insert(
        "Combination method".to_owned(),
        match config.capital_mode {
            CapitalMode::NormalizedWeights => {
                "Daily-rebalanced weighted strategy returns over the intersection of source calendars"
                    .to_owned()
            }
            CapitalMode::SequentialFullCapital => {
                "Event-time capital reuse over the intersection of source calendars; source intervals were verified not to overlap"
                    .to_owned()
            }
            CapitalMode::SequentialGroups => {
                "Event-time capital reuse across non-overlapping groups; concurrent components within a group are additive full-size overlays and may exceed 100% gross allocation"
                    .to_owned()
            }
        },
    );
    parameters.insert(
        "Costs".to_owned(),
        "Already embedded in each source strategy; no additional capital-transfer cost".to_owned(),
    );
    parameters.insert(
        "Volatility targeting".to_owned(),
        "Each source retains its original internal sizing and lagged volatility target; no additional portfolio-level scaler"
            .to_owned(),
    );
    parameters.insert(
        "Trade statistics".to_owned(),
        "Rows represent active component sleeve-days, not the source strategies' underlying trades"
            .to_owned(),
    );
    for component in &component_summaries {
        parameters.insert(
            format!("Component: {}", component.name),
            format!(
                "shared-period return {:.2}%, CAGR {:.2}%, volatility {:.2}%, Sharpe {}, max DD -{:.2}%",
                component.total_return_percent,
                component.cagr_percent,
                component.annualized_volatility_percent,
                component
                    .sharpe
                    .map_or_else(|| "N/A".to_owned(), |value| format!("{value:.2}")),
                component.max_drawdown_percent
            ),
        );
    }
    for pair in &pairwise_correlations {
        parameters.insert(
            format!("Correlation: {} / {}", pair.first, pair.second),
            pair.correlation
                .map_or_else(|| "N/A".to_owned(), |value| format!("{value:.3}")),
        );
    }
    parameters.insert(
        "Source artifacts".to_owned(),
        sources
            .iter()
            .map(|source| format!("{}={}", source.name, source.results_dir.display()))
            .collect::<Vec<_>>()
            .join("; "),
    );
    let manifest = PortfolioManifest {
        strategy_name: "Combined Strategy Portfolio",
        resolution: "Daily combined return streams",
        initial_capital: config.initial_capital,
        start,
        end,
        symbols: sources.iter().map(|source| source.name.clone()).collect(),
        parameters,
        correlation_matrices,
        config: &config,
    };
    fs::write(
        output_dir.join("run_config.toml"),
        toml::to_string_pretty(&manifest)?,
    )?;
    write_daily_equity(&portfolio_days, &output_dir.join("daily_equity.parquet"))?;
    write_sleeve_days(&sleeve_days, &output_dir.join("trades.parquet"))?;
    write_coverage(&sources, &output_dir.join("coverage.parquet"))?;

    let ending_equity = portfolio_days
        .last()
        .map_or(config.initial_capital, |day| day.ending_equity);
    Ok(PortfolioSummary {
        components: sources.len(),
        start,
        end,
        sessions: portfolio_days.len(),
        starting_equity: config.initial_capital,
        ending_equity,
        total_return_percent: 100.0 * (ending_equity / config.initial_capital - 1.0),
        max_drawdown_percent: max_drawdown_percent(&portfolio_days, config.initial_capital),
        component_summaries,
        pairwise_correlations,
    })
}

fn summarize_component(source: &SourcePortfolio, dates: &[NaiveDate]) -> ComponentSummary {
    let returns: Vec<f64> = dates
        .iter()
        .map(|date| source.days[date].return_value)
        .collect();
    let ending = returns
        .iter()
        .fold(1.0, |equity, value| equity * (1.0 + value));
    let calendar_days = (*dates.last().expect("nonempty dates")
        - *dates.first().expect("nonempty dates"))
    .num_days()
    .max(1) as f64;
    let standard_deviation = sample_standard_deviation(&returns);
    ComponentSummary {
        name: source.name.clone(),
        total_return_percent: 100.0 * (ending - 1.0),
        cagr_percent: 100.0 * (ending.powf(365.25 / calendar_days) - 1.0),
        annualized_volatility_percent: 100.0 * standard_deviation * 252.0_f64.sqrt(),
        sharpe: (standard_deviation > f64::EPSILON)
            .then(|| mean(&returns) / standard_deviation * 252.0_f64.sqrt()),
        max_drawdown_percent: max_return_drawdown_percent(&returns),
    }
}

fn pairwise_correlations(matrix: &CorrelationMatrix) -> Vec<PairwiseCorrelation> {
    let mut pairs = Vec::new();
    for first_index in 0..matrix.labels.len() {
        for second_index in first_index + 1..matrix.labels.len() {
            let value = matrix.values[first_index][second_index];
            pairs.push(PairwiseCorrelation {
                first: matrix.labels[first_index].clone(),
                second: matrix.labels[second_index].clone(),
                correlation: value.is_finite().then_some(value),
            });
        }
    }
    pairs
}

fn correlation_matrix(
    sources: &[SourcePortfolio],
    dates: &[NaiveDate],
    frequency: CorrelationFrequency,
) -> CorrelationMatrix {
    let series: Vec<Vec<f64>> = sources
        .iter()
        .map(|source| aggregate_returns(source, dates, frequency))
        .collect();
    let observations = series.first().map_or(0, Vec::len);
    let values = (0..sources.len())
        .map(|first| {
            (0..sources.len())
                .map(|second| {
                    if first == second {
                        1.0
                    } else {
                        correlation(&series[first], &series[second]).unwrap_or(f64::NAN)
                    }
                })
                .collect()
        })
        .collect();
    CorrelationMatrix {
        frequency: match frequency {
            CorrelationFrequency::Daily => "Daily",
            CorrelationFrequency::Weekly => "Weekly",
            CorrelationFrequency::Monthly => "Monthly",
        }
        .to_owned(),
        observations,
        labels: sources.iter().map(|source| source.name.clone()).collect(),
        values,
    }
}

fn aggregate_returns(
    source: &SourcePortfolio,
    dates: &[NaiveDate],
    frequency: CorrelationFrequency,
) -> Vec<f64> {
    let mut groups = Vec::<((i32, u32), f64)>::new();
    for date in dates {
        let key = match frequency {
            CorrelationFrequency::Daily => (date.year(), date.ordinal()),
            CorrelationFrequency::Weekly => {
                let week = date.iso_week();
                (week.year(), week.week())
            }
            CorrelationFrequency::Monthly => (date.year(), date.month()),
        };
        let daily_return = source.days[date].return_value;
        if let Some((last_key, compounded)) = groups.last_mut()
            && *last_key == key
        {
            *compounded = (1.0 + *compounded) * (1.0 + daily_return) - 1.0;
        } else {
            groups.push((key, daily_return));
        }
    }
    groups.into_iter().map(|(_, value)| value).collect()
}

fn correlation(first: &[f64], second: &[f64]) -> Option<f64> {
    if first.len() != second.len() || first.len() < 2 {
        return None;
    }
    let first_mean = mean(first);
    let second_mean = mean(second);
    let covariance = first
        .iter()
        .zip(second)
        .map(|(a, b)| (a - first_mean) * (b - second_mean))
        .sum::<f64>()
        / (first.len() - 1) as f64;
    let denominator = sample_standard_deviation(first) * sample_standard_deviation(second);
    (denominator > f64::EPSILON).then(|| covariance / denominator)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn sample_standard_deviation(values: &[f64]) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let mean = mean(values);
    (values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64)
        .sqrt()
}

fn max_return_drawdown_percent(returns: &[f64]) -> f64 {
    let mut equity = 1.0_f64;
    let mut peak = 1.0_f64;
    let mut maximum = 0.0_f64;
    for value in returns {
        equity *= 1.0 + value;
        peak = peak.max(equity);
        maximum = maximum.max(100.0 * (peak - equity) / peak);
    }
    maximum
}

fn load_source_days(
    path: &Path,
    starting_equity: f64,
    intervals: &BTreeMap<NaiveDate, SourceInterval>,
) -> Result<BTreeMap<NaiveDate, SourceDay>> {
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let dates = frame.column("date")?.str()?;
    let equities = frame.column("ending_equity")?.f64()?;
    let fills = frame.column("fills")?.u64()?;
    let mut ordered = Vec::with_capacity(frame.height());
    for index in 0..frame.height() {
        ordered.push((
            NaiveDate::parse_from_str(dates.get(index).context("date is null")?, "%Y-%m-%d")?,
            equities.get(index).context("ending_equity is null")?,
            fills.get(index).context("fills is null")? as usize,
        ));
    }
    ordered.sort_unstable_by_key(|row| row.0);
    let mut previous = starting_equity;
    let mut days = BTreeMap::new();
    for (date, equity, fills) in ordered {
        anyhow::ensure!(previous > 0.0, "non-positive source equity before {date}");
        let interval = intervals.get(&date).copied();
        anyhow::ensure!(
            fills == 0 || interval.is_some(),
            "source daily equity has {fills} fills on {date} but no matching trade interval"
        );
        days.insert(
            date,
            SourceDay {
                return_value: equity / previous - 1.0,
                fills,
                entry_time: interval.map(|value| value.entry_time),
                exit_time: interval.map(|value| value.exit_time),
            },
        );
        previous = equity;
    }
    Ok(days)
}

fn load_source_intervals(path: &Path) -> Result<BTreeMap<NaiveDate, SourceInterval>> {
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let dates = frame.column("trade_date")?.str()?;
    let entries = frame.column("entry_time")?.str()?;
    let exits = frame.column("exit_time")?.str()?;
    let mut intervals = BTreeMap::<NaiveDate, SourceInterval>::new();
    for index in 0..frame.height() {
        let date =
            NaiveDate::parse_from_str(dates.get(index).context("trade_date is null")?, "%Y-%m-%d")?;
        let entry_time =
            DateTime::parse_from_rfc3339(entries.get(index).context("entry_time is null")?)?;
        let exit_time =
            DateTime::parse_from_rfc3339(exits.get(index).context("exit_time is null")?)?;
        // A five-minute strategy can conservatively model a limit fill followed by a stop in the
        // same bar, producing equal recorded timestamps. Only backward intervals are invalid.
        anyhow::ensure!(entry_time <= exit_time, "invalid trade interval on {date}");
        intervals
            .entry(date)
            .and_modify(|interval| {
                interval.entry_time = interval.entry_time.min(entry_time);
                interval.exit_time = interval.exit_time.max(exit_time);
            })
            .or_insert(SourceInterval {
                entry_time,
                exit_time,
            });
    }
    Ok(intervals)
}

fn load_coverage_statuses(path: &Path) -> Result<Vec<String>> {
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let statuses = frame.column("status")?.str()?;
    Ok((0..frame.height())
        .filter_map(|index| statuses.get(index).map(str::to_owned))
        .collect())
}

fn shared_dates(sources: &[SourcePortfolio]) -> Result<Vec<NaiveDate>> {
    let mut shared: Option<BTreeSet<NaiveDate>> = None;
    for source in sources {
        let dates: BTreeSet<NaiveDate> = source.days.keys().copied().collect();
        shared = Some(match shared {
            Some(current) => current.intersection(&dates).copied().collect(),
            None => dates,
        });
    }
    let dates: Vec<NaiveDate> = shared.unwrap_or_default().into_iter().collect();
    anyhow::ensure!(!dates.is_empty(), "component calendars do not overlap");
    Ok(dates)
}

fn validate_non_overlapping_intervals(
    sources: &[SourcePortfolio],
    dates: &[NaiveDate],
) -> Result<()> {
    let date_set: BTreeSet<NaiveDate> = dates.iter().copied().collect();
    let mut intervals = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        for (date, day) in &source.days {
            if !date_set.contains(date) || day.fills == 0 {
                continue;
            }
            intervals.push((
                source_index,
                source.name.as_str(),
                *date,
                day.entry_time.expect("filled day has entry time"),
                day.exit_time.expect("filled day has exit time"),
            ));
        }
    }
    intervals.sort_unstable_by_key(|interval| interval.3);
    for first_index in 0..intervals.len() {
        for second in &intervals[first_index + 1..] {
            let first = intervals[first_index];
            if second.3 >= first.4 {
                break;
            }
            if first.0 != second.0 && first.3 < second.4 && second.3 < first.4 {
                anyhow::bail!(
                    "sequential full-capital mode found overlapping intervals: {} {} to {} and {} {} to {}",
                    first.1,
                    first.3,
                    first.4,
                    second.1,
                    second.3,
                    second.4
                );
            }
        }
    }
    Ok(())
}

fn build_sequential_full_capital_portfolio(
    initial_capital: f64,
    sources: &[SourcePortfolio],
    dates: &[NaiveDate],
) -> Result<(Vec<PortfolioDay>, Vec<SleeveDay>)> {
    let date_set: BTreeSet<NaiveDate> = dates.iter().copied().collect();
    let mut events = Vec::new();
    for source in sources {
        for (date, day) in &source.days {
            if !date_set.contains(date) || day.fills == 0 {
                continue;
            }
            events.push((
                day.exit_time.context("filled day has no exit time")?,
                day.entry_time.context("filled day has no entry time")?,
                source.name.as_str(),
                source.configured_weight,
                day.return_value,
                day.fills,
            ));
        }
    }
    events.sort_unstable_by_key(|event| event.0);
    let mut calendar: BTreeSet<NaiveDate> = dates.iter().copied().collect();
    calendar.extend(events.iter().map(|event| event.0.date_naive()));
    let mut events_by_date = BTreeMap::<NaiveDate, Vec<_>>::new();
    for event in events {
        events_by_date
            .entry(event.0.date_naive())
            .or_default()
            .push(event);
    }

    let mut equity = initial_capital;
    let mut portfolio_days = Vec::with_capacity(calendar.len());
    let mut sleeve_days = Vec::new();
    for date in calendar {
        let day_start = equity;
        let mut fills = 0;
        if let Some(day_events) = events_by_date.get_mut(&date) {
            day_events.sort_unstable_by_key(|event| event.0);
            for (exit_time, entry_time, name, allocation, return_value, event_fills) in day_events {
                let pnl = equity * *allocation * *return_value;
                equity += pnl;
                fills += *event_fills;
                sleeve_days.push(SleeveDay {
                    date,
                    entry_time: *entry_time,
                    exit_time: *exit_time,
                    name: (*name).to_owned(),
                    pnl,
                    return_percent: 100.0 * *allocation * *return_value,
                });
            }
        }
        portfolio_days.push(PortfolioDay {
            date,
            ending_equity: equity,
            pnl: equity - day_start,
            fills,
            return_value: equity / day_start - 1.0,
        });
    }
    Ok((portfolio_days, sleeve_days))
}

fn build_sequential_group_portfolio(
    initial_capital: f64,
    sources: &[SourcePortfolio],
    dates: &[NaiveDate],
) -> Result<(Vec<PortfolioDay>, Vec<SleeveDay>)> {
    let mut events = Vec::<GroupEvent>::new();
    for date in dates {
        let mut by_group = BTreeMap::<String, Vec<GroupComponentEvent>>::new();
        for source in sources {
            let day = source.days[date];
            if day.fills == 0 {
                continue;
            }
            by_group
                .entry(source.capital_group.clone())
                .or_default()
                .push(GroupComponentEvent {
                    name: source.name.clone(),
                    weight: source.group_weight,
                    return_value: day.return_value,
                    fills: day.fills,
                    entry_time: day.entry_time.context("filled day has no entry time")?,
                    exit_time: day.exit_time.context("filled day has no exit time")?,
                });
        }
        for (group, components) in by_group {
            let entry_time = components
                .iter()
                .map(|component| component.entry_time)
                .min()
                .expect("nonempty group event");
            let exit_time = components
                .iter()
                .map(|component| component.exit_time)
                .max()
                .expect("nonempty group event");
            events.push(GroupEvent {
                group,
                entry_time,
                exit_time,
                components,
            });
        }
    }
    events.sort_unstable_by_key(|event| event.entry_time);
    for first_index in 0..events.len() {
        for second in &events[first_index + 1..] {
            let first = &events[first_index];
            if second.entry_time >= first.exit_time {
                break;
            }
            if first.group != second.group
                && first.entry_time < second.exit_time
                && second.entry_time < first.exit_time
            {
                anyhow::bail!(
                    "sequential-groups mode found overlapping capital groups: {} {} to {} and {} {} to {}",
                    first.group,
                    first.entry_time,
                    first.exit_time,
                    second.group,
                    second.entry_time,
                    second.exit_time
                );
            }
        }
    }
    events.sort_unstable_by_key(|event| event.exit_time);
    let mut calendar: BTreeSet<NaiveDate> = dates.iter().copied().collect();
    calendar.extend(events.iter().map(|event| event.exit_time.date_naive()));
    let mut events_by_date = BTreeMap::<NaiveDate, Vec<GroupEvent>>::new();
    for event in events {
        events_by_date
            .entry(event.exit_time.date_naive())
            .or_default()
            .push(event);
    }

    let mut equity = initial_capital;
    let mut portfolio_days = Vec::with_capacity(calendar.len());
    let mut sleeve_days = Vec::new();
    for date in calendar {
        let day_start = equity;
        let mut fills = 0;
        if let Some(day_events) = events_by_date.get_mut(&date) {
            day_events.sort_unstable_by_key(|event| event.exit_time);
            for event in day_events {
                let event_start = equity;
                let event_return = event
                    .components
                    .iter()
                    .map(|component| component.weight * component.return_value)
                    .sum::<f64>();
                for component in &event.components {
                    let component_return = component.weight * component.return_value;
                    sleeve_days.push(SleeveDay {
                        date,
                        entry_time: component.entry_time,
                        exit_time: component.exit_time,
                        name: component.name.clone(),
                        pnl: event_start * component_return,
                        return_percent: 100.0 * component_return,
                    });
                    fills += component.fills;
                }
                equity = event_start * (1.0 + event_return);
            }
        }
        portfolio_days.push(PortfolioDay {
            date,
            ending_equity: equity,
            pnl: equity - day_start,
            fills,
            return_value: equity / day_start - 1.0,
        });
    }
    Ok((portfolio_days, sleeve_days))
}

fn build_daily_rebalanced_portfolio(
    initial_capital: f64,
    sources: &[SourcePortfolio],
    dates: &[NaiveDate],
) -> (Vec<PortfolioDay>, Vec<SleeveDay>) {
    let mut equity = initial_capital;
    let mut portfolio_days = Vec::with_capacity(dates.len());
    let mut sleeve_days = Vec::new();
    for date in dates {
        let day_start = equity;
        let mut combined_return = 0.0;
        let mut fills = 0;
        for source in sources {
            let day = source.days[date];
            combined_return += source.normalized_weight * day.return_value;
            fills += day.fills;
            if day.fills > 0 {
                sleeve_days.push(SleeveDay {
                    date: *date,
                    entry_time: day.entry_time.expect("filled day has entry time"),
                    exit_time: day.exit_time.expect("filled day has exit time"),
                    name: source.name.clone(),
                    pnl: day_start * source.normalized_weight * day.return_value,
                    return_percent: 100.0 * day.return_value,
                });
            }
        }
        equity = day_start * (1.0 + combined_return);
        portfolio_days.push(PortfolioDay {
            date: *date,
            ending_equity: equity,
            pnl: equity - day_start,
            fills,
            return_value: combined_return,
        });
    }
    (portfolio_days, sleeve_days)
}

fn write_daily_equity(rows: &[PortfolioDay], path: &Path) -> Result<()> {
    let mut frame = df!(
        "date" => rows.iter().map(|row| row.date.to_string()).collect::<Vec<_>>(),
        "ending_equity" => rows.iter().map(|row| row.ending_equity).collect::<Vec<_>>(),
        "pnl" => rows.iter().map(|row| row.pnl).collect::<Vec<_>>(),
        "fills" => rows.iter().map(|row| row.fills as u64).collect::<Vec<_>>(),
        "combined_return_percent" => rows.iter().map(|row| 100.0 * row.return_value).collect::<Vec<_>>()
    )?;
    write_frame(&mut frame, path)
}

fn write_sleeve_days(rows: &[SleeveDay], path: &Path) -> Result<()> {
    let mut frame = df!(
        "trade_date" => rows.iter().map(|row| row.date.to_string()).collect::<Vec<_>>(),
        "symbol" => rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
        "entry_time" => rows.iter().map(|row| row.entry_time.to_rfc3339()).collect::<Vec<_>>(),
        "exit_time" => rows.iter().map(|row| row.exit_time.to_rfc3339()).collect::<Vec<_>>(),
        "pnl" => rows.iter().map(|row| row.pnl).collect::<Vec<_>>(),
        "return_percent" => rows.iter().map(|row| row.return_percent).collect::<Vec<_>>()
    )?;
    write_frame(&mut frame, path)
}

fn write_coverage(sources: &[SourcePortfolio], path: &Path) -> Result<()> {
    let mut dates = Vec::new();
    let mut components = Vec::new();
    let mut statuses = Vec::new();
    for source in sources {
        for status in &source.coverage_statuses {
            dates.push("");
            components.push(source.name.as_str());
            statuses.push(status.as_str());
        }
    }
    let mut frame = df!(
        "trade_date" => dates,
        "symbol" => components,
        "status" => statuses
    )?;
    write_frame(&mut frame, path)
}

fn write_frame(frame: &mut DataFrame, path: &Path) -> Result<()> {
    let file = File::create(path)?;
    ParquetWriter::new(file)
        .with_compression(ParquetCompression::Zstd(None))
        .finish(frame)?;
    Ok(())
}

fn max_drawdown_percent(rows: &[PortfolioDay], starting_equity: f64) -> f64 {
    let mut peak = starting_equity;
    let mut maximum = 0.0_f64;
    for row in rows {
        peak = peak.max(row.ending_equity);
        maximum = maximum.max(100.0 * (peak - row.ending_equity) / peak);
    }
    maximum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_and_monthly_returns_are_compounded_before_correlation() {
        let dates = vec![
            NaiveDate::from_ymd_opt(2025, 1, 30).unwrap(),
            NaiveDate::from_ymd_opt(2025, 1, 31).unwrap(),
            NaiveDate::from_ymd_opt(2025, 2, 3).unwrap(),
        ];
        let days = dates
            .iter()
            .copied()
            .zip([0.10, -0.10, 0.05])
            .map(|(date, return_value)| {
                (
                    date,
                    SourceDay {
                        return_value,
                        fills: 0,
                        entry_time: None,
                        exit_time: None,
                    },
                )
            })
            .collect();
        let source = SourcePortfolio {
            name: "Test".to_owned(),
            results_dir: PathBuf::new(),
            configured_weight: 1.0,
            normalized_weight: 1.0,
            capital_group: "test".to_owned(),
            group_weight: 1.0,
            days,
            coverage_statuses: Vec::new(),
        };
        let weekly = aggregate_returns(&source, &dates, CorrelationFrequency::Weekly);
        let monthly = aggregate_returns(&source, &dates, CorrelationFrequency::Monthly);
        assert_eq!(weekly.len(), 2);
        assert_eq!(monthly.len(), 2);
        assert!((weekly[0] + 0.01).abs() < 1e-12);
        assert!((monthly[0] + 0.01).abs() < 1e-12);
        assert!((weekly[1] - 0.05).abs() < 1e-12);
    }

    #[test]
    fn daily_rebalanced_portfolio_combines_weighted_returns() {
        let date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let entry = DateTime::parse_from_rfc3339("2025-01-02T09:30:00-05:00").unwrap();
        let exit = DateTime::parse_from_rfc3339("2025-01-02T12:00:00-05:00").unwrap();
        let sources = vec![
            SourcePortfolio {
                name: "A".to_owned(),
                results_dir: PathBuf::new(),
                configured_weight: 0.75,
                normalized_weight: 0.75,
                capital_group: "daily".to_owned(),
                group_weight: 0.75,
                days: [(
                    date,
                    SourceDay {
                        return_value: 0.10,
                        fills: 1,
                        entry_time: Some(entry),
                        exit_time: Some(exit),
                    },
                )]
                .into_iter()
                .collect(),
                coverage_statuses: Vec::new(),
            },
            SourcePortfolio {
                name: "B".to_owned(),
                results_dir: PathBuf::new(),
                configured_weight: 0.25,
                normalized_weight: 0.25,
                capital_group: "daily".to_owned(),
                group_weight: 0.25,
                days: [(
                    date,
                    SourceDay {
                        return_value: -0.02,
                        fills: 1,
                        entry_time: Some(entry),
                        exit_time: Some(exit),
                    },
                )]
                .into_iter()
                .collect(),
                coverage_statuses: Vec::new(),
            },
        ];
        let (days, sleeves) = build_daily_rebalanced_portfolio(100.0, &sources, &[date]);
        assert!((days[0].ending_equity - 107.0).abs() < 1e-12);
        assert!((sleeves[0].pnl - 7.5).abs() < 1e-12);
        assert!((sleeves[1].pnl + 0.5).abs() < 1e-12);
    }

    #[test]
    fn sequential_mode_reuses_all_capital_and_compounds_in_event_order() {
        let date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let morning_entry = DateTime::parse_from_rfc3339("2025-01-02T09:30:00-05:00").unwrap();
        let morning_exit = DateTime::parse_from_rfc3339("2025-01-02T09:35:00-05:00").unwrap();
        let noon_entry = DateTime::parse_from_rfc3339("2025-01-02T09:40:00-05:00").unwrap();
        let noon_exit = DateTime::parse_from_rfc3339("2025-01-02T12:00:00-05:00").unwrap();
        let sources = vec![
            SourcePortfolio {
                name: "A".to_owned(),
                results_dir: PathBuf::new(),
                configured_weight: 1.0,
                normalized_weight: 0.5,
                capital_group: "A".to_owned(),
                group_weight: 1.0,
                days: [(
                    date,
                    SourceDay {
                        return_value: 0.10,
                        fills: 1,
                        entry_time: Some(morning_entry),
                        exit_time: Some(morning_exit),
                    },
                )]
                .into_iter()
                .collect(),
                coverage_statuses: Vec::new(),
            },
            SourcePortfolio {
                name: "B".to_owned(),
                results_dir: PathBuf::new(),
                configured_weight: 1.0,
                normalized_weight: 0.5,
                capital_group: "B".to_owned(),
                group_weight: 1.0,
                days: [(
                    date,
                    SourceDay {
                        return_value: -0.02,
                        fills: 1,
                        entry_time: Some(noon_entry),
                        exit_time: Some(noon_exit),
                    },
                )]
                .into_iter()
                .collect(),
                coverage_statuses: Vec::new(),
            },
        ];
        validate_non_overlapping_intervals(&sources, &[date]).unwrap();
        let (days, sleeves) =
            build_sequential_full_capital_portfolio(100.0, &sources, &[date]).unwrap();
        assert!((days[0].ending_equity - 107.8).abs() < 1e-12);
        assert!((sleeves[0].pnl - 10.0).abs() < 1e-12);
        assert!((sleeves[1].pnl + 2.2).abs() < 1e-12);
    }

    #[test]
    fn sequential_groups_reuse_capital_then_add_concurrent_full_size_overlays() {
        let date = NaiveDate::from_ymd_opt(2025, 1, 2).unwrap();
        let overnight_entry = DateTime::parse_from_rfc3339("2025-01-01T16:00:00-05:00").unwrap();
        let open = DateTime::parse_from_rfc3339("2025-01-02T09:30:00-05:00").unwrap();
        let orb_entry = DateTime::parse_from_rfc3339("2025-01-02T09:35:00-05:00").unwrap();
        let noon = DateTime::parse_from_rfc3339("2025-01-02T12:00:00-05:00").unwrap();
        let source =
            |name: &str, group: &str, return_value: f64, entry_time, exit_time| SourcePortfolio {
                name: name.to_owned(),
                results_dir: PathBuf::new(),
                configured_weight: 1.0,
                normalized_weight: 1.0 / 3.0,
                capital_group: group.to_owned(),
                group_weight: 1.0,
                days: [(
                    date,
                    SourceDay {
                        return_value,
                        fills: 1,
                        entry_time: Some(entry_time),
                        exit_time: Some(exit_time),
                    },
                )]
                .into_iter()
                .collect(),
                coverage_statuses: Vec::new(),
            };
        let sources = vec![
            source("Overnight", "overnight", 0.10, overnight_entry, open),
            source("ORB", "intraday", 0.02, orb_entry, noon),
            source("Rebound", "intraday", -0.01, open, noon),
        ];
        let (days, sleeves) = build_sequential_group_portfolio(100.0, &sources, &[date]).unwrap();
        assert!((days[0].ending_equity - 111.1).abs() < 1e-12);
        assert_eq!(sleeves.len(), 3);
        assert!((sleeves.iter().map(|row| row.pnl).sum::<f64>() - 11.1).abs() < 1e-12);
    }
}
