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
