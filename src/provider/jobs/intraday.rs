//! The native intraday increment job (DS-09, BT-1206; decisions 0020 and 0022).
//!
//! A run brings one intraday dataset (5-minute or 1-minute bars, hourly where the provider
//! offers them) up to the instant it is given. [`plan`] reads the folder and says what the
//! run would do: for every symbol with a file, the windows from the file's last timestamp
//! through that instant, cut to the longest span the provider serves in one request at the
//! resolution; for every symbol without one, the windows from the from-date; and the call
//! estimate the budget judges, every request at the intraday cost. [`run`] then, in this
//! order:
//!
//! 1. Refuses without a call when the folder is missing, not a folder, or not writable (the
//!    job never creates it), when the provider offers no intraday bars at the resolution,
//!    or when the mandatory calls (the increments) exceed what the budget has above its
//!    reserve.
//! 2. Extends every file: its windows are fetched oldest first, the first one starting at
//!    the file's last bar so a current file gets that bar back and nothing else, and the
//!    bars after the last timestamp are appended, never one already in the file, through
//!    a part file renamed over the target. A delisted symbol is never incremented. A symbol
//!    the provider answers with no bars in any window, or whose request it answers with
//!    something other than bars (its 404), is skipped with the reason and not asked again
//!    in the run.
//! 3. Backfills the symbols with no file, delisted ones included, from the from-date, a new
//!    file each, stopping before the request that would reach the budget's reserve: the
//!    windows fetched so far make the file (the next run extends it from its last bar), the
//!    symbols left are counted, and the next run, planning from the files again, picks them
//!    up.
//!
//! Files keep the layout the engine reads (`docs/DATA_SOURCES.md`):
//! `Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume`, the timestamp as UTC epoch
//! seconds, the datetime as that instant in UTC, one file per symbol named
//! `<CODE>.<EXCHANGE>.csv`. Progress and the log go to the callbacks the caller passes; the
//! outcome carries the state, the counts, the symbols skipped with their reasons, and the
//! error.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};

use super::eod::{
    DatasetSymbol, JobState, Progress, Skipped, check_folder, ends_the_run, file_name, tail_of,
    write_part_then_rename,
};
use crate::provider::budget::{CallBudget, Estimate, INTRADAY_CALL_COST};
use crate::provider::{IntradayBar, Provider};

/// The intraday file's header (`docs/DATA_SOURCES.md`).
pub const INTRADAY_HEADER: &str = "Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume";

/// What a run needs to know about its dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntradayJobInput {
    /// The exchange code the files are named with (`<CODE>.<EXCHANGE>.csv`).
    pub exchange: String,
    /// The bar resolution in the engine's name (`5m`, `1m`, `1h`).
    pub resolution: String,
    /// The dataset folder. It must exist: the job never creates it.
    pub folder: PathBuf,
    /// Where a backfilled symbol's bars start (midnight UTC of the date).
    pub from_date: NaiveDate,
    /// The instant a run fetches through (now, from the service).
    pub through: DateTime<Utc>,
    /// The dataset's symbols: the listing of its types, delisted ones included when the
    /// dataset includes them.
    pub symbols: Vec<DatasetSymbol>,
}

/// One request's span: the bars whose start lies in `from..=to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// A symbol and the windows a run fetches for it, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolWindows {
    pub code: String,
    pub windows: Vec<Window>,
}

/// What a run would do, read from the folder before any call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The provider's longest span per request at the resolution, in days.
    pub window_days: u32,
    /// The symbols with a file, in listing order, each with the windows from its last bar
    /// through the instant; a file at the instant already has none, and a delisted
    /// symbol's file is never extended.
    pub increments: Vec<SymbolWindows>,
    /// The symbols with no file, in listing order, each with the windows from the
    /// from-date: the backfill.
    pub backfills: Vec<SymbolWindows>,
    /// Files whose last row carries no timestamp: left as they are and skipped.
    pub unreadable: Vec<String>,
    pub estimate: Estimate,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub state: JobState,
    /// The plan's estimate; `None` when the run was refused before one was made.
    pub estimate: Option<Estimate>,
    /// Calls charged to the budget: every request at the intraday cost.
    pub calls: u64,
    /// Requests made.
    pub requests: u64,
    /// Files created by the backfill.
    pub added: u64,
    /// Files appended to.
    pub updated: u64,
    pub skipped: Vec<Skipped>,
    pub error: Option<String>,
    /// Symbols left to backfill when the run stopped at the reserve; zero otherwise.
    pub backfills_left: u64,
}

