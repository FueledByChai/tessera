//! Exogenous series for studies: anything observed outside the bar stream (funding rates, open
//! interest, a macro release, a signal file) joined to bars as-of the moment it became
//! available, never its nominal time.
//!
//! A series is declared in `local.toml` under `[[data.series]]` (or inline in a study config)
//! with a name, a CSV or parquet path, a kind, and how availability is known: an
//! `available_at` column, or a fixed publication lag after the nominal time. `level` series
//! carry the last available value forward; `event` series are the value on the first bar
//! that can see it and zero elsewhere. Funding and open interest from the tick lake register
//! themselves for lake instruments (see [`crate::study`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

/// How a series' values relate to bars once available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesKind {
    /// The last available value holds until the next one (a rate, a level, a count).
    #[default]
    Level,
    /// The value lands on the first bar that can see it and is zero on every other bar.
    Event,
}

/// One declared series: `[[data.series]]` in `local.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeriesSpec {
    /// The base name expressions use, e.g. `cpi_surprise | zscore 12`.
    pub name: String,
    /// CSV (`.csv`) or parquet (`.parquet`) file.
    pub path: PathBuf,
    #[serde(default)]
    pub kind: SeriesKind,
    /// Column holding the nominal time: a date, an ISO datetime, or epoch seconds, millis, or
    /// micros. Default `time`.
    #[serde(default = "default_time_column")]
    pub time_column: String,
    /// Column holding the value. Default `value`.
    #[serde(default = "default_value_column")]
    pub value_column: String,
    /// Column naming the instrument a row belongs to (`SPY.US`, `BINANCE_FUTURES:SOLUSDT`).
    /// Absent means the series applies to every symbol.
    #[serde(default)]
    pub symbol_column: Option<String>,
    /// Column holding the moment the row became observable, same formats as the time column.
    #[serde(default)]
    pub available_at_column: Option<String>,
    /// Fixed publication lag after the nominal time, used when there is no availability
    /// column. Zero means available at the nominal time.
    #[serde(default)]
    pub publication_lag_secs: i64,
}

fn default_time_column() -> String {
    "time".to_owned()
}
fn default_value_column() -> String {
    "value".to_owned()
}

/// One row of a series, times in microseconds since the epoch, UTC.
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub nominal_us: i64,
    pub available_us: i64,
    pub symbol: Option<String>,
    pub value: f64,
}

/// A loaded series: observations sorted by the moment they became available.
#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub kind: SeriesKind,
    pub observations: Vec<Observation>,
}

impl Series {
    pub fn new(name: &str, kind: SeriesKind, mut observations: Vec<Observation>) -> Self {
        observations.sort_by_key(|o| (o.available_us, o.nominal_us));
        Self {
            name: name.to_owned(),
            kind,
            observations,
        }
    }

    /// Loads the declared file. Relative paths resolve against `root`.
    pub fn load(spec: &SeriesSpec, root: &Path) -> Result<Self> {
        let path = if spec.path.is_relative() {
            root.join(&spec.path)
        } else {
            spec.path.clone()
        };
        let rows = match path.extension().and_then(|e| e.to_str()) {
            Some("parquet") => read_parquet_rows(&path)?,
            _ => read_csv_rows(&path)?,
        };
        let mut observations = Vec::with_capacity(rows.len());
        for (line, row) in rows.iter().enumerate() {
            let cell = |column: &str| -> Result<&str> {
                row.get(column)
                    .map(String::as_str)
                    .with_context(|| format!("series {}: no column {column:?}", spec.name))
            };
            let nominal_us = match parse_instant(cell(&spec.time_column)?) {
                Some(t) => t,
                None => continue, // header echoes, blank lines
            };
            let Ok(value) = cell(&spec.value_column)?.trim().parse::<f64>() else {
                continue;
            };
            let available_us = match &spec.available_at_column {
                Some(column) => parse_instant(cell(column)?).with_context(|| {
                    format!(
                        "series {}: row {} has no readable {column}",
                        spec.name,
                        line + 2
                    )
                })?,
                None => nominal_us + spec.publication_lag_secs * 1_000_000,
            };
            let symbol = match &spec.symbol_column {
                Some(column) => Some(cell(column)?.trim().to_owned()),
                None => None,
            };
            observations.push(Observation {
                nominal_us,
                available_us,
                symbol,
                value,
            });
        }
        anyhow::ensure!(
            !observations.is_empty(),
            "series {} at {} has no usable rows",
            spec.name,
            path.display()
        );
        Ok(Self::new(&spec.name, spec.kind, observations))
    }

