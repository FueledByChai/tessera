use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use polars::prelude::*;
use serde::{Deserialize, Serialize};

pub const STANDARD_ARTIFACT_FILES: [&str; 4] = [
    "run_config.toml",
    "daily_equity.parquet",
    "trades.parquet",
    "coverage.parquet",
];

/// Metadata interpreted by the shared report generator. Strategy-specific
/// configuration remains in its immutable `strategy.toml` snapshot.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StandardRunMetadata {
    pub strategy_name: String,
    pub resolution: String,
    pub initial_capital: f64,
    pub currency: String,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub symbols: Vec<String>,
    pub parameters: BTreeMap<String, String>,
    #[serde(default = "default_annualization_periods")]
    pub annualization_periods: f64,
}

fn default_annualization_periods() -> f64 {
    252.0
}

#[derive(Debug, Clone)]
pub struct StandardDailyRecord {
    pub date: NaiveDate,
    pub ending_equity: f64,
    pub fills: usize,
}

#[derive(Debug, Clone)]
pub struct StandardTradeRecord {
    pub symbol: String,
    pub direction: String,
    pub trade_date: NaiveDate,
    pub entry_time: String,
    pub exit_time: String,
    pub pnl: f64,
    pub return_percent: f64,
    pub leverage: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct StandardCoverageRecord {
    pub trade_date: NaiveDate,
    pub symbol: String,
    pub status: String,
}

pub struct StandardArtifactBundle<'a> {
    pub metadata: &'a StandardRunMetadata,
    pub daily: &'a [StandardDailyRecord],
    pub trades: &'a [StandardTradeRecord],
    pub coverage: &'a [StandardCoverageRecord],
}

/// Write the one stable artifact schema consumed by reports, portfolios, and
/// the UI. Strategies only map their domain results into these normalized rows.
pub fn write_standard_artifacts(
    output_dir: &Path,
    bundle: &StandardArtifactBundle<'_>,
) -> Result<()> {
    anyhow::ensure!(
        !bundle.daily.is_empty(),
        "standard daily results cannot be empty"
    );
    anyhow::ensure!(
        bundle.metadata.start <= bundle.metadata.end,
        "standard report start must not be after end"
    );
    anyhow::ensure!(
        bundle.metadata.initial_capital.is_finite() && bundle.metadata.initial_capital > 0.0,
        "standard report initial capital must be positive"
    );
    anyhow::ensure!(
        bundle.metadata.annualization_periods.is_finite()
            && bundle.metadata.annualization_periods > 0.0,
        "annualization periods must be positive"
    );
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    fs::write(
        output_dir.join("run_config.toml"),
        toml::to_string_pretty(bundle.metadata)?,
    )?;

    let mut daily = df!(
        "date" => bundle.daily.iter().map(|row| row.date.to_string()).collect::<Vec<_>>(),
        "ending_equity" => bundle.daily.iter().map(|row| row.ending_equity).collect::<Vec<_>>(),
        "fills" => bundle.daily.iter().map(|row| row.fills as u64).collect::<Vec<_>>()
    )?;
    write_parquet(&mut daily, &output_dir.join("daily_equity.parquet"))?;

    let mut trades = df!(
        "symbol" => bundle.trades.iter().map(|row| row.symbol.as_str()).collect::<Vec<_>>(),
        "direction" => bundle.trades.iter().map(|row| row.direction.as_str()).collect::<Vec<_>>(),
        "trade_date" => bundle.trades.iter().map(|row| row.trade_date.to_string()).collect::<Vec<_>>(),
        "entry_time" => bundle.trades.iter().map(|row| row.entry_time.as_str()).collect::<Vec<_>>(),
        "exit_time" => bundle.trades.iter().map(|row| row.exit_time.as_str()).collect::<Vec<_>>(),
        "pnl" => bundle.trades.iter().map(|row| row.pnl).collect::<Vec<_>>(),
        "return_percent" => bundle.trades.iter().map(|row| row.return_percent).collect::<Vec<_>>(),
        "leverage" => bundle.trades.iter().map(|row| row.leverage).collect::<Vec<_>>()
    )?;
    write_parquet(&mut trades, &output_dir.join("trades.parquet"))?;

    let mut coverage = df!(
        "trade_date" => bundle.coverage.iter().map(|row| row.trade_date.to_string()).collect::<Vec<_>>(),
        "symbol" => bundle.coverage.iter().map(|row| row.symbol.as_str()).collect::<Vec<_>>(),
        "status" => bundle.coverage.iter().map(|row| row.status.as_str()).collect::<Vec<_>>()
    )?;
    write_parquet(&mut coverage, &output_dir.join("coverage.parquet"))?;
    validate_standard_artifacts(output_dir)
}