/// The windows covering `from..=to` in spans of `window_days` (at least one day), oldest
/// first; none when `from` is after `to`.
pub fn windows(from: DateTime<Utc>, to: DateTime<Utc>, window_days: u32) -> Vec<Window> {
    let span = Duration::days(i64::from(window_days.max(1)));
    let second = Duration::seconds(1);
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let end = (start + span - second).min(to);
        out.push(Window {
            from: start,
            to: end,
        });
        start = end + second;
    }
    out
}

/// The timestamp in the first field of the last non-empty line of `text`, if it is one.
fn last_timestamp_in_text(text: &str) -> Option<i64> {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| line.split(',').next())
        .and_then(|field| field.trim().parse::<i64>().ok())
}

/// The last row's timestamp of an intraday file, read from its tail; `None` for a file
/// with no such row (a header alone, or an empty file) or one that cannot be read.
pub fn last_timestamp_of(path: &Path) -> Option<i64> {
    last_timestamp_in_text(&tail_of(path)?)
}

/// The `Datetime` column for `timestamp`: the instant in UTC as `YYYY-MM-DD HH:MM:SS`.
pub fn datetime_text(timestamp: i64) -> String {
    Utc.timestamp_opt(timestamp, 0)
        .single()
        .map(|at| at.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

/// One intraday row as the file carries it.
pub fn csv_line(bar: &IntradayBar) -> String {
    format!(
        "{},{},{},{},{},{},{},{}",
        bar.timestamp,
        bar.gmtoffset,
        datetime_text(bar.timestamp),
        bar.open,
        bar.high,
        bar.low,
        bar.close,
        bar.volume
    )
}

/// `bars` in timestamp order, one per timestamp.
fn normalized(mut bars: Vec<IntradayBar>) -> Vec<IntradayBar> {
    bars.sort_by_key(|bar| bar.timestamp);
    bars.dedup_by_key(|bar| bar.timestamp);
    bars
}

/// Writes a symbol's file whole: the header and every bar, in order, one per timestamp.
fn write_rows(path: &Path, bars: &[IntradayBar]) -> io::Result<()> {
    let mut text = String::with_capacity(64 * (bars.len() + 1));
    text.push_str(INTRADAY_HEADER);
    text.push('\n');
    for bar in bars {
        text.push_str(&csv_line(bar));
        text.push('\n');
    }
    write_part_then_rename(path, text.as_bytes())
}

/// Appends the bars of `bars` after the file's last timestamp, in order, and returns how
/// many; a file with nothing to append is left untouched.
fn append_rows(path: &Path, bars: &[IntradayBar]) -> io::Result<usize> {
    let bytes = fs::read(path)?;
    let mut text = String::from_utf8(bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not UTF-8: {e}", path.display()),
        )
    })?;
    let last = last_timestamp_in_text(&text);
    let new = normalized(
        bars.iter()
            .filter(|bar| last.is_none_or(|last| bar.timestamp > last))
            .cloned()
            .collect(),
    );
    if new.is_empty() {
        return Ok(0);
    }
    if text.trim().is_empty() {
        text.clear();
        text.push_str(INTRADAY_HEADER);
        text.push('\n');
    } else if !text.ends_with('\n') {
        text.push('\n');
    }
    for bar in &new {
        text.push_str(&csv_line(bar));
        text.push('\n');
    }
    write_part_then_rename(path, text.as_bytes())?;
    Ok(new.len())
}

