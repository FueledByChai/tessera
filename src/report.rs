use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Datelike, NaiveDate};
use polars::prelude::*;
use serde::{Deserialize, Serialize};

use crate::portfolio::CorrelationMatrix;

#[derive(Debug, Deserialize)]
struct RunManifest {
    #[serde(default)]
    strategy_name: Option<String>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    initial_capital: Option<f64>,
    #[serde(default)]
    position_percent: Option<f64>,
    #[serde(default)]
    max_positions_per_day: Option<usize>,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default = "default_annualization_periods")]
    annualization_periods: f64,
    start: NaiveDate,
    end: NaiveDate,
    #[serde(default)]
    symbols: Vec<String>,
    #[serde(default)]
    parameters: BTreeMap<String, String>,
    #[serde(default)]
    correlation_matrices: Vec<CorrelationMatrix>,
    #[serde(default)]
    config: Option<toml::Value>,
}

impl RunManifest {
    fn strategy_name(&self) -> &str {
        self.strategy_name.as_deref().unwrap_or("Limit Buyer")
    }

    fn resolution(&self) -> &str {
        self.resolution
            .as_deref()
            .unwrap_or("Recorded intraday bars")
    }

    fn strategy_config_value(&self, key: &str) -> Option<&toml::Value> {
        self.config.as_ref()?.get("strategy")?.get(key)
    }

    fn initial_capital(&self) -> f64 {
        self.initial_capital
            .or_else(|| {
                self.strategy_config_value("initial_capital")
                    .and_then(toml::Value::as_float)
            })
            .unwrap_or(100_000.0)
    }

    fn position_percent(&self) -> Option<f64> {
        self.position_percent.or_else(|| {
            self.strategy_config_value("position_percent")
                .and_then(toml::Value::as_float)
        })
    }

    fn max_positions_per_day(&self) -> Option<i64> {
        self.max_positions_per_day
            .map(|value| value as i64)
            .or_else(|| {
                self.strategy_config_value("max_positions_per_day")
                    .and_then(toml::Value::as_integer)
            })
    }

    fn random_seed(&self) -> Option<i64> {
        self.strategy_config_value("random_seed")
            .and_then(toml::Value::as_integer)
    }

    fn currency(&self) -> &str {
        self.currency.as_deref().unwrap_or("USD")
    }

    fn annualization_periods(&self) -> f64 {
        self.annualization_periods
    }
}

fn default_annualization_periods() -> f64 {
    252.0
}

#[derive(Debug, Clone)]
struct DailyPoint {
    date: NaiveDate,
    equity: f64,
    fills: usize,
    daily_return: f64,
    drawdown: f64,
}

#[derive(Debug, Clone)]
struct TradePoint {
    symbol: String,
    direction: Option<String>,
    trade_date: String,
    entry_time: String,
    exit_time: String,
    pnl: f64,
    return_percent: f64,
    leverage: Option<f64>,
}

#[derive(Debug, Default)]
struct Coverage {
    covered: usize,
    missing_file: usize,
    missing_session: usize,
}

impl Coverage {
    fn total(&self) -> usize {
        self.covered + self.missing_file + self.missing_session
    }

    fn percent(&self) -> f64 {
        if self.total() == 0 {
            0.0
        } else {
            100.0 * self.covered as f64 / self.total() as f64
        }
    }
}

#[derive(Debug)]
struct Metrics {
    ending_equity: f64,
    total_return: f64,
    cagr: f64,
    sharpe: Option<f64>,
    sortino: Option<f64>,
    calmar: Option<f64>,
    max_drawdown: f64,
    annual_volatility: f64,
    win_rate: f64,
    profit_factor: Option<f64>,
    average_trade: f64,
    best_trade: f64,
    worst_trade: f64,
    average_hold_minutes: f64,
    max_daily_fills: usize,
    average_leverage: Option<f64>,
    maximum_leverage: Option<f64>,
}