    /// The series as seen from one instrument at each bar close (microseconds, ascending):
    /// `level` carries the last value available at or before the close; `event` is the value
    /// that became available since the previous close (the last one if several) and zero
    /// otherwise. `NaN` before anything is available.
    pub fn align(&self, symbol: &str, bar_close_us: &[i64]) -> Vec<f64> {
        let mut out = Vec::with_capacity(bar_close_us.len());
        let mut next = 0;
        let mut current = f64::NAN;
        let mut seen_any = false;
        for (i, &close) in bar_close_us.iter().enumerate() {
            let mut landed: Option<f64> = None;
            while next < self.observations.len() && self.observations[next].available_us <= close {
                let observation = &self.observations[next];
                next += 1;
                if observation
                    .symbol
                    .as_deref()
                    .is_some_and(|s| !s.eq_ignore_ascii_case(symbol))
                {
                    continue;
                }
                current = observation.value;
                landed = Some(observation.value);
                seen_any = true;
            }
            let _ = i;
            out.push(match self.kind {
                SeriesKind::Level => current,
                SeriesKind::Event => {
                    if !seen_any {
                        f64::NAN
                    } else {
                        landed.unwrap_or(0.0)
                    }
                }
            });
        }
        out
    }
}

/// The instant a text cell denotes, in microseconds UTC: `2024-01-05`, `2024-01-05T14:30:00Z`,
/// `2024-01-05 14:30:00`, or epoch seconds / milliseconds / microseconds by magnitude.
pub fn parse_instant(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(number) = text.parse::<f64>() {
        if !number.is_finite() {
            return None;
        }
        // Seconds until 1e11 (year 5138), milliseconds until 1e14, else microseconds.
        let micros = if number.abs() < 1e11 {
            number * 1e6
        } else if number.abs() < 1e14 {
            number * 1e3
        } else {
            number
        };
        return Some(micros.round() as i64);
    }
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_micros());
    }
    if let Ok(stamp) = DateTime::parse_from_rfc3339(text) {
        return Some(stamp.with_timezone(&Utc).timestamp_micros());
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(text, format) {
            return Some(naive.and_utc().timestamp_micros());
        }
    }
    None
}

fn read_csv_rows(path: &Path) -> Result<Vec<BTreeMap<String, String>>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let headers: Vec<String> = reader
        .headers()?
        .iter()
        .map(|h| h.trim().to_owned())
        .collect();
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record?;
        rows.push(
            headers
                .iter()
                .cloned()
                .zip(record.iter().map(str::to_owned))
                .collect(),
        );
    }
    Ok(rows)
}