/// Reads the folder and says what a run would do with windows of `window_days`; refuses
/// the folder as [`check_folder`] does.
pub fn plan(input: &IntradayJobInput, window_days: u32) -> Result<Plan, String> {
    check_folder(&input.folder)?;
    let backfill_from = input
        .from_date
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc();
    let mut increments = Vec::new();
    let mut backfills = Vec::new();
    let mut unreadable = Vec::new();
    for symbol in &input.symbols {
        let code = &symbol.listing.code;
        let path = input.folder.join(file_name(code, &input.exchange));
        if path.is_file() {
            if symbol.delisted {
                continue;
            }
            match last_timestamp_of(&path).and_then(|last| Utc.timestamp_opt(last, 0).single()) {
                Some(last) => increments.push(SymbolWindows {
                    code: code.clone(),
                    windows: windows(last, input.through, window_days),
                }),
                None => unreadable.push(code.clone()),
            }
        } else {
            backfills.push(SymbolWindows {
                code: code.clone(),
                windows: windows(backfill_from, input.through, window_days),
            });
        }
    }
    let count = |set: &[SymbolWindows]| set.iter().map(|s| s.windows.len() as u64).sum();
    let estimate = Estimate::intraday_windows(count(&increments), count(&backfills));
    Ok(Plan {
        window_days,
        increments,
        backfills,
        unreadable,
        estimate,
    })
}

/// What fetching a symbol's windows came to.
enum Fetched {
    /// The provider answered with bars, in order, one per timestamp.
    Bars(Vec<IntradayBar>),
    /// The symbol was skipped (the reason is recorded): no bars in any window, or an
    /// answer that was not bars.
    Skipped,
    /// The reserve was reached before window `windows_done` of the symbol's; the bars of
    /// the windows before it.
    AtReserve {
        bars: Vec<IntradayBar>,
        windows_done: usize,
    },
}

struct Runner<'a, P: Provider> {
    provider: &'a P,
    input: &'a IntradayJobInput,
    budget: &'a mut CallBudget,
    log: &'a mut (dyn Write + Send),
    progress: &'a mut (dyn FnMut(Progress) + Send),
    units_done: u64,
    units_total: u64,
    requests: u64,
    calls: u64,
    added: u64,
    updated: u64,
    skipped: Vec<Skipped>,
}

impl<P: Provider> Runner<'_, P> {
    fn log(&mut self, line: &str) {
        let _ = writeln!(
            self.log,
            "{} {line}",
            Utc::now().format("%Y-%m-%dT%H:%M:%SZ")
        );
    }

    fn charge(&mut self) {
        self.budget.charge(INTRADAY_CALL_COST);
        self.calls += INTRADAY_CALL_COST;
        self.requests += 1;
        self.units_done += 1;
        self.report();
    }

    fn report(&mut self) {
        let percent = if self.units_total == 0 {
            100
        } else {
            (self.units_done.min(self.units_total) * 100 / self.units_total) as u8
        };
        (self.progress)(Progress {
            percent,
            calls: self.calls,
            added: self.added,
            updated: self.updated,
            skipped: self.skipped.len() as u64,
        });
    }

    fn symbol(&self, code: &str) -> String {
        format!("{code}.{}", self.input.exchange)
    }

    fn skip(&mut self, symbol: &str, reason: String) {
        self.log(&format!("skipped {symbol}: {reason}"));
        self.skipped.push(Skipped {
            symbol: symbol.to_owned(),
            reason,
        });
    }

    fn outcome(
        self,
        state: JobState,
        estimate: Option<Estimate>,
        error: Option<String>,
        backfills_left: u64,
    ) -> Outcome {
        Outcome {
            state,
            estimate,
            calls: self.calls,
            requests: self.requests,
            added: self.added,
            updated: self.updated,
            skipped: self.skipped,
            error,
            backfills_left,
        }
    }

    /// One symbol's bars over `windows`, oldest first, each request charged; with
    /// `stop_at_reserve`, no request is made once the budget is at its reserve. `Err`
    /// when the run must end.
    async fn fetch(
        &mut self,
        code: &str,
        windows: &[Window],
        what: &str,
        stop_at_reserve: bool,
    ) -> Result<Fetched, String> {
        let symbol = self.symbol(code);
        let mut bars = Vec::new();
        for (index, window) in windows.iter().enumerate() {
            if stop_at_reserve && self.budget.at_reserve() {
                return Ok(Fetched::AtReserve {
                    bars: normalized(bars),
                    windows_done: index,
                });
            }
            let fetched = self
                .provider
                .intraday(&symbol, &self.input.resolution, window.from, window.to)
                .await;
            self.charge();
            match fetched {
                Ok(rows) => {
                    self.log(&format!(
                        "{symbol}: {} bars from {} to {} ({what}, window {} of {})",
                        rows.len(),
                        window.from.format("%Y-%m-%dT%H:%M:%SZ"),
                        window.to.format("%Y-%m-%dT%H:%M:%SZ"),
                        index + 1,
                        windows.len()
                    ));
                    bars.extend(rows);
                }
                Err(error) if ends_the_run(&error) => {
                    return Err(format!("{what} of {symbol}: {error}"));
                }
                Err(error) => {
                    self.skip(&symbol, format!("{what}: {error}"));
                    return Ok(Fetched::Skipped);
                }
            }
        }
        if bars.is_empty() {
            let (Some(first), Some(last)) = (windows.first(), windows.last()) else {
                unreachable!("a symbol with no windows is never fetched")
            };
            self.skip(
                &symbol,
                format!(
                    "{what}: no bars from the provider between {} and {}",
                    first.from.format("%Y-%m-%dT%H:%M:%SZ"),
                    last.to.format("%Y-%m-%dT%H:%M:%SZ")
                ),
            );
            return Ok(Fetched::Skipped);
        }
        Ok(Fetched::Bars(normalized(bars)))
    }
}