#[derive(Debug)]
pub struct ReportSummary {
    pub output: PathBuf,
    pub coverage_percent: f64,
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub calmar: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReportView {
    pub strategy_name: String,
    pub resolution: String,
    pub currency: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub symbols: Vec<String>,
    pub parameters: BTreeMap<String, String>,
    pub correlation_matrices: Vec<CorrelationMatrix>,
    pub metrics: ReportMetricsView,
    pub coverage: ReportCoverageView,
    pub coverage_rows: Vec<ReportCoverageRowView>,
    pub watchlist: Vec<ReportWatchlistRowView>,
    pub daily: Vec<ReportDailyPointView>,
    pub trades: Vec<ReportTradeView>,
    pub trade_breakdown: Vec<ReportTradeBreakdownView>,
    pub yearly_returns: Vec<ReportYearView>,
}

#[derive(Debug, Serialize)]
pub struct ReportCoverageRowView {
    pub trade_date: String,
    pub symbol: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ReportWatchlistRowView {
    pub watch_date: String,
    pub symbol: String,
    pub momentum_rank: usize,
    pub prior_close: f64,
    pub prior_average_dollar_volume: f64,
    pub prior_short_momentum_return_percent: f64,
    pub prior_long_momentum_return_percent: f64,
    pub prior_annualized_realized_volatility_percent: f64,
    pub prior_distance_from_high_percent: f64,
}

#[derive(Debug, Serialize)]
pub struct ReportMetricsView {
    pub initial_capital: f64,
    pub ending_equity: f64,
    pub total_return_percent: f64,
    pub cagr_percent: f64,
    pub sharpe: Option<f64>,
    pub sortino: Option<f64>,
    pub calmar: Option<f64>,
    pub max_drawdown_percent: f64,
    pub annual_volatility_percent: f64,
    pub win_rate_percent: f64,
    pub profit_factor: Option<f64>,
    pub average_trade_pnl: f64,
    pub best_trade_percent: f64,
    pub worst_trade_percent: f64,
    pub average_hold_minutes: f64,
    pub max_daily_fills: usize,
    pub average_leverage: Option<f64>,
    pub maximum_leverage: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReportCoverageView {
    pub covered: usize,
    pub missing_file: usize,
    pub missing_session: usize,
    pub total: usize,
    pub percent: f64,
}

#[derive(Debug, Serialize)]
pub struct ReportDailyPointView {
    pub date: NaiveDate,
    pub equity: f64,
    pub daily_return_percent: f64,
    pub drawdown_percent: f64,
    pub fills: usize,
}

#[derive(Debug, Serialize)]
pub struct ReportTradeView {
    pub symbol: String,
    pub direction: Option<String>,
    pub trade_date: String,
    pub entry_time: String,
    pub exit_time: String,
    pub pnl: f64,
    pub return_percent: f64,
    pub leverage: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReportTradeBreakdownView {
    pub scope: String,
    pub trades: usize,
    pub win_rate_percent: f64,
    pub total_pnl: f64,
    pub average_pnl: f64,
    pub average_return_percent: f64,
    pub profit_factor: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ReportYearView {
    pub year: i32,
    pub months: [Option<f64>; 12],
    pub annual_return_percent: f64,
    pub annual_drawdown_percent: f64,
}

pub fn generate_report(results_dir: &Path, output: Option<&Path>) -> Result<ReportSummary> {
    let manifest: RunManifest = toml::from_str(
        &fs::read_to_string(results_dir.join("run_config.toml"))
            .with_context(|| format!("missing run_config.toml in {}", results_dir.display()))?,
    )?;
    let mut daily = load_daily_equity(
        &results_dir.join("daily_equity.parquet"),
        manifest.initial_capital(),
    )?;
    anyhow::ensure!(!daily.is_empty(), "daily_equity.parquet contains no rows");
    let trades = load_trades(&results_dir.join("trades.parquet"))?;
    let (coverage, _) = load_coverage(&results_dir.join("coverage.parquet"))?;
    apply_drawdowns(&mut daily, manifest.initial_capital());
    let metrics = calculate_metrics(&manifest, &daily, &trades);
    let output = output
        .map(Path::to_owned)
        .unwrap_or_else(|| results_dir.join("report.html"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &output,
        render_report(&manifest, &daily, &trades, &coverage, &metrics),
    )?;
    Ok(ReportSummary {
        output,
        coverage_percent: coverage.percent(),
        sharpe: metrics.sharpe,
        sortino: metrics.sortino,
        calmar: metrics.calmar,
    })
}

pub fn load_report_view(results_dir: &Path) -> Result<ReportView> {
    let manifest: RunManifest = toml::from_str(
        &fs::read_to_string(results_dir.join("run_config.toml"))
            .with_context(|| format!("missing run_config.toml in {}", results_dir.display()))?,
    )?;
    let initial_capital = manifest.initial_capital();
    let mut daily = load_daily_equity(&results_dir.join("daily_equity.parquet"), initial_capital)?;
    anyhow::ensure!(!daily.is_empty(), "daily_equity.parquet contains no rows");
    let trades = load_trades(&results_dir.join("trades.parquet"))?;
    let (coverage, coverage_rows) = load_coverage(&results_dir.join("coverage.parquet"))?;
    let watchlist = load_watchlist(&results_dir.join("watchlist.parquet"))?;
    apply_drawdowns(&mut daily, initial_capital);
    let metrics = calculate_metrics(&manifest, &daily, &trades);

    let mut month_boundaries = BTreeMap::<(i32, u32), (f64, f64)>::new();
    let mut previous = initial_capital;
    for row in &daily {
        let entry = month_boundaries
            .entry((row.date.year(), row.date.month()))
            .or_insert((previous, row.equity));
        entry.1 = row.equity;
        previous = row.equity;
    }
    let yearly_performance = yearly_performance(&daily, initial_capital);
    let yearly_returns = yearly_performance
        .iter()
        .map(|(year, (annual_return, annual_drawdown))| {
            let mut months = [None; 12];
            for month in 1..=12 {
                if let Some((start, end)) = month_boundaries.get(&(*year, month)) {
                    months[(month - 1) as usize] = Some(100.0 * (end / start - 1.0));
                }
            }
            ReportYearView {
                year: *year,
                months,
                annual_return_percent: *annual_return,
                annual_drawdown_percent: *annual_drawdown,
            }
        })
        .collect();

    let mut scopes = vec![("All", trades.iter().collect::<Vec<_>>())];
    if trades.iter().any(|trade| trade.direction.is_some()) {
        scopes.push((
            "Long",
            trades
                .iter()
                .filter(|trade| trade.direction.as_deref() == Some("long"))
                .collect(),
        ));
        scopes.push((
            "Short",
            trades
                .iter()
                .filter(|trade| trade.direction.as_deref() == Some("short"))
                .collect(),
        ));
    }
    let trade_breakdown = scopes
        .into_iter()
        .map(|(scope, scoped)| {
            let breakdown = calculate_trade_breakdown(&scoped);
            ReportTradeBreakdownView {
                scope: scope.to_owned(),
                trades: breakdown.trades,
                win_rate_percent: breakdown.win_rate,
                total_pnl: breakdown.total_pnl,
                average_pnl: breakdown.average_pnl,
                average_return_percent: breakdown.average_return,
                profit_factor: breakdown.profit_factor,
            }
        })
        .collect();

    Ok(ReportView {
        strategy_name: manifest.strategy_name().to_owned(),
        resolution: manifest.resolution().to_owned(),
        currency: manifest.currency().to_owned(),
        start: manifest.start,
        end: manifest.end,
        symbols: manifest.symbols,
        parameters: manifest.parameters,
        correlation_matrices: manifest.correlation_matrices,
        metrics: ReportMetricsView {
            initial_capital,
            ending_equity: metrics.ending_equity,
            total_return_percent: 100.0 * metrics.total_return,
            cagr_percent: 100.0 * metrics.cagr,
            sharpe: metrics.sharpe,
            sortino: metrics.sortino,
            calmar: metrics.calmar,
            max_drawdown_percent: -100.0 * metrics.max_drawdown,
            annual_volatility_percent: 100.0 * metrics.annual_volatility,
            win_rate_percent: metrics.win_rate,
            profit_factor: metrics.profit_factor,
            average_trade_pnl: metrics.average_trade,
            best_trade_percent: metrics.best_trade,
            worst_trade_percent: metrics.worst_trade,
            average_hold_minutes: metrics.average_hold_minutes,
            max_daily_fills: metrics.max_daily_fills,
            average_leverage: metrics.average_leverage,
            maximum_leverage: metrics.maximum_leverage,
        },
        coverage: ReportCoverageView {
            covered: coverage.covered,
            missing_file: coverage.missing_file,
            missing_session: coverage.missing_session,
            total: coverage.total(),
            percent: coverage.percent(),
        },
        coverage_rows,
        watchlist,
        daily: daily
            .into_iter()
            .map(|row| ReportDailyPointView {
                date: row.date,
                equity: row.equity,
                daily_return_percent: 100.0 * row.daily_return,
                drawdown_percent: 100.0 * row.drawdown,
                fills: row.fills,
            })
            .collect(),
        trades: trades
            .into_iter()
            .map(|trade| ReportTradeView {
                symbol: trade.symbol,
                direction: trade.direction,
                trade_date: trade.trade_date,
                entry_time: trade.entry_time,
                exit_time: trade.exit_time,
                pnl: trade.pnl,
                return_percent: trade.return_percent,
                leverage: trade.leverage,
            })
            .collect(),
        trade_breakdown,
        yearly_returns,
    })
}

fn load_daily_equity(path: &Path, starting_equity: f64) -> Result<Vec<DailyPoint>> {
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let dates = frame.column("date")?.str()?;
    let equities = frame.column("ending_equity")?.f64()?;
    let fills = frame.column("fills")?.u64()?;
    let mut rows = Vec::with_capacity(frame.height());
    for index in 0..frame.height() {
        let equity = equities.get(index).context("ending_equity is null")?;
        rows.push(DailyPoint {
            date: parse_date(dates.get(index).context("date is null")?)?,
            equity,
            fills: fills.get(index).context("fills is null")? as usize,
            daily_return: 0.0,
            drawdown: 0.0,
        });
    }
    rows.sort_unstable_by_key(|row| row.date);
    let mut previous = starting_equity;
    for row in &mut rows {
        row.daily_return = row.equity / previous - 1.0;
        previous = row.equity;
    }
    Ok(rows)
}

fn load_trades(path: &Path) -> Result<Vec<TradePoint>> {
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let symbols = frame.column("symbol")?.str()?;
    let dates = frame.column("trade_date")?.str()?;
    let entries = frame.column("entry_time")?.str()?;
    let exits = frame.column("exit_time")?.str()?;
    let pnls = frame.column("pnl")?.f64()?;
    let returns = frame.column("return_percent")?.f64()?;
    let directions = frame
        .column("direction")
        .ok()
        .and_then(|column| column.str().ok());
    let leverages = frame
        .column("leverage")
        .ok()
        .and_then(|column| column.f64().ok());
    let mut rows = Vec::with_capacity(frame.height());
    for index in 0..frame.height() {
        rows.push(TradePoint {
            symbol: symbols
                .get(index)
                .context("trade symbol is null")?
                .to_owned(),
            direction: directions
                .and_then(|values| values.get(index))
                .map(str::to_owned),
            trade_date: dates.get(index).context("trade date is null")?.to_owned(),
            entry_time: entries.get(index).context("entry time is null")?.to_owned(),
            exit_time: exits.get(index).context("exit time is null")?.to_owned(),
            pnl: pnls.get(index).context("trade pnl is null")?,
            return_percent: returns.get(index).context("trade return is null")?,
            leverage: leverages.and_then(|values| values.get(index)),
        });
    }
    Ok(rows)
}

fn load_coverage(path: &Path) -> Result<(Coverage, Vec<ReportCoverageRowView>)> {
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let dates = frame.column("trade_date")?.str()?;
    let symbols = frame.column("symbol")?.str()?;
    let statuses = frame.column("status")?.str()?;
    let mut coverage = Coverage::default();
    let mut rows = Vec::with_capacity(frame.height());
    for index in 0..frame.height() {
        let Some(status) = statuses.get(index) else {
            continue;
        };
        match status {
            "covered" => coverage.covered += 1,
            "missing_file" => coverage.missing_file += 1,
            _ => coverage.missing_session += 1,
        }
        rows.push(ReportCoverageRowView {
            trade_date: dates.get(index).unwrap_or_default().to_owned(),
            symbol: symbols.get(index).unwrap_or_default().to_owned(),
            status: status.to_owned(),
        });
    }
    Ok((coverage, rows))
}

fn load_watchlist(path: &Path) -> Result<Vec<ReportWatchlistRowView>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let frame = ParquetReader::new(File::open(path)?).finish()?;
    let required = [
        "watch_date",
        "symbol",
        "momentum_rank",
        "prior_close",
        "prior_average_dollar_volume",
        "prior_short_momentum_return_percent",
        "prior_long_momentum_return_percent",
        "prior_annualized_realized_volatility_percent",
        "prior_distance_from_high_percent",
    ];
    if required.iter().any(|name| frame.column(name).is_err()) {
        // Strategies may preserve their own watchlist audit beside the shared
        // report bundle. It is optional UI data and must not invalidate an
        // otherwise complete standardized result.
        return Ok(Vec::new());
    }
    let dates = frame.column("watch_date")?.str()?;
    let symbols = frame.column("symbol")?.str()?;
    let ranks = frame.column("momentum_rank")?.u64()?;
    let prior_close = frame.column("prior_close")?.f64()?;
    let adv = frame.column("prior_average_dollar_volume")?.f64()?;
    let short_momentum = frame.column("prior_short_momentum_return_percent")?.f64()?;
    let long_momentum = frame.column("prior_long_momentum_return_percent")?.f64()?;
    let realized_volatility = frame
        .column("prior_annualized_realized_volatility_percent")?
        .f64()?;
    let distance_from_high = frame.column("prior_distance_from_high_percent")?.f64()?;
    let start = frame.height().saturating_sub(1_000);
    let mut rows = Vec::with_capacity(frame.height() - start);
    for index in start..frame.height() {
        rows.push(ReportWatchlistRowView {
            watch_date: dates.get(index).unwrap_or_default().to_owned(),
            symbol: symbols.get(index).unwrap_or_default().to_owned(),
            momentum_rank: ranks.get(index).unwrap_or_default() as usize,
            prior_close: prior_close.get(index).unwrap_or_default(),
            prior_average_dollar_volume: adv.get(index).unwrap_or_default(),
            prior_short_momentum_return_percent: short_momentum.get(index).unwrap_or_default(),
            prior_long_momentum_return_percent: long_momentum.get(index).unwrap_or_default(),
            prior_annualized_realized_volatility_percent: realized_volatility
                .get(index)
                .unwrap_or_default(),
            prior_distance_from_high_percent: distance_from_high.get(index).unwrap_or_default(),
        });
    }
    Ok(rows)
}

fn apply_drawdowns(rows: &mut [DailyPoint], starting_equity: f64) {
    let mut peak = starting_equity;
    for row in rows {
        peak = peak.max(row.equity);
        row.drawdown = row.equity / peak - 1.0;
    }
}

fn calculate_metrics(
    manifest: &RunManifest,
    daily: &[DailyPoint],
    trades: &[TradePoint],
) -> Metrics {
    let starting = manifest.initial_capital();
    let ending = daily.last().map_or(starting, |row| row.equity);
    let total_return = ending / starting - 1.0;
    let calendar_days = (manifest.end - manifest.start).num_days().max(1) as f64;
    let cagr = (ending / starting).powf(365.25 / calendar_days) - 1.0;
    let returns: Vec<f64> = daily.iter().map(|row| row.daily_return).collect();
    let mean_return = mean(&returns);
    let standard_deviation = sample_standard_deviation(&returns);
    let annualization = manifest.annualization_periods();
    let annual_volatility = standard_deviation * annualization.sqrt();
    let sharpe = (standard_deviation > f64::EPSILON)
        .then(|| mean_return / standard_deviation * annualization.sqrt());
    let downside_deviation = (returns
        .iter()
        .map(|value| value.min(0.0).powi(2))
        .sum::<f64>()
        / returns.len().max(1) as f64)
        .sqrt();
    let sortino = (downside_deviation > f64::EPSILON)
        .then(|| mean_return / downside_deviation * annualization.sqrt());
    let max_drawdown = daily
        .iter()
        .map(|row| row.drawdown.abs())
        .fold(0.0_f64, f64::max);
    let calmar = (max_drawdown > f64::EPSILON).then(|| cagr / max_drawdown);
    let gross_profit: f64 = trades.iter().map(|trade| trade.pnl.max(0.0)).sum();
    let gross_loss: f64 = trades.iter().map(|trade| trade.pnl.min(0.0).abs()).sum();
    let profit_factor = (gross_loss > f64::EPSILON).then(|| gross_profit / gross_loss);
    let wins = trades.iter().filter(|trade| trade.pnl > 0.0).count();
    let hold_minutes: Vec<f64> = trades
        .iter()
        .filter_map(|trade| {
            let entry = DateTime::parse_from_rfc3339(&trade.entry_time).ok()?;
            let exit = DateTime::parse_from_rfc3339(&trade.exit_time).ok()?;
            Some((exit - entry).num_minutes() as f64)
        })
        .collect();
    let leverages: Vec<f64> = trades.iter().filter_map(|trade| trade.leverage).collect();
    Metrics {
        ending_equity: ending,
        total_return,
        cagr,
        sharpe,
        sortino,
        calmar,
        max_drawdown,
        annual_volatility,
        win_rate: 100.0 * wins as f64 / trades.len().max(1) as f64,
        profit_factor,
        average_trade: mean(&trades.iter().map(|trade| trade.pnl).collect::<Vec<_>>()),
        best_trade: trades
            .iter()
            .map(|trade| trade.return_percent)
            .reduce(f64::max)
            .unwrap_or(0.0),
        worst_trade: trades
            .iter()
            .map(|trade| trade.return_percent)
            .reduce(f64::min)
            .unwrap_or(0.0),
        average_hold_minutes: mean(&hold_minutes),
        max_daily_fills: daily.iter().map(|row| row.fills).max().unwrap_or(0),
        average_leverage: (!leverages.is_empty()).then(|| mean(&leverages)),
        maximum_leverage: leverages.into_iter().reduce(f64::max),
    }
}

fn render_report(
    manifest: &RunManifest,
    daily: &[DailyPoint],
    trades: &[TradePoint],
    coverage: &Coverage,
    metrics: &Metrics,
) -> String {
    let mut report_warnings = if coverage.percent() < 99.5 {
        format!(
            "<section class=\"warning\"><strong>Partial-universe result</strong><span>Only {:.2}% of candidate-days have usable intraday bars. Performance metrics are diagnostic, not strategy-valid.</span></section>",
            coverage.percent()
        )
    } else {
        "<section class=\"complete\"><strong>Coverage qualified</strong><span>At least 99.5% of candidate-days have usable intraday bars.</span></section>".to_owned()
    };
    if manifest
        .parameters
        .get("Combination method")
        .is_some_and(|value| value.contains("additive full-size overlays"))
    {
        report_warnings.push_str(
            "<section class=\"warning\"><strong>Unconstrained capital overlay</strong><span>Concurrent strategy sleeves are applied at their full standalone sizing. Portfolio gross allocation may exceed 100%; financing costs and a portfolio-level leverage cap are not modeled.</span></section>",
        );
    }
    if let Some(note) = manifest.parameters.get("Universe limitation") {
        report_warnings.push_str(&format!(
            "<section class=\"warning\"><strong>Universe-bias warning</strong><span>{}</span></section>",
            escape_html(note)
        ));
    }
    let cards = [
        (
            "Ending equity",
            currency(metrics.ending_equity, manifest.currency()),
        ),
        ("Total return", percent(metrics.total_return)),
        ("CAGR", percent(metrics.cagr)),
        ("Sharpe", optional_ratio(metrics.sharpe)),
        ("Sortino", optional_ratio(metrics.sortino)),
        ("Calmar", optional_ratio(metrics.calmar)),
        ("Max drawdown", percent(-metrics.max_drawdown)),
        ("Coverage", format!("{:.2}%", coverage.percent())),
    ]
    .into_iter()
    .map(|(label, value)| {
        format!(
            "<div class=\"metric\"><span>{}</span><strong>{}</strong></div>",
            label, value
        )
    })
    .collect::<String>();
    let equity: Vec<f64> = daily.iter().map(|row| row.equity).collect();
    let drawdowns: Vec<f64> = daily.iter().map(|row| 100.0 * row.drawdown).collect();
    let labels: Vec<String> = daily.iter().map(|row| row.date.to_string()).collect();
    let rolling = rolling_sharpe(daily, 21, manifest.annualization_periods());
    let rolling_values: Vec<f64> = rolling.iter().map(|(_, value)| *value).collect();
    let rolling_labels: Vec<String> = rolling.iter().map(|(date, _)| date.to_string()).collect();
    let mut metrics_table = format!(
        "<tr><th>Annualized volatility</th><td>{}</td><th>Win rate</th><td>{:.2}%</td></tr>\
         <tr><th>Profit factor</th><td>{}</td><th>Average trade P&amp;L</th><td>{}</td></tr>\
         <tr><th>Best trade</th><td>{:.2}%</td><th>Worst trade</th><td>{:.2}%</td></tr>\
         <tr><th>Trades</th><td>{}</td><th>Average hold</th><td>{:.1} min</td></tr>\
         <tr><th>Active sessions</th><td>{}</td><th>Max fills in one day</th><td>{}</td></tr>",
        percent(metrics.annual_volatility),
        metrics.win_rate,
        optional_ratio(metrics.profit_factor),
        currency(metrics.average_trade, manifest.currency()),
        metrics.best_trade,
        metrics.worst_trade,
        trades.len(),
        metrics.average_hold_minutes,
        daily.iter().filter(|row| row.fills > 0).count(),
        metrics.max_daily_fills,
    );
    if let (Some(average), Some(maximum)) = (metrics.average_leverage, metrics.maximum_leverage) {
        metrics_table.push_str(&format!(
            "<tr><th>Average gross exposure</th><td>{average:.3}×</td><th>Peak gross exposure</th><td>{maximum:.3}×</td></tr>"
        ));
    }
    let coverage_table = format!(
        "<tr><th>Covered candidate-days</th><td>{}</td></tr>\
         <tr><th>Missing file</th><td>{}</td></tr>\
         <tr><th>Missing session</th><td>{}</td></tr>\
         <tr><th>Total candidate-days</th><td>{}</td></tr>",
        coverage.covered,
        coverage.missing_file,
        coverage.missing_session,
        coverage.total()
    );
    let symbol_scope = if manifest.symbols.is_empty() {
        "All watchlist symbols".to_owned()
    } else {
        manifest.symbols.join(", ")
    };
    let mut details = String::new();
    details.push_str(&detail_row(
        "Period",
        &format!("{} to {}", manifest.start, manifest.end),
    ));
    details.push_str(&detail_row("Resolution", manifest.resolution()));
    details.push_str(&detail_row("Scope", &symbol_scope));
    details.push_str(&detail_row(
        "Initial capital",
        &currency(manifest.initial_capital(), manifest.currency()),
    ));
    if let Some(position_percent) = manifest.position_percent() {
        details.push_str(&detail_row(
            "Position size",
            &format!("{:.1}%", 100.0 * position_percent),
        ));
    }
    if let Some(maximum) = manifest.max_positions_per_day() {
        details.push_str(&detail_row("Daily position cap", &maximum.to_string()));
    }
    if let Some(seed) = manifest.random_seed() {
        details.push_str(&detail_row("Random seed", &seed.to_string()));
    }
    let bps_cost_model = manifest
        .strategy_config_value("execution_cost_model")
        .and_then(toml::Value::as_str)
        == Some("all_in_round_trip_bps");
    if bps_cost_model {
        if let Some(bps) = manifest
            .strategy_config_value("all_in_round_trip_bps")
            .and_then(toml::Value::as_float)
        {
            details.push_str(&detail_row(
                "Execution costs",
                &format!("{bps:.1} bps all-in round trip"),
            ));
        }
    } else if !manifest.parameters.contains_key("Slippage")
        && let (Some(ticks), Some(tick_size)) = (
            manifest
                .strategy_config_value("slippage_ticks")
                .and_then(toml::Value::as_integer),
            manifest
                .strategy_config_value("tick_size")
                .and_then(toml::Value::as_float),
        )
    {
        details.push_str(&detail_row(
            "Slippage",
            &format!("{ticks} adverse tick per fill at ${tick_size:.4} per tick"),
        ));
    }
    if !bps_cost_model
        && !manifest.parameters.contains_key("Commission")
        && let Some(commission) = manifest
            .strategy_config_value("commission_per_share")
            .and_then(toml::Value::as_float)
    {
        details.push_str(&detail_row(
            "Commission",
            &format!("${commission:.4} per share per fill"),
        ));
    }
    for (label, value) in &manifest.parameters {
        details.push_str(&detail_row(label, value));
    }
    let recent_trades = trade_table(trades, manifest.currency());
    let trade_breakdown = trade_breakdown_table(trades, manifest.currency());
    let correlation_matrices = render_correlation_matrices(&manifest.correlation_matrices);

    HTML_TEMPLATE
        .replace("{{STRATEGY_NAME}}", &escape_html(manifest.strategy_name()))
        .replace("{{COVERAGE_WARNING}}", &report_warnings)
        .replace("{{CARDS}}", &cards)
        .replace(
            "{{EQUITY_CHART}}",
            &line_chart(
                &equity,
                &labels,
                ChartKind::LogCurrency,
                manifest.currency(),
            ),
        )
        .replace(
            "{{DRAWDOWN_CHART}}",
            &line_chart(&drawdowns, &labels, ChartKind::Percent, manifest.currency()),
        )
        .replace(
            "{{ROLLING_CHART}}",
            &line_chart(
                &rolling_values,
                &rolling_labels,
                ChartKind::Ratio,
                manifest.currency(),
            ),
        )
        .replace("{{MONTHLY_HEATMAP}}", &monthly_heatmap(daily, manifest))
        .replace("{{CORRELATION_MATRICES}}", &correlation_matrices)
        .replace("{{TRADE_HISTOGRAM}}", &trade_histogram(trades))
        .replace("{{METRICS_TABLE}}", &metrics_table)
        .replace("{{COVERAGE_TABLE}}", &coverage_table)
        .replace("{{RUN_DETAILS}}", &details)
        .replace("{{TRADE_BREAKDOWN}}", &trade_breakdown)
        .replace("{{RECENT_TRADES}}", &recent_trades)
        .replace(
            "{{ANNUALIZATION}}",
            &format!("{:.2}", manifest.annualization_periods()),
        )
}

fn render_correlation_matrices(matrices: &[CorrelationMatrix]) -> String {
    if matrices.is_empty() {
        return String::new();
    }
    let panels = matrices
        .iter()
        .map(|matrix| {
            let header = matrix
                .labels
                .iter()
                .map(|label| {
                    format!(
                        "<th title=\"{}\">{}</th>",
                        escape_html(label),
                        escape_html(label)
                    )
                })
                .collect::<String>();
            let rows = matrix
                .labels
                .iter()
                .enumerate()
                .map(|(row_index, label)| {
                    let cells = matrix.values[row_index]
                        .iter()
                        .map(|value| {
                            if !value.is_finite() {
                                "<td class=\"missing\">N/A</td>".to_owned()
                            } else {
                                let class = if *value >= 0.0 { "positive" } else { "negative" };
                                let heat = value.abs().min(1.0) * 0.72;
                                format!(
                                    "<td class=\"{class}\" style=\"--heat:{heat:.3}\">{value:.2}</td>"
                                )
                            }
                        })
                        .collect::<String>();
                    format!("<tr><th>{}</th>{cells}</tr>", escape_html(label))
                })
                .collect::<String>();
            format!(
                "<div class=\"panel correlation-panel\"><h2>{} return correlation <span class=\"observations\">({} observations)</span></h2><table class=\"correlation heatmap\"><thead><tr><th></th>{header}</tr></thead><tbody>{rows}</tbody></table></div>",
                escape_html(&matrix.frequency),
                matrix.observations,
            )
        })
        .collect::<String>();
    format!("<section class=\"correlation-grid\">{panels}</section>")
}

#[derive(Clone, Copy)]
enum ChartKind {
    LogCurrency,
    Percent,
    Ratio,
}

fn line_chart(values: &[f64], labels: &[String], kind: ChartKind, currency_code: &str) -> String {
    if values.is_empty() {
        return "<p class=\"empty\">No observations</p>".to_owned();
    }
    if matches!(kind, ChartKind::LogCurrency) && values.iter().any(|value| *value <= 0.0) {
        return "<p class=\"empty\">Log-scale chart requires positive values</p>".to_owned();
    }
    let scaled_values: Vec<f64> = values
        .iter()
        .map(|value| match kind {
            ChartKind::LogCurrency => value.ln(),
            _ => *value,
        })
        .collect();
    let width = 1000.0;
    let height = 280.0;
    let left = 72.0;
    let right = 980.0;
    let top = 18.0;
    let bottom = 238.0;
    let mut minimum = scaled_values.iter().copied().fold(f64::INFINITY, f64::min);
    let mut maximum = scaled_values
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    if (maximum - minimum).abs() < f64::EPSILON {
        minimum -= 1.0;
        maximum += 1.0;
    }
    let padding = 0.05 * (maximum - minimum);
    minimum -= padding;
    maximum += padding;
    let points: Vec<(f64, f64)> = scaled_values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = left + index as f64 / (values.len() - 1).max(1) as f64 * (right - left);
            let y = bottom - (value - minimum) / (maximum - minimum) * (bottom - top);
            (x, y)
        })
        .collect();
    let path = points
        .iter()
        .enumerate()
        .map(|(index, (x, y))| format!("{} {:.2} {:.2}", if index == 0 { "M" } else { "L" }, x, y))
        .collect::<Vec<_>>()
        .join(" ");
    let fill_path = format!(
        "{} L {:.2} {:.2} L {:.2} {:.2} Z",
        path, right, bottom, left, bottom
    );
    let grid = (0..=4)
        .map(|index| {
            let ratio = index as f64 / 4.0;
            let y = top + ratio * (bottom - top);
            let scaled_value = maximum - ratio * (maximum - minimum);
            let value = match kind {
                ChartKind::LogCurrency => scaled_value.exp(),
                _ => scaled_value,
            };
            format!(
                "<line x1=\"{left}\" y1=\"{y:.2}\" x2=\"{right}\" y2=\"{y:.2}\" class=\"gridline\"/><text x=\"64\" y=\"{:.2}\" text-anchor=\"end\" class=\"axis\">{}</text>",
                y + 4.0,
                chart_value(value, kind, currency_code)
            )
        })
        .collect::<String>();
    format!(
        "<svg viewBox=\"0 0 {width} {height}\" role=\"img\" aria-label=\"{}\">\
         {grid}<path d=\"{fill_path}\" class=\"area\"/><path d=\"{path}\" class=\"line\"/>\
         <text x=\"{left}\" y=\"264\" class=\"axis\">{}</text>\
         <text x=\"{right}\" y=\"264\" text-anchor=\"end\" class=\"axis\">{}</text></svg>",
        if matches!(kind, ChartKind::LogCurrency) {
            "Logarithmic equity time series chart"
        } else {
            "Time series chart"
        },
        labels.first().map(String::as_str).unwrap_or(""),
        labels.last().map(String::as_str).unwrap_or("")
    )
}

fn monthly_heatmap(daily: &[DailyPoint], manifest: &RunManifest) -> String {
    let mut months = BTreeMap::<(i32, u32), (f64, f64)>::new();
    let mut previous = manifest.initial_capital();
    for row in daily {
        let key = (row.date.year(), row.date.month());
        let entry = months.entry(key).or_insert((previous, row.equity));
        entry.1 = row.equity;
        previous = row.equity;
    }
    let years: Vec<i32> = months
        .keys()
        .map(|(year, _)| *year)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let headers = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let yearly = yearly_performance(daily, manifest.initial_capital());
    let mut html = format!(
        "<table class=\"heatmap\"><thead><tr><th>Year</th>{}<th>Annual Return</th><th>Annual Max DD</th></tr></thead><tbody>",
        headers
            .iter()
            .map(|month| format!("<th>{month}</th>"))
            .collect::<String>()
    );
    for year in years {
        html.push_str(&format!("<tr><th>{year}</th>"));
        for month in 1..=12 {
            if let Some((start, end)) = months.get(&(year, month)) {
                let value = 100.0 * (end / start - 1.0);
                html.push_str(&heatmap_cell(value, ""));
            } else {
                html.push_str("<td class=\"missing\">—</td>");
            }
        }
        let (annual_return, annual_drawdown) = yearly[&year];
        html.push_str(&heatmap_cell(annual_return, "annual"));
        html.push_str(&heatmap_cell(annual_drawdown, "annual"));
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

fn yearly_performance(daily: &[DailyPoint], starting_equity: f64) -> BTreeMap<i32, (f64, f64)> {
    let mut accumulators = BTreeMap::<i32, (f64, f64, f64, f64)>::new();
    let mut previous_equity = starting_equity;
    for row in daily {
        let entry = accumulators.entry(row.date.year()).or_insert((
            previous_equity,
            row.equity,
            previous_equity,
            0.0,
        ));
        entry.1 = row.equity;
        entry.2 = entry.2.max(row.equity);
        entry.3 = entry.3.min(row.equity / entry.2 - 1.0);
        previous_equity = row.equity;
    }
    accumulators
        .into_iter()
        .map(|(year, (start, end, _, maximum_drawdown))| {
            (
                year,
                (100.0 * (end / start - 1.0), 100.0 * maximum_drawdown),
            )
        })
        .collect()
}

fn heatmap_cell(value: f64, extra_class: &str) -> String {
    let intensity = (value.abs() / 10.0).min(1.0) * 0.72 + 0.12;
    format!(
        "<td class=\"{} {extra_class}\" style=\"--heat:{intensity:.2}\">{value:.1}%</td>",
        if value >= 0.0 { "positive" } else { "negative" }
    )
}

fn trade_histogram(trades: &[TradePoint]) -> String {
    if trades.is_empty() {
        return "<p class=\"empty\">No trades</p>".to_owned();
    }
    let values: Vec<f64> = trades.iter().map(|trade| trade.return_percent).collect();
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let bins = 16_usize;
    let span = (maximum - minimum).max(0.01);
    let mut counts = vec![0_usize; bins];
    for value in values {
        let index = (((value - minimum) / span * bins as f64).floor() as usize).min(bins - 1);
        counts[index] += 1;
    }
    let max_count = counts.iter().copied().max().unwrap_or(1) as f64;
    let bars = counts
        .iter()
        .enumerate()
        .map(|(index, count)| {
            let bar_width = 850.0 / bins as f64;
            let x = 90.0 + index as f64 * bar_width;
            let height = *count as f64 / max_count * 180.0;
            let midpoint = minimum + (index as f64 + 0.5) / bins as f64 * span;
            format!("<rect x=\"{x:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{height:.2}\" class=\"{}\"><title>{midpoint:.2}%: {count} trades</title></rect>", 218.0 - height, bar_width - 3.0, if midpoint >= 0.0 { "bar-positive" } else { "bar-negative" })
        })
        .collect::<String>();
    format!(
        "<svg viewBox=\"0 0 1000 260\" role=\"img\" aria-label=\"Trade return histogram\">{bars}<line x1=\"90\" y1=\"218\" x2=\"940\" y2=\"218\" class=\"gridline\"/><text x=\"90\" y=\"244\" class=\"axis\">{minimum:.1}%</text><text x=\"940\" y=\"244\" text-anchor=\"end\" class=\"axis\">{maximum:.1}%</text></svg>"
    )
}

fn trade_table(trades: &[TradePoint], currency_code: &str) -> String {
    if trades.is_empty() {
        return "<p class=\"empty\">No trades</p>".to_owned();
    }
    let rows = trades
        .iter()
        .rev()
        .take(12)
        .map(|trade| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{:.2}%</td><td>{}</td></tr>",
                escape_html(&trade.trade_date),
                escape_html(&trade.symbol),
                trade.return_percent,
                currency(trade.pnl, currency_code)
            )
        })
        .collect::<String>();
    format!(
        "<table><thead><tr><th>Date</th><th>Symbol</th><th>Return</th><th>P&amp;L</th></tr></thead><tbody>{rows}</tbody></table>"
    )
}

#[derive(Debug)]
struct TradeBreakdown {
    trades: usize,
    win_rate: f64,
    total_pnl: f64,
    average_pnl: f64,
    average_return: f64,
    profit_factor: Option<f64>,
}

fn trade_breakdown_table(trades: &[TradePoint], currency_code: &str) -> String {
    if trades.is_empty() {
        return "<p class=\"empty\">No trades</p>".to_owned();
    }
    let mut scopes = vec![("All", trades.iter().collect::<Vec<_>>())];
    if trades.iter().any(|trade| trade.direction.is_some()) {
        scopes.push((
            "Long",
            trades
                .iter()
                .filter(|trade| trade.direction.as_deref() == Some("long"))
                .collect(),
        ));
        scopes.push((
            "Short",
            trades
                .iter()
                .filter(|trade| trade.direction.as_deref() == Some("short"))
                .collect(),
        ));
    }
    let rows = scopes
        .into_iter()
        .map(|(label, scoped)| {
            let metrics = calculate_trade_breakdown(&scoped);
            format!(
                "<tr><th>{label}</th><td>{}</td><td>{:.2}%</td><td>{}</td><td>{}</td><td>{:.3}%</td><td>{}</td></tr>",
                metrics.trades,
                metrics.win_rate,
                currency(metrics.total_pnl, currency_code),
                currency(metrics.average_pnl, currency_code),
                metrics.average_return,
                optional_ratio(metrics.profit_factor),
            )
        })
        .collect::<String>();
    format!(
        "<table><thead><tr><th>Scope</th><th>Trades</th><th>Win rate</th><th>Net P&amp;L</th><th>Average P&amp;L</th><th>Average return</th><th>Profit factor</th></tr></thead><tbody>{rows}</tbody></table>"
    )
}

fn calculate_trade_breakdown(trades: &[&TradePoint]) -> TradeBreakdown {
    let gross_profit: f64 = trades.iter().map(|trade| trade.pnl.max(0.0)).sum();
    let gross_loss: f64 = trades.iter().map(|trade| trade.pnl.min(0.0).abs()).sum();
    let wins = trades.iter().filter(|trade| trade.pnl > 0.0).count();
    let total_pnl = trades.iter().map(|trade| trade.pnl).sum();
    TradeBreakdown {
        trades: trades.len(),
        win_rate: 100.0 * wins as f64 / trades.len().max(1) as f64,
        total_pnl,
        average_pnl: total_pnl / trades.len().max(1) as f64,
        average_return: mean(
            &trades
                .iter()
                .map(|trade| trade.return_percent)
                .collect::<Vec<_>>(),
        ),
        profit_factor: (gross_loss > f64::EPSILON).then(|| gross_profit / gross_loss),
    }
}

fn rolling_sharpe(
    rows: &[DailyPoint],
    window: usize,
    annualization_periods: f64,
) -> Vec<(NaiveDate, f64)> {
    if rows.len() < window {
        return Vec::new();
    }
    rows.windows(window)
        .filter_map(|slice| {
            let returns: Vec<f64> = slice.iter().map(|row| row.daily_return).collect();
            let deviation = sample_standard_deviation(&returns);
            (deviation > f64::EPSILON).then(|| {
                (
                    slice.last().expect("rolling window is non-empty").date,
                    mean(&returns) / deviation * annualization_periods.sqrt(),
                )
            })
        })
        .collect()
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
    let average = mean(values);
    (values
        .iter()
        .map(|value| (value - average).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64)
        .sqrt()
}

fn currency(value: f64, currency_code: &str) -> String {
    let prefix = if currency_code.eq_ignore_ascii_case("USD") {
        "$".to_owned()
    } else {
        format!("{} ", currency_code.to_uppercase())
    };
    if value < 0.0 {
        format!("-{prefix}{:.2}", value.abs())
    } else {
        format!("{prefix}{value:.2}")
    }
}

fn percent(value: f64) -> String {
    format!("{:.2}%", 100.0 * value)
}

fn optional_ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |value| format!("{value:.2}"))
}

fn chart_value(value: f64, kind: ChartKind, currency_code: &str) -> String {
    match kind {
        ChartKind::LogCurrency => currency(value, currency_code)
            .trim_end_matches(".00")
            .to_owned(),
        ChartKind::Percent => format!("{value:.1}%"),
        ChartKind::Ratio => format!("{value:.1}"),
    }
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid report date {value:?}"))
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn detail_row(label: &str, value: &str) -> String {
    format!(
        "<tr><th>{}</th><td>{}</td></tr>",
        escape_html(label),
        escape_html(value)
    )
}

const HTML_TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Backtest Tear Sheet</title>
<style>
:root{color-scheme:light dark;--bg:#f4f7fb;--panel:#fff;--text:#172033;--muted:#667085;--border:#dfe5ee;--accent:#0f8b8d;--accent-soft:#d9f1ef;--positive:#16865c;--negative:#c2414b;--warning:#9a6700;--warning-bg:#fff4ce;--shadow:0 12px 34px rgba(20,33,61,.08)}
@media(prefers-color-scheme:dark){:root{--bg:#0c111b;--panel:#151c29;--text:#eef2f8;--muted:#9aa7b8;--border:#2a3547;--accent:#58c8c3;--accent-soft:#163c3d;--positive:#55c995;--negative:#ff7d87;--warning:#ffd166;--warning-bg:#3c3112;--shadow:0 16px 40px rgba(0,0,0,.22)}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--text);font:14px/1.5 Inter,ui-sans-serif,system-ui,-apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}main{max-width:1420px;margin:auto;padding:36px 24px 64px}header{display:flex;justify-content:space-between;gap:24px;align-items:end;margin-bottom:22px}h1{font-size:30px;line-height:1.15;margin:0 0 6px}h2{font-size:17px;margin:0 0 16px}p{margin:0;color:var(--muted)}.period{text-align:right}.warning,.complete{display:flex;gap:12px;align-items:center;padding:12px 16px;border-radius:10px;margin-bottom:20px}.warning{color:var(--warning);background:var(--warning-bg)}.complete{color:var(--positive);background:var(--accent-soft)}.warning span,.complete span{color:inherit}.metrics{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px;margin-bottom:18px}.metric,.panel{background:var(--panel);border:1px solid var(--border);box-shadow:var(--shadow);border-radius:14px}.metric{padding:16px 18px}.metric span{display:block;color:var(--muted);font-size:12px;margin-bottom:4px}.metric strong{font-size:22px;font-weight:600}.panel{padding:20px;margin-bottom:18px}.two{display:grid;grid-template-columns:1fr 1fr;gap:18px}.chart svg{display:block;width:100%;height:auto;overflow:visible}.line{fill:none;stroke:var(--accent);stroke-width:2.5;vector-effect:non-scaling-stroke}.area{fill:var(--accent);opacity:.09}.gridline{stroke:var(--border);stroke-width:1;vector-effect:non-scaling-stroke}.axis{fill:var(--muted);font-size:12px}.bar-positive{fill:var(--positive);opacity:.82}.bar-negative{fill:var(--negative);opacity:.82}table{width:100%;border-collapse:collapse}th,td{padding:10px 9px;border-bottom:1px solid var(--border);text-align:right}th:first-child,td:first-child{text-align:left}th{font-size:12px;color:var(--muted);font-weight:600}.heatmap{font-variant-numeric:tabular-nums}.heatmap td{text-align:center;border:3px solid var(--panel);border-radius:7px;min-width:56px}.heatmap .positive{background:color-mix(in srgb,var(--positive) calc(var(--heat)*100%),var(--panel));color:var(--text)}.heatmap .negative{background:color-mix(in srgb,var(--negative) calc(var(--heat)*100%),var(--panel));color:var(--text)}.heatmap .missing{color:var(--muted)}.correlation-grid{display:grid;grid-template-columns:1fr;gap:18px}.correlation-panel{overflow-x:auto}.correlation-panel h2{white-space:nowrap}.observations{font-weight:400;color:var(--muted);font-size:12px}.correlation th{max-width:180px;white-space:normal;line-height:1.2}.correlation td{min-width:72px}.detail-table th{width:34%}.empty{padding:42px;text-align:center}.footnote{font-size:12px;color:var(--muted);margin-top:20px}.coverage-value{font-variant-numeric:tabular-nums}@media(max-width:900px){.metrics{grid-template-columns:repeat(2,minmax(0,1fr))}.two{grid-template-columns:1fr}header{align-items:start;flex-direction:column}.period{text-align:left}.heatmap{font-size:11px}.heatmap th,.heatmap td{padding:6px 3px;min-width:0}}@media(max-width:520px){main{padding:22px 12px 42px}.metrics{grid-template-columns:1fr 1fr}.metric strong{font-size:18px}.panel{padding:14px;overflow-x:auto}.warning,.complete{align-items:flex-start;flex-direction:column}}
</style>
</head>
<body><main>
<header><div><h1>Backtest Tear Sheet</h1><p>{{STRATEGY_NAME}} · Portfolio simulation</p></div><div class="period"><p>Generated from immutable Parquet results</p></div></header>
{{COVERAGE_WARNING}}
<section class="metrics">{{CARDS}}</section>
<section class="panel chart"><h2>Equity curve <span class="observations">(log scale)</span></h2>{{EQUITY_CHART}}</section>
<section class="panel chart"><h2>Underwater curve</h2>{{DRAWDOWN_CHART}}</section>
<section class="panel"><h2>Monthly returns</h2>{{MONTHLY_HEATMAP}}</section>
{{CORRELATION_MATRICES}}
<section class="two"><div class="panel chart"><h2>21-session rolling Sharpe</h2>{{ROLLING_CHART}}</div><div class="panel chart"><h2>Trade return distribution</h2>{{TRADE_HISTOGRAM}}</div></section>
<section class="panel"><h2>All, long, and short trade breakdown</h2>{{TRADE_BREAKDOWN}}</section>
<section class="two"><div class="panel"><h2>Risk and trade statistics</h2><table class="detail-table"><tbody>{{METRICS_TABLE}}</tbody></table></div><div class="panel"><h2>Data coverage</h2><table class="detail-table coverage-value"><tbody>{{COVERAGE_TABLE}}</tbody></table></div></section>
<section class="two"><div class="panel"><h2>Run configuration</h2><table class="detail-table"><tbody>{{RUN_DETAILS}}</tbody></table></div><div class="panel"><h2>Most recent trades</h2>{{RECENT_TRADES}}</div></section>
<p class="footnote">Sharpe and Sortino use daily portfolio returns, a zero risk-free/target return, and {{ANNUALIZATION}} observations-per-year annualization. Calmar is CAGR divided by absolute maximum drawdown. Metrics remain coverage-dependent.</p>
</main></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn sample_standard_deviation_is_zero_for_constant_returns() {
        assert!(sample_standard_deviation(&[0.1, 0.1, 0.1]) < 1e-12);
    }

    #[test]
    fn equity_chart_uses_logarithmic_scaling() {
        let chart = line_chart(
            &[100.0, 1_000.0, 10_000.0],
            &["start".to_owned(), "middle".to_owned(), "end".to_owned()],
            ChartKind::LogCurrency,
            "USD",
        );
        assert!(chart.contains("Logarithmic equity time series chart"));
        assert!(chart.contains("$"));
        assert!(!chart.contains("requires positive values"));
    }

    #[test]
    fn non_usd_reports_label_currency_without_a_dollar_sign() {
        assert_eq!(currency(1_000.0, "CNY"), "CNY 1000.00");
        assert_eq!(currency(-15.0, "CNY"), "-CNY 15.00");
    }

    #[test]
    fn coverage_percentage_counts_all_statuses() {
        let coverage = Coverage {
            covered: 80,
            missing_file: 15,
            missing_session: 5,
        };
        assert_eq!(coverage.percent(), 80.0);
    }

    #[test]
    fn strategy_specific_watchlist_does_not_block_shared_report_loading() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "backtester-strategy-watchlist-{}-{suffix}.parquet",
            std::process::id()
        ));
        let mut frame = df!(
            "trade_date" => &["2026-01-02"],
            "symbol" => &["TEST.US"],
            "adx" => &[35.0]
        )
        .expect("frame");
        ParquetWriter::new(File::create(&path).expect("create watchlist"))
            .finish(&mut frame)
            .expect("write watchlist");
        let rows = load_watchlist(&path).expect("optional watchlist should be non-blocking");
        assert!(rows.is_empty());
        fs::remove_file(path).expect("clean test watchlist");
    }

    #[test]
    fn yearly_performance_compounds_return_and_resets_drawdown_each_year() {
        let point = |year, month, day, equity| DailyPoint {
            date: NaiveDate::from_ymd_opt(year, month, day).unwrap(),
            equity,
            fills: 0,
            daily_return: 0.0,
            drawdown: 0.0,
        };
        let daily = vec![
            point(2024, 1, 2, 110.0),
            point(2024, 2, 1, 88.0),
            point(2024, 12, 31, 120.0),
            point(2025, 1, 2, 108.0),
        ];
        let yearly = yearly_performance(&daily, 100.0);
        assert!((yearly[&2024].0 - 20.0).abs() < 1e-12);
        assert!((yearly[&2024].1 + 20.0).abs() < 1e-12);
        assert!((yearly[&2025].0 + 10.0).abs() < 1e-12);
        assert!((yearly[&2025].1 + 10.0).abs() < 1e-12);
    }

    #[test]
    fn trade_breakdown_separates_long_and_short_net_results() {
        let trade = |direction: &str, pnl: f64, return_percent: f64| TradePoint {
            symbol: "TEST.US".to_owned(),
            direction: Some(direction.to_owned()),
            trade_date: "2025-01-02".to_owned(),
            entry_time: "2025-01-02T09:35:00-05:00".to_owned(),
            exit_time: "2025-01-02T12:00:00-05:00".to_owned(),
            pnl,
            return_percent,
            leverage: None,
        };
        let trades = [trade("long", 100.0, 1.0), trade("long", -50.0, -0.5)];
        let references = trades.iter().collect::<Vec<_>>();
        let metrics = calculate_trade_breakdown(&references);
        assert_eq!(metrics.trades, 2);
        assert_eq!(metrics.win_rate, 50.0);
        assert_eq!(metrics.total_pnl, 50.0);
        assert_eq!(metrics.average_pnl, 25.0);
        assert_eq!(metrics.average_return, 0.25);
        assert_eq!(metrics.profit_factor, Some(2.0));
    }
}