fn read_parquet_rows(path: &Path) -> Result<Vec<BTreeMap<String, String>>> {
    use polars::prelude::*;
    let frame = ParquetReader::new(
        std::fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?,
    )
    .finish()
    .with_context(|| format!("failed to read {}", path.display()))?;
    let names: Vec<String> = frame
        .get_column_names()
        .iter()
        .map(|n| n.to_string())
        .collect();
    let mut columns: Vec<Vec<String>> = Vec::with_capacity(names.len());
    for name in &names {
        let series = frame.column(name)?.as_materialized_series().clone();
        let text = match series.dtype() {
            DataType::Datetime(unit, _) => {
                let cast = series.cast(&DataType::Int64)?;
                let values = cast.i64()?;
                let scale = match unit {
                    TimeUnit::Nanoseconds => 1_000,
                    TimeUnit::Microseconds => 1,
                    TimeUnit::Milliseconds => 0, // marker: multiply instead
                };
                (0..values.len())
                    .map(|i| match values.get(i) {
                        Some(v) if scale == 0 => (v * 1_000).to_string(),
                        Some(v) => (v / scale).to_string(),
                        None => String::new(),
                    })
                    .collect()
            }
            DataType::Date => {
                let cast = series.cast(&DataType::Int32)?;
                let values = cast.i32()?;
                (0..values.len())
                    .map(|i| match values.get(i) {
                        Some(days) => (i64::from(days) * 86_400 * 1_000_000).to_string(),
                        None => String::new(),
                    })
                    .collect()
            }
            _ => {
                let cast = series.cast(&DataType::String)?;
                let values = cast.str()?;
                (0..values.len())
                    .map(|i| values.get(i).unwrap_or_default().to_owned())
                    .collect()
            }
        };
        columns.push(text);
    }
    Ok((0..frame.height())
        .map(|row| {
            names
                .iter()
                .cloned()
                .zip(columns.iter().map(|c| c[row].clone()))
                .collect()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day_close_us(date: &str) -> i64 {
        // A daily bar's close: 16:00 New York, 21:00 UTC in January.
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap();
        date.and_hms_opt(21, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_micros()
    }

    /// The done line: a value nominally for the 3rd that is published on the 5th is invisible
    /// on the 3rd and the 4th and visible from the 5th, whether availability comes from a
    /// column or from a fixed lag.
    #[test]
    fn availability_gates_what_a_bar_can_see() {
        let dir = std::env::temp_dir().join(format!("tessera-wb05-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("macro.csv"),
            "time,available_at,value\n2024-01-03,2024-01-05T00:00:00Z,1.5\n2024-01-08,2024-01-09T13:30:00Z,2.5\n",
        )
        .unwrap();
        let spec = SeriesSpec {
            name: "macro".to_owned(),
            path: dir.join("macro.csv"),
            kind: SeriesKind::Level,
            time_column: "time".to_owned(),
            value_column: "value".to_owned(),
            symbol_column: None,
            available_at_column: Some("available_at".to_owned()),
            publication_lag_secs: 0,
        };
        let series = Series::load(&spec, &dir).unwrap();
        assert_eq!(series.observations.len(), 2);
        let closes: Vec<i64> = [
            "2024-01-02",
            "2024-01-03",
            "2024-01-04",
            "2024-01-05",
            "2024-01-08",
            "2024-01-09",
        ]
        .iter()
        .map(|d| day_close_us(d))
        .collect();
        let seen = series.align("SPY.US", &closes);
        assert!(
            seen[0].is_nan() && seen[1].is_nan() && seen[2].is_nan(),
            "nothing published yet: {seen:?}"
        );
        assert_eq!(
            seen[3], 1.5,
            "published on the 5th, seen at the 5th's close"
        );
        assert_eq!(seen[4], 1.5, "a level carries forward");
        assert_eq!(
            seen[5], 2.5,
            "the 9th's 13:30 release is in by the 9th's close"
        );

        // The same series as events: the value lands once, zero elsewhere.
        let events = Series::new("macro", SeriesKind::Event, series.observations.clone());
        let seen = events.align("SPY.US", &closes);
        assert!(seen[2].is_nan() && seen[3] == 1.5 && seen[4] == 0.0 && seen[5] == 2.5);

        // A fixed two-day lag instead of a column gives the same visibility.
        std::fs::write(dir.join("lagged.csv"), "time,value\n2024-01-03,1.5\n").unwrap();
        let lagged = Series::load(
            &SeriesSpec {
                name: "lagged".to_owned(),
                path: dir.join("lagged.csv"),
                available_at_column: None,
                publication_lag_secs: 2 * 86_400,
                ..spec.clone()
            },
            &dir,
        )
        .unwrap();
        let seen = lagged.align("SPY.US", &closes);
        assert!(seen[2].is_nan() && seen[3] == 1.5);
        // Without a lag the nominal date is the availability, which is the mistake this
        // module exists to prevent: the value would show on the 3rd.
        let naive = Series::load(
            &SeriesSpec {
                name: "naive".to_owned(),
                path: dir.join("lagged.csv"),
                available_at_column: None,
                publication_lag_secs: 0,
                ..spec.clone()
            },
            &dir,
        )
        .unwrap();
        assert_eq!(naive.align("SPY.US", &closes)[1], 1.5);

        // A symbol column scopes rows to their instrument.
        std::fs::write(
            dir.join("per_symbol.csv"),
            "time,symbol,value\n2024-01-02,SPY.US,10\n2024-01-02,QQQ.US,20\n",
        )
        .unwrap();
        let scoped = Series::load(
            &SeriesSpec {
                name: "scoped".to_owned(),
                path: dir.join("per_symbol.csv"),
                symbol_column: Some("symbol".to_owned()),
                available_at_column: None,
                ..spec.clone()
            },
            &dir,
        )
        .unwrap();
        assert_eq!(scoped.align("QQQ.US", &closes)[0], 20.0);
        assert_eq!(scoped.align("spy.us", &closes)[0], 10.0);
        assert!(scoped.align("IWM.US", &closes)[5].is_nan());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instants_parse_from_dates_datetimes_and_epochs() {
        let day = NaiveDate::from_ymd_opt(2024, 1, 5)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        assert_eq!(parse_instant("2024-01-05"), Some(day.timestamp_micros()));
        assert_eq!(
            parse_instant("2024-01-05T00:00:00Z"),
            Some(day.timestamp_micros())
        );
        assert_eq!(
            parse_instant("2024-01-05 00:00:00"),
            Some(day.timestamp_micros())
        );
        assert_eq!(
            parse_instant("2024-01-05T01:00:00+01:00"),
            Some(day.timestamp_micros())
        );
        let secs = day.timestamp();
        assert_eq!(
            parse_instant(&secs.to_string()),
            Some(day.timestamp_micros())
        );
        assert_eq!(
            parse_instant(&(secs * 1_000).to_string()),
            Some(day.timestamp_micros())
        );
        assert_eq!(
            parse_instant(&(secs * 1_000_000).to_string()),
            Some(day.timestamp_micros())
        );
        assert_eq!(parse_instant(""), None);
        assert_eq!(parse_instant("soon"), None);
    }
}
