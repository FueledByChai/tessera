use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use tessera::portfolio::combine_portfolio;
use tessera::report::generate_report;
use tessera::sdk;
use tessera::sdk::runner::SdkRunConfig;

#[derive(Debug, Parser)]
#[command(
    name = "tessera",
    version,
    about = "Tessera: fast event-driven market tessera"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate an HTML tear sheet from a simulation result directory.
    Report {
        #[arg(long)]
        results_dir: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Combine completed strategy artifacts into a weighted portfolio.
    Combine {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, default_value = "artifacts/combined_portfolio")]
        output_dir: PathBuf,
    },
    /// Run a one-file SDK strategy (any resolution) from a frozen SDK run config.
    RunStrategy {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Print the manifests of every SDK strategy compiled into this engine as JSON.
    SdkManifests,
    /// Build N-second bars with book features from the parquet lake and print a sample.
    LakeBars {
        #[arg(long)]
        lake: PathBuf,
        /// EXCHANGE:SYMBOL, for example PARADEX:SOL-USD-PERP
        #[arg(long)]
        symbol: String,
        #[arg(long, default_value_t = 1)]
        step_secs: u32,
        #[arg(long)]
        start: NaiveDate,
        #[arg(long)]
        end: NaiveDate,
        #[arg(long, default_value_t = 10)]
        rows: usize,
    },
    /// Feature study on the tick lake: feature vs forward return at several horizons.
    Study {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        start: NaiveDate,
        #[arg(long)]
        end: NaiveDate,
        #[arg(long)]
        output_dir: PathBuf,
    },
    /// Check a lake symbol-day: epochs, delta mix, and whether the rebuilt book brackets trades.
    LakeDiagnose {
        #[arg(long)]
        lake: PathBuf,
        #[arg(long)]
        symbol: String,
        #[arg(long)]
        date: NaiveDate,
    },
    /// Print a parquet file's schema and its first rows (handy for new data lakes).
    ParquetSchema {
        #[arg(long)]
        path: PathBuf,
        #[arg(long, default_value_t = 5)]
        rows: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Report {
            results_dir,
            output,
        } => {
            let summary = generate_report(&results_dir, output.as_deref())?;
            println!("wrote {}", summary.output.display());
            println!("coverage: {:.2}%", summary.coverage_percent);
            println!("sharpe: {}", format_ratio(summary.sharpe));
            println!("sortino: {}", format_ratio(summary.sortino));
            println!("calmar: {}", format_ratio(summary.calmar));
        }
        Command::Combine { config, output_dir } => {
            let summary = combine_portfolio(&config, &output_dir)?;
            println!("wrote {}", output_dir.display());
            println!("components: {}", summary.components);
            println!("shared period: {} to {}", summary.start, summary.end);
            println!("sessions: {}", summary.sessions);
            println!("starting equity: {:.2}", summary.starting_equity);
            println!("ending equity: {:.2}", summary.ending_equity);
            println!("total return: {:.3}%", summary.total_return_percent);
            println!("max drawdown: {:.3}%", summary.max_drawdown_percent);
            for component in &summary.component_summaries {
                println!(
                    "component {}: return {:.3}% CAGR {:.3}% vol {:.3}% Sharpe {} maxDD {:.3}%",
                    component.name,
                    component.total_return_percent,
                    component.cagr_percent,
                    component.annualized_volatility_percent,
                    format_ratio(component.sharpe),
                    component.max_drawdown_percent
                );
            }
            for pair in &summary.pairwise_correlations {
                println!(
                    "correlation {} / {}: {}",
                    pair.first,
                    pair.second,
                    format_ratio(pair.correlation)
                );
            }
        }
        Command::RunStrategy {
            config,
            start,
            end,
            output_dir,
        } => {
            let config = SdkRunConfig::load(&config)?;
            let entry = sdk::find(&config.strategy)?;
            let summary = sdk::runner::run(
                &config,
                &entry,
                parse_date(&start, "start")?,
                parse_date(&end, "end")?,
                &output_dir,
            )?;
            sdk::runner::print_summary(&summary);
        }
        Command::SdkManifests => {
            println!("{}", sdk::manifests_json()?);
        }
        Command::LakeBars {
            lake,
            symbol,
            step_secs,
            start,
            end,
            rows,
        } => {
            let sym = tessera::lake::LakeSymbol::parse(&symbol)
                .context("symbol must look like EXCHANGE:SYMBOL")?;
            let started = std::time::Instant::now();
            let bars = tessera::lake::build_bars(&lake, &sym, step_secs, start, end)?;
            let with_book = bars.iter().filter(|bar| bar.book.is_some()).count();
            println!(
                "{} bars ({step_secs}s), {with_book} with book features, built in {:.1}s",
                bars.len(),
                started.elapsed().as_secs_f64()
            );
            for bar in bars.iter().skip(bars.len() / 2).take(rows) {
                match bar.book {
                    Some(book) => println!(
                        "{} {} o={:.3} h={:.3} l={:.3} c={:.3} v={:.2} | bid={:.3} ask={:.3} spread={:.1}bps obi1={:+.2} obi5={:+.2} obi10={:+.2} micro={:+.2}bps trades={} buy={:.2} sell={:.2}",
                        bar.date,
                        bar.time,
                        bar.open,
                        bar.high,
                        bar.low,
                        bar.close,
                        bar.volume,
                        book.bid,
                        book.ask,
                        book.spread_bps,
                        book.obi_l1,
                        book.obi_l5,
                        book.obi_l10,
                        book.microprice_bps(),
                        book.trade_count,
                        book.buy_volume,
                        book.sell_volume
                    ),
                    None => println!(
                        "{} {} o={:.3} h={:.3} l={:.3} c={:.3} v={:.2} | (no book)",
                        bar.date, bar.time, bar.open, bar.high, bar.low, bar.close, bar.volume
                    ),
                }
            }
        }
        Command::Study {
            config,
            start,
            end,
            output_dir,
        } => {
            let text = std::fs::read_to_string(&config)
                .with_context(|| format!("failed to read {}", config.display()))?;
            let config: tessera::study::StudyConfig = toml::from_str(&text)?;
            let result = tessera::study::run(&config, start, end, &output_dir)?;
            for coverage in &result.symbols {
                println!(
                    "{}: {} bars, {} with book",
                    coverage.symbol, coverage.bars, coverage.bars_with_book
                );
            }
            print!("{}", tessera::study::summary_table(&result));
            println!("wrote {}", output_dir.join("study.json").display());
        }
        Command::LakeDiagnose { lake, symbol, date } => {
            let sym = tessera::lake::LakeSymbol::parse(&symbol)
                .context("symbol must look like EXCHANGE:SYMBOL")?;
            print!("{}", tessera::lake::diagnose_day(&lake, &sym, date)?);
        }
        Command::ParquetSchema { path, rows } => {
            use polars::prelude::*;
            let frame = ParquetReader::new(std::fs::File::open(&path)?).finish()?;
            println!("{} rows x {} columns", frame.height(), frame.width());
            for (name, dtype) in frame.get_column_names().iter().zip(frame.dtypes()) {
                println!("  {name}: {dtype:?}");
            }
            println!("{}", frame.head(Some(rows)));
            let names: Vec<String> = frame
                .get_column_names()
                .iter()
                .map(|name| name.to_string())
                .collect();
            for name in names {
                let Ok(column) = frame.column(&name) else {
                    continue;
                };
                if matches!(column.dtype(), DataType::String | DataType::Binary) {
                    if let Ok(unique) = column.as_materialized_series().unique() {
                        if unique.len() <= 12 {
                            println!("  distinct {name}: {}", unique.head(Some(12)));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn format_ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "N/A".to_owned(), |value| format!("{value:.3}"))
}

fn parse_date(value: &str, name: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .with_context(|| format!("invalid {name} date {value:?}; expected YYYY-MM-DD"))
}