pub fn validate_standard_artifacts(output_dir: &Path) -> Result<()> {
    for file in STANDARD_ARTIFACT_FILES {
        anyhow::ensure!(
            output_dir.join(file).is_file(),
            "standard result bundle is missing {file}"
        );
    }
    crate::report::load_report_view(output_dir)
        .with_context(|| format!("invalid standard result bundle in {}", output_dir.display()))?;
    Ok(())
}

fn write_parquet(frame: &mut DataFrame, path: &Path) -> Result<()> {
    let file =
        File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    ParquetWriter::new(file)
        .with_compression(ParquetCompression::Zstd(None))
        .finish(frame)?;
    Ok(())
}

/// Stable boundary between a strategy and the reusable backtest application.
///
/// Implementations own their configuration, signals, and execution rules. They
/// normalize their results through [`write_standard_artifacts`]. Serialization,
/// metrics, charts, tables, and report presentation belong to the engine.
pub trait ConfiguredStrategy {
    type Config;
    type Summary;

    const ID: &'static str;

    fn load_config(path: &Path) -> Result<Self::Config>;

    fn run(
        config: &Self::Config,
        start: NaiveDate,
        end: NaiveDate,
        output_dir: &Path,
    ) -> Result<Self::Summary>;
}

pub fn run_configured_strategy<S: ConfiguredStrategy>(
    config_path: &Path,
    start: NaiveDate,
    end: NaiveDate,
    output_dir: &Path,
) -> Result<S::Summary> {
    let config = S::load_config(config_path)?;
    S::run(&config, start, end, output_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn shared_artifact_writer_produces_a_complete_report_contract() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let output = std::env::temp_dir().join(format!(
            "tessera-standard-artifacts-{}-{suffix}",
            std::process::id()
        ));
        let start = NaiveDate::from_ymd_opt(2026, 1, 1).expect("date");
        let end = NaiveDate::from_ymd_opt(2026, 1, 2).expect("date");
        let metadata = StandardRunMetadata {
            strategy_name: "Contract Test".to_owned(),
            resolution: "Daily UTC bars".to_owned(),
            initial_capital: 100_000.0,
            currency: "USD".to_owned(),
            start,
            end,
            symbols: vec!["TEST".to_owned()],
            parameters: BTreeMap::new(),
            annualization_periods: 365.25,
        };
        let daily = vec![
            StandardDailyRecord {
                date: start,
                ending_equity: 100_000.0,
                fills: 0,
            },
            StandardDailyRecord {
                date: end,
                ending_equity: 101_000.0,
                fills: 1,
            },
        ];
        let trades = vec![StandardTradeRecord {
            symbol: "TEST".to_owned(),
            direction: "long".to_owned(),
            trade_date: end,
            entry_time: "2026-01-02T00:00:00+00:00".to_owned(),
            exit_time: "2026-01-02T23:59:59+00:00".to_owned(),
            pnl: 1_000.0,
            return_percent: 1.0,
            leverage: Some(1.0),
        }];
        let coverage = vec![StandardCoverageRecord {
            trade_date: end,
            symbol: "TEST".to_owned(),
            status: "covered".to_owned(),
        }];
        write_standard_artifacts(
            &output,
            &StandardArtifactBundle {
                metadata: &metadata,
                daily: &daily,
                trades: &trades,
                coverage: &coverage,
            },
        )
        .expect("write standard bundle");
        for file in STANDARD_ARTIFACT_FILES {
            assert!(output.join(file).is_file(), "missing {file}");
        }
        let report = crate::report::load_report_view(&output).expect("load shared report");
        assert_eq!(report.strategy_name, "Contract Test");
        assert_eq!(report.daily.len(), 2);
        assert_eq!(report.trades.len(), 1);
        assert!(report.metrics.annual_volatility_percent > 13.4);
        assert!(report.metrics.annual_volatility_percent < 13.6);
        fs::remove_dir_all(output).expect("clean test output");
    }
}