/// Runs the job against `provider` with `budget`, logging lines to `log` and reporting
/// progress after each request. The outcome says how it ended; the budget is charged for
/// every request made.
pub async fn run<P: Provider>(
    provider: &P,
    input: &IntradayJobInput,
    budget: &mut CallBudget,
    log: &mut (dyn Write + Send),
    progress: &mut (dyn FnMut(Progress) + Send),
) -> Outcome {
    let mut runner = Runner {
        provider,
        input,
        budget,
        log,
        progress,
        units_done: 0,
        units_total: 0,
        requests: 0,
        calls: 0,
        added: 0,
        updated: 0,
        skipped: Vec::new(),
    };
    let exchange = input.exchange.clone();
    let resolution = input.resolution.clone();

    let Some(window_days) = provider.intraday_window_days(&resolution) else {
        let text = format!("the provider offers no intraday bars at {resolution:?}");
        runner.log(&format!("refused: {text}"));
        return runner.outcome(JobState::Failed, None, Some(text), 0);
    };
    let plan = match plan(input, window_days) {
        Ok(plan) => plan,
        Err(refusal) => {
            runner.log(&format!("refused: {refusal}"));
            return runner.outcome(JobState::Failed, None, Some(refusal), 0);
        }
    };
    let estimate = plan.estimate;
    let increment_windows: u64 = plan.increments.iter().map(|s| s.windows.len() as u64).sum();
    let backfill_windows: u64 = plan.backfills.iter().map(|s| s.windows.len() as u64).sum();
    runner.log(&format!(
        "dataset {} on {exchange} at {resolution}: {} symbols, {} with a file ({} windows to fetch), {} to backfill ({} windows), windows of {} days through {}",
        input.folder.display(),
        input.symbols.len(),
        plan.increments.len() + plan.unreadable.len(),
        increment_windows,
        plan.backfills.len(),
        backfill_windows,
        plan.window_days,
        input.through.format("%Y-%m-%dT%H:%M:%SZ")
    ));
    runner.log(&format!(
        "estimate: {} mandatory calls, {} optional ({} calls a request); budget {} used of {} with {} in reserve",
        estimate.mandatory,
        estimate.optional,
        INTRADAY_CALL_COST,
        runner.budget.used,
        runner.budget.limit,
        runner.budget.reserve
    ));
    if let Err(refused) = runner.budget.can_start(estimate.mandatory) {
        let text = refused.to_string();
        runner.log(&format!("refused: {text}"));
        return runner.outcome(JobState::Failed, Some(estimate), Some(text), 0);
    }
    runner.units_total = increment_windows + backfill_windows;
    runner.report();
    for code in &plan.unreadable {
        let symbol = runner.symbol(code);
        runner.skip(
            &symbol,
            "increment: the file's last row carries no timestamp; left as it is".to_owned(),
        );
    }

    // 2. The increments, from each file's last bar.
    for symbol in &plan.increments {
        if symbol.windows.is_empty() {
            continue;
        }
        let path = input.folder.join(file_name(&symbol.code, &exchange));
        match runner
            .fetch(&symbol.code, &symbol.windows, "increment", false)
            .await
        {
            Ok(Fetched::Bars(bars)) => match append_rows(&path, &bars) {
                Ok(0) => {
                    runner.log(&format!("{} is current; nothing appended", path.display()));
                }
                Ok(appended) => {
                    runner.updated += 1;
                    runner.log(&format!("appended {appended} bars to {}", path.display()));
                }
                Err(error) => {
                    let text = format!("write {}: {error}", path.display());
                    runner.log(&text);
                    return runner.outcome(JobState::Failed, Some(estimate), Some(text), 0);
                }
            },
            Ok(Fetched::Skipped) => {}
            Ok(Fetched::AtReserve { .. }) => {
                unreachable!("the increments never stop at the reserve")
            }
            Err(text) => {
                runner.log(&text);
                return runner.outcome(JobState::Failed, Some(estimate), Some(text), 0);
            }
        }
    }

    // 3. The backfill, up to the reserve.
    let mut backfills_left = 0u64;
    for (index, symbol) in plan.backfills.iter().enumerate() {
        if symbol.windows.is_empty() {
            // The instant is before the from-date: nothing to fetch, and no file is made
            // for nothing.
            continue;
        }
        let left_after = (plan.backfills.len() - index - 1) as u64;
        if runner.budget.at_reserve() {
            backfills_left = left_after + 1;
            runner.log(&format!(
                "stopped at reserve: {backfills_left} symbols left to backfill, from {}",
                runner.symbol(&symbol.code)
            ));
            break;
        }
        let path = input.folder.join(file_name(&symbol.code, &exchange));
        let write = |runner: &mut Runner<'_, P>, bars: &[IntradayBar]| -> Result<(), String> {
            write_rows(&path, bars)
                .map_err(|error| format!("write {}: {error}", path.display()))?;
            runner.added += 1;
            runner.log(&format!(
                "backfilled {} with {} bars from {}",
                path.display(),
                bars.len(),
                input.from_date
            ));
            Ok(())
        };
        match runner
            .fetch(&symbol.code, &symbol.windows, "backfill", true)
            .await
        {
            Ok(Fetched::Bars(bars)) => {
                if let Err(text) = write(&mut runner, &bars) {
                    runner.log(&text);
                    return runner.outcome(JobState::Failed, Some(estimate), Some(text), 0);
                }
            }
            Ok(Fetched::Skipped) => {}
            Ok(Fetched::AtReserve { bars, windows_done }) => {
                let symbol_text = runner.symbol(&symbol.code);
                if bars.is_empty() {
                    backfills_left = left_after + 1;
                    runner.log(&format!(
                        "stopped at reserve: {backfills_left} symbols left to backfill, from {symbol_text}"
                    ));
                } else {
                    if let Err(text) = write(&mut runner, &bars) {
                        runner.log(&text);
                        return runner.outcome(JobState::Failed, Some(estimate), Some(text), 0);
                    }
                    backfills_left = left_after;
                    runner.log(&format!(
                        "stopped at reserve: {symbol_text} holds {windows_done} of {} windows and the next run extends it; {backfills_left} symbols left to backfill",
                        symbol.windows.len()
                    ));
                }
                break;
            }
            Err(text) => {
                runner.log(&text);
                return runner.outcome(JobState::Failed, Some(estimate), Some(text), 0);
            }
        }
    }

    runner.units_done = runner.units_total;
    runner.report();
    runner.log(&format!(
        "complete: {} requests ({} calls), {} files added, {} updated, {} skipped{}",
        runner.requests,
        runner.calls,
        runner.added,
        runner.updated,
        runner.skipped.len(),
        if backfills_left > 0 {
            format!(", stopped at reserve with {backfills_left} left")
        } else {
            String::new()
        }
    ));
    runner.outcome(JobState::Complete, Some(estimate), None, backfills_left)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Listing;
    use crate::provider::jobs::eod::part_path;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn bar(timestamp: i64, close: f64) -> IntradayBar {
        IntradayBar {
            timestamp,
            gmtoffset: 0,
            open: close - 1.0,
            high: close + 1.0,
            low: close - 2.0,
            close,
            volume: 100.0,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tessera-intraday-job-{tag}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn windows_cover_the_span_in_whole_days_and_meet_without_overlap() {
        let from = at("2024-01-02T14:30:00Z");
        let two = windows(from, at("2025-12-02T14:30:00Z"), 600);
        assert_eq!(
            two,
            vec![
                Window {
                    from,
                    to: at("2025-08-24T14:29:59Z")
                },
                Window {
                    from: at("2025-08-24T14:30:00Z"),
                    to: at("2025-12-02T14:30:00Z")
                },
            ]
        );
        let one = windows(from, at("2024-01-02T14:35:00Z"), 120);
        assert_eq!(one.len(), 1);
        assert_eq!((one[0].from, one[0].to), (from, at("2024-01-02T14:35:00Z")));
        // From the instant itself: one window of one second.
        let same = windows(from, from, 5);
        assert_eq!(same, vec![Window { from, to: from }]);
        assert!(windows(from, at("2024-01-02T14:29:59Z"), 600).is_empty());
        // A window of zero days is one day, never an endless loop.
        assert_eq!(windows(from, at("2024-01-04T00:00:00Z"), 0).len(), 2);
    }

    #[test]
    fn rows_print_in_the_intraday_layout_and_the_last_timestamp_is_read_from_the_tail() {
        assert_eq!(
            INTRADAY_HEADER,
            "Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume"
        );
        assert_eq!(
            csv_line(&bar(1_704_205_800, 277.6451)),
            "1704205800,0,2024-01-02 14:30:00,276.6451,278.6451,275.6451,277.6451,100"
        );
        assert_eq!(datetime_text(0), "1970-01-01 00:00:00");
        assert_eq!(
            last_timestamp_in_text(&format!(
                "{INTRADAY_HEADER}\n1,0,x,1,1,1,1,1\n2,0,x,1,1,1,1,1\n\n"
            )),
            Some(2)
        );
        assert_eq!(last_timestamp_in_text(INTRADAY_HEADER), None);
        assert_eq!(last_timestamp_in_text(""), None);

        let dir = scratch("tail");
        let path = dir.join("SPY.US.csv");
        fs::write(
            &path,
            format!("{INTRADAY_HEADER}\n1704205800,0,2024-01-02 14:30:00,1,1,1,1,1\n"),
        )
        .unwrap();
        assert_eq!(last_timestamp_of(&path), Some(1_704_205_800));
        assert_eq!(last_timestamp_of(&dir.join("none.csv")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn appends_skip_timestamps_already_in_the_file_and_leave_no_part_file() {
        let dir = scratch("append");
        let path = dir.join("AAPL.US.csv");
        fs::write(
            &path,
            format!("{INTRADAY_HEADER}\n1000,0,1970-01-01 00:16:40,1,2,0,1,5"),
        )
        .unwrap();
        let appended = append_rows(
            &path,
            &[
                bar(1600, 16.0),
                bar(1000, 10.0),
                bar(1300, 13.0),
                bar(1300, 13.0),
                bar(700, 7.0),
            ],
        )
        .unwrap();
        assert_eq!(appended, 2);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert_eq!(lines[1], "1000,0,1970-01-01 00:16:40,1,2,0,1,5");
        assert!(
            lines[2].starts_with("1300,0,1970-01-01 00:21:40,"),
            "{text}"
        );
        assert!(lines[3].starts_with("1600,"), "{text}");
        assert!(text.ends_with('\n'));
        assert_eq!(append_rows(&path, &[bar(1600, 16.0)]).unwrap(), 0);
        assert!(!part_path(&path).exists());

        // An empty file gets the header before its first row.
        let empty = dir.join("NEW.US.csv");
        fs::write(&empty, "").unwrap();
        assert_eq!(append_rows(&empty, &[bar(5, 1.0)]).unwrap(), 1);
        assert!(
            fs::read_to_string(&empty)
                .unwrap()
                .starts_with(INTRADAY_HEADER)
        );

        write_rows(&path, &[bar(2, 2.0), bar(1, 1.0), bar(2, 2.0)]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.starts_with(INTRADAY_HEADER));
        assert!(!part_path(&path).exists());
        let gone = dir.join("gone").join("X.US.csv");
        assert!(write_rows(&gone, &[bar(1, 1.0)]).is_err());
        assert!(!part_path(&gone).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plan_windows_each_file_from_its_last_bar_and_the_missing_from_the_from_date() {
        let dir = scratch("plan");
        let folder = dir.join("5m");
        fs::create_dir_all(&folder).unwrap();
        let symbol = |code: &str, delisted: bool| DatasetSymbol {
            listing: Listing {
                code: code.into(),
                name: String::new(),
                kind: "ETF".into(),
                currency: "USD".into(),
                country: String::new(),
                venue: String::new(),
            },
            delisted,
        };
        let input = IntradayJobInput {
            exchange: "US".into(),
            resolution: "5m".into(),
            folder: folder.clone(),
            from_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            through: at("2025-12-02T14:30:00Z"),
            symbols: vec![
                symbol("SPY", false),
                symbol("AAPL", false),
                symbol("NVDA", false),
                symbol("YHOO", true),
                symbol("BAD", false),
            ],
        };
        // SPY is 700 days behind (two windows); AAPL is at the instant (one window of one
        // second); YHOO is delisted with a file (never extended); BAD has a header only.
        fs::write(
            folder.join("SPY.US.csv"),
            format!("{INTRADAY_HEADER}\n1704205800,0,2024-01-02 14:30:00,1,1,1,1,1\n"),
        )
        .unwrap();
        fs::write(
            folder.join("AAPL.US.csv"),
            format!("{INTRADAY_HEADER}\n1764685800,0,2025-12-02 14:30:00,1,1,1,1,1\n"),
        )
        .unwrap();
        fs::write(
            folder.join("YHOO.US.csv"),
            format!("{INTRADAY_HEADER}\n1,0,x,1,1,1,1,1\n"),
        )
        .unwrap();
        fs::write(folder.join("BAD.US.csv"), format!("{INTRADAY_HEADER}\n")).unwrap();
        let plan = plan(&input, 600).unwrap();
        assert_eq!(plan.window_days, 600);
        assert_eq!(plan.increments.len(), 2);
        assert_eq!(plan.increments[0].code, "SPY");
        assert_eq!(plan.increments[0].windows.len(), 2);
        assert_eq!(
            plan.increments[0].windows[0].from,
            at("2024-01-02T14:30:00Z")
        );
        assert_eq!(plan.increments[1].code, "AAPL");
        assert_eq!(
            plan.increments[1].windows,
            vec![Window {
                from: input.through,
                to: input.through
            }]
        );
        assert_eq!(plan.backfills.len(), 1);
        assert_eq!(plan.backfills[0].code, "NVDA");
        assert_eq!(plan.backfills[0].windows.len(), 2);
        assert_eq!(
            plan.backfills[0].windows[0].from,
            at("2024-01-01T00:00:00Z")
        );
        assert_eq!(plan.backfills[1..].len(), 0);
        assert_eq!(plan.unreadable, vec!["BAD"]);
        assert_eq!(plan.estimate, Estimate::intraday_windows(3, 2));

        fs::remove_dir_all(&folder).unwrap();
        assert!(super::plan(&input, 600).unwrap_err().contains("missing"));
        assert!(!folder.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
