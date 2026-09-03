//! Machine-local configuration: where market data lives, which engine binary to run, and
//! where private strategy files are discovered. Never committed; see `local.example.toml`.
//!
//! Resolution order for every setting: environment variable, then `local.toml` at the
//! repository root, then the bundled example data so a fresh clone works out of the box.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const LOCAL_FILE: &str = "local.toml";

/// A CSV-folder market-data library. Daily files are `<symbol>.csv` with
/// `Date,Open,High,Low,Close,Adjusted_close,Volume`; intraday files carry
/// `Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume` with UTC epoch timestamps.
/// The catalog directory holds `catalog.csv` (`Code,Name,Country,Exchange,Currency,Type`)
/// plus optional `stocks.txt` / `etfs.txt` universe lists and provider sub-catalogs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataLibrary {
    #[serde(default)]
    pub daily_dir: PathBuf,
    #[serde(default)]
    pub five_minute_dir: PathBuf,
    #[serde(default)]
    pub one_minute_dir: PathBuf,
    #[serde(default)]
    pub catalog_dir: PathBuf,
    /// Daily file whose dates define the trading calendar for screened runs.
    #[serde(default = "default_calendar_symbol")]
    pub calendar_symbol: String,
    /// Optional JSON written by the provider's refresh job, read for freshness display.
    #[serde(default)]
    pub freshness_file: Option<PathBuf>,
    /// Optional shell command the Data workspace may run to refresh the library.
    #[serde(default)]
    pub update_command: Option<String>,
    #[serde(default = "default_provider_name")]
    pub provider: String,
}

fn default_calendar_symbol() -> String {
    "SPY.US".to_owned()
}
fn default_provider_name() -> String {
    "csv-folders".to_owned()
}

impl DataLibrary {
    pub fn stock_universe(&self) -> PathBuf {
        self.catalog_dir.join("stocks.txt")
    }
    pub fn etf_universe(&self) -> PathBuf {
        self.catalog_dir.join("etfs.txt")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Engine binary to run strategies with. Defaults to this repository's release build.
    #[serde(default)]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StrategyDirs {
    /// Extra directories of one-file strategies compiled into the engine, for example a
    /// private repository's `strategies/` folder. Read by `build.rs`.
    #[serde(default)]
    pub dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConfig {
    pub data: DataLibrary,
    #[serde(default)]
    pub engine: EngineConfig,
    #[serde(default)]
    pub strategies: StrategyDirs,
}

impl LocalConfig {
    /// Loads `local.toml` from the repository root, falling back to the bundled example
    /// data. Environment variables `BACKTESTER_DATA_ROOT` (a folder containing `eod/`,
    /// `5m/`, `1m/`, and `catalog/`) and `BACKTESTER_ENGINE` override the file.
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(LOCAL_FILE);
        let mut config = if path.is_file() {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            let parsed: Self = toml::from_str(&text)
                .with_context(|| format!("failed to parse {}", path.display()))?;
            parsed.resolve_relative(root)
        } else {
            Self::bundled_example(root)
        };
        if let Ok(data_root) = env::var("BACKTESTER_DATA_ROOT") {
            let data_root = PathBuf::from(data_root);
            config.data.daily_dir = data_root.join("eod");
            config.data.five_minute_dir = data_root.join("5m");
            config.data.one_minute_dir = data_root.join("1m");
            config.data.catalog_dir = data_root.join("catalog");
        }
        if let Ok(engine) = env::var("BACKTESTER_ENGINE") {
            config.engine.path = Some(PathBuf::from(engine));
        }
        if let Ok(dirs) = env::var("BACKTESTER_STRATEGY_DIRS") {
            config
                .strategies
                .dirs
                .extend(env::split_paths(&dirs).filter(|p| !p.as_os_str().is_empty()));
        }
        config.validate()?;
        Ok(config)
    }

    /// The synthetic dataset shipped under `examples/data`, enough to run the examples.
    pub fn bundled_example(root: &Path) -> Self {
        let data = root.join("examples/data");
        Self {
            data: DataLibrary {
                daily_dir: data.join("eod"),
                five_minute_dir: data.join("5m"),
                one_minute_dir: data.join("1m"),
                catalog_dir: data.join("catalog"),
                calendar_symbol: "DEMO.US".to_owned(),
                freshness_file: None,
                update_command: None,
                provider: "bundled-example".to_owned(),
            },
            engine: EngineConfig::default(),
            strategies: StrategyDirs::default(),
        }
    }

    fn resolve_relative(mut self, root: &Path) -> Self {
        let fix = |path: &mut PathBuf| {
            if path.is_relative() && !path.as_os_str().is_empty() {
                *path = root.join(&*path);
            }
        };
        fix(&mut self.data.daily_dir);
        fix(&mut self.data.five_minute_dir);
        fix(&mut self.data.one_minute_dir);
        fix(&mut self.data.catalog_dir);
        if let Some(path) = self.data.freshness_file.as_mut() {
            fix(path);
        }
        if let Some(path) = self.engine.path.as_mut() {
            fix(path);
        }
        for dir in &mut self.strategies.dirs {
            fix(dir);
        }
        self
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.data.daily_dir.is_dir(),
            "daily data directory does not exist: {} (edit {LOCAL_FILE} or set BACKTESTER_DATA_ROOT)",
            self.data.daily_dir.display()
        );
        Ok(())
    }

    pub fn engine_path(&self, root: &Path) -> PathBuf {
        if let Some(path) = &self.engine.path {
            return path.clone();
        }
        let release = root.join("target/release/backtester");
        if release.is_file() {
            release
        } else {
            root.join("target/debug/backtester")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_example_points_at_examples_data() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let config = LocalConfig::bundled_example(root);
        assert!(config.data.daily_dir.ends_with("examples/data/eod"));
        assert_eq!(config.data.calendar_symbol, "DEMO.US");
    }

    #[test]
    fn relative_paths_resolve_against_root() {
        let root = Path::new("/tmp/backtester-root");
        let config = LocalConfig {
            data: DataLibrary {
                daily_dir: PathBuf::from("data/eod"),
                five_minute_dir: PathBuf::from("/abs/5m"),
                one_minute_dir: PathBuf::new(),
                catalog_dir: PathBuf::from("data/catalog"),
                calendar_symbol: "SPY.US".to_owned(),
                freshness_file: None,
                update_command: None,
                provider: "csv-folders".to_owned(),
            },
            engine: EngineConfig::default(),
            strategies: StrategyDirs::default(),
        }
        .resolve_relative(root);
        assert_eq!(config.data.daily_dir, root.join("data/eod"));
        assert_eq!(config.data.five_minute_dir, PathBuf::from("/abs/5m"));
        assert!(config.data.one_minute_dir.as_os_str().is_empty());
    }
}
