//! The native EOD download job (DS-08, BT-1205; decisions 0020 and 0022).
//!
//! A run brings one daily dataset up to date. [`plan`] reads the folder and says what the
//! run would do: the sessions after the dataset's latest date (the calendar symbol's last
//! date, or the latest last date across the files when the calendar symbol is not on this
//! exchange), the symbols with no file (the backfill), and the call estimate the budget
//! judges. [`run`] then, in this order:
//!
//! 1. Refuses without a call when the folder is missing, not a folder, or not writable (the
//!    job never creates it), or when the mandatory calls exceed what the budget has above
//!    its reserve.
//! 2. Fetches every session's bulk bars and splits before writing anything. A session the
//!    provider has no bars for is not a session (a holiday) and is skipped; one with bars
//!    but without the calendar symbol, or under the dataset's minimum row count, fails the
//!    job whole with the row count and nothing is written.
//! 3. Writes the files: a symbol that split has its whole history refetched and its file
//!    replaced, so its adjusted closes are on one basis; every other symbol with a file has
//!    the new rows appended, never a date already in the file, and a delisted symbol is
//!    never incremented. Every write is a part file renamed over the target.
//! 4. Backfills the symbols with no file, one history call each from the from-date, delisted
//!    ones included, stopping at the budget's reserve with the count left, which the next
//!    run picks up because it plans from the files again.
//! 5. Regenerates `catalog.csv`, `stocks.txt`, and `etfs.txt` in the catalog folder from
//!    the exchange's listing, in the columns the instrument index and the run form read.
//!
//! Progress and the log go to the callbacks the caller passes; the outcome carries the
//! state, the counts, the symbols skipped with their reasons, and the error.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};

use crate::provider::budget::{CallBudget, Estimate};
use crate::provider::{Bar, Listing, Provider, ProviderError};

/// The daily file's header (`docs/DATA_SOURCES.md`).
pub const DAILY_HEADER: &str = "Date,Open,High,Low,Close,Adjusted_close,Volume";

/// The row count a bulk day must reach unless the dataset says otherwise: a US session
/// carries tens of thousands of rows, and a short answer is a partial publish.
pub const DEFAULT_MIN_BULK_ROWS: u64 = 10_000;

/// What a file is called while it is being written.
pub const PART_SUFFIX: &str = ".part";

/// The catalog files a run regenerates.
pub const CATALOG_FILE: &str = "catalog.csv";
pub const STOCKS_FILE: &str = "stocks.txt";
pub const ETFS_FILE: &str = "etfs.txt";
const CATALOG_HEADER: [&str; 6] = ["Code", "Name", "Country", "Exchange", "Currency", "Type"];

/// The listing type whose codes make `stocks.txt`, and the one whose codes make `etfs.txt`.
const STOCK_KIND: &str = "Common Stock";
const ETF_KIND: &str = "ETF";

/// A symbol the dataset covers: its listing and whether the provider lists it as delisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetSymbol {
    pub listing: Listing,
    pub delisted: bool,
}

/// What a run needs to know about its dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EodJobInput {
    /// The exchange code the files are named with (`<CODE>.<EXCHANGE>.csv`).
    pub exchange: String,
    /// The dataset folder. It must exist: the job never creates it.
    pub folder: PathBuf,
    /// Where `catalog.csv`, `stocks.txt`, and `etfs.txt` are regenerated.
    pub catalog_dir: PathBuf,
    /// Where a backfilled symbol's history starts.
    pub from_date: NaiveDate,
    /// The last session a run fetches (today where the exchange is).
    pub through: NaiveDate,
    /// A bulk day with fewer rows is refused.
    pub min_bulk_rows: u64,
    /// The calendar symbol's code when it is listed on this exchange: a bulk day without it
    /// is refused, and its file's last date is the dataset's latest date.
    pub calendar_code: Option<String>,
    /// The dataset's symbols: the listing of its types, delisted ones included when the
    /// dataset includes them.
    pub symbols: Vec<DatasetSymbol>,
    /// The exchange's active listing, every type, for the catalog files.
    pub catalog: Vec<Listing>,
}

/// A job record's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Queued,
    Running,
    Complete,
    Failed,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "Queued",
            JobState::Running => "Running",
            JobState::Complete => "Complete",
            JobState::Failed => "Failed",
        }
    }
}

/// A symbol a run did not bring current, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub symbol: String,
    pub reason: String,
}

/// What a run has done so far, reported after each unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Units done over units planned, 100 when nothing was planned.
    pub percent: u8,
    pub calls: u64,
    /// Files created by the backfill.
    pub added: u64,
    /// Files appended to or replaced.
    pub updated: u64,
    pub skipped: u64,
}

/// What a run would do, read from the folder before any call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The dataset's latest date; `None` with no dated file.
    pub latest_date: Option<NaiveDate>,
    /// The weekdays after the latest date through `through`, oldest first; empty without a
    /// latest date (a dataset with no files is all backfill).
    pub sessions: Vec<NaiveDate>,
    /// The dataset's codes with a file, in listing order.
    pub present: Vec<String>,
    /// The dataset's codes without one, in listing order: the backfill.
    pub missing: Vec<String>,
    pub estimate: Estimate,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub state: JobState,
    /// The plan's estimate; `None` when the folder was refused before one was made.
    pub estimate: Option<Estimate>,
    pub calls: u64,
    pub added: u64,
    pub updated: u64,
    pub skipped: Vec<Skipped>,
    pub error: Option<String>,
    /// The sessions whose rows were appended.
    pub sessions: Vec<NaiveDate>,
    /// Symbols left to backfill when the run stopped at the reserve; zero otherwise.
    pub backfills_left: u64,
}

/// The file a symbol's bars live in.
pub fn file_name(code: &str, exchange: &str) -> String {
    format!("{code}.{exchange}.csv")
}

/// Refuses a folder that is missing, not a folder, or not writable by this process; the
/// text says which. The job never creates the folder (decision 0022).
pub fn check_folder(folder: &Path) -> Result<(), String> {
    if !folder.exists() {
        return Err(format!(
            "dataset folder {} is missing; the job never creates it",
            folder.display()
        ));
    }
    if !folder.is_dir() {
        return Err(format!("{} is not a folder", folder.display()));
    }
    if !writable(folder) {
        return Err(format!(
            "dataset folder {} is not writable",
            folder.display()
        ));
    }
    Ok(())
}

/// Whether `access(2)` grants this process write permission on `path`.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: access reads a NUL-terminated path and touches nothing else.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// The weekdays after `latest` through `through`, oldest first. Weekends are never
/// sessions; a holiday among them is found out when the provider has no bars for it.
pub fn sessions_after(latest: NaiveDate, through: NaiveDate) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut day = latest;
    while let Some(next) = day.succ_opt() {
        if next > through {
            break;
        }
        if !matches!(next.weekday(), Weekday::Sat | Weekday::Sun) {
            out.push(next);
        }
        day = next;
    }
    out
}

/// The date in the first field of the last non-empty line of `text`, if it is one.
fn last_date_in_text(text: &str) -> Option<NaiveDate> {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| line.split(',').next())
        .and_then(|field| NaiveDate::parse_from_str(field.trim(), "%Y-%m-%d").ok())
}

/// The last row's date of a daily file, read from its tail; `None` for a file with no
/// dated row or one that cannot be read.
pub fn last_date_of(path: &Path) -> Option<NaiveDate> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let tail = len.min(4096);
    file.seek(SeekFrom::Start(len - tail)).ok()?;
    let mut buf = vec![0u8; tail as usize];
    file.read_exact(&mut buf).ok()?;
    last_date_in_text(&String::from_utf8_lossy(&buf))
}

/// One daily row as the file carries it.
pub fn csv_line(bar: &Bar) -> String {
    format!(
        "{},{},{},{},{},{},{}",
        bar.date, bar.open, bar.high, bar.low, bar.close, bar.adjusted_close, bar.volume
    )
}

fn part_path(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(PART_SUFFIX);
    PathBuf::from(os)
}

/// Writes `content` to the part file beside `path` and renames it over `path`; on any
/// failure the part file is removed and the target is as it was.
fn write_part_then_rename(path: &Path, content: &[u8]) -> io::Result<()> {
    let part = part_path(path);
    let written = (|| {
        let mut file = fs::File::create(&part)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&part, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&part);
    }
    written
}

/// Writes a symbol's whole history: the header and every bar, oldest first.
fn write_history(path: &Path, bars: &[Bar]) -> io::Result<()> {
    let mut text = String::with_capacity(64 * (bars.len() + 1));
    text.push_str(DAILY_HEADER);
    text.push('\n');
    for bar in bars {
        text.push_str(&csv_line(bar));
        text.push('\n');
    }
    write_part_then_rename(path, text.as_bytes())
}

/// Appends the bars of `bars` dated after the file's last row, in date order, and returns
/// how many; a file with nothing to append is left untouched.
fn append_bars(path: &Path, bars: &[Bar]) -> io::Result<usize> {
    let bytes = fs::read(path)?;
    let mut text = String::from_utf8(bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not UTF-8: {e}", path.display()),
        )
    })?;
    let last = last_date_in_text(&text);
    let mut new: Vec<&Bar> = bars
        .iter()
        .filter(|bar| last.is_none_or(|last| bar.date > last))
        .collect();
    new.sort_by_key(|bar| bar.date);
    new.dedup_by_key(|bar| bar.date);
    if new.is_empty() {
        return Ok(0);
    }
    if text.trim().is_empty() {
        text.clear();
        text.push_str(DAILY_HEADER);
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

/// The three catalog files' contents from the exchange's listing: `catalog.csv` in the
/// columns `Code,Name,Country,Exchange,Currency,Type`, and the universe lists as one
/// `CODE.EXCHANGE` per line, common stocks in `stocks.txt` and ETFs in `etfs.txt`, all
/// sorted by code.
pub fn catalog_files(exchange: &str, listing: &[Listing]) -> io::Result<[(String, Vec<u8>); 3]> {
    let mut rows: Vec<&Listing> = listing.iter().collect();
    rows.sort_by(|a, b| a.code.cmp(&b.code));
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(CATALOG_HEADER)?;
    for row in &rows {
        writer.write_record([
            row.code.as_str(),
            row.name.as_str(),
            row.country.as_str(),
            row.venue.as_str(),
            row.currency.as_str(),
            row.kind.as_str(),
        ])?;
    }
    let catalog = writer
        .into_inner()
        .map_err(|e| io::Error::other(e.to_string()))?;
    let list = |kind: &str| -> Vec<u8> {
        let mut text = String::new();
        for row in rows.iter().filter(|row| row.kind == kind) {
            text.push_str(&row.code);
            text.push('.');
            text.push_str(exchange);
            text.push('\n');
        }
        text.into_bytes()
    };
    Ok([
        (CATALOG_FILE.to_owned(), catalog),
        (STOCKS_FILE.to_owned(), list(STOCK_KIND)),
        (ETFS_FILE.to_owned(), list(ETF_KIND)),
    ])
}

/// Writes the catalog files into `catalog_dir` (created when missing: it is the source's
/// own folder, not a data folder), each through a part file.
pub fn write_catalog(catalog_dir: &Path, exchange: &str, listing: &[Listing]) -> io::Result<()> {
    fs::create_dir_all(catalog_dir)?;
    for (name, content) in catalog_files(exchange, listing)? {
        write_part_then_rename(&catalog_dir.join(name), &content)?;
    }
    Ok(())
}

/// Reads the folder and says what a run would do; refuses the folder as [`check_folder`]
/// does.
pub fn plan(input: &EodJobInput) -> Result<Plan, String> {
    check_folder(&input.folder)?;
    let mut present = Vec::new();
    let mut missing = Vec::new();
    let mut latest_across_files: Option<NaiveDate> = None;
    let mut calendar_date: Option<NaiveDate> = None;
    for symbol in &input.symbols {
        let code = &symbol.listing.code;
        let path = input.folder.join(file_name(code, &input.exchange));
        if path.is_file() {
            present.push(code.clone());
            if let Some(date) = last_date_of(&path) {
                if input.calendar_code.as_deref() == Some(code.as_str()) {
                    calendar_date = Some(date);
                }
                if latest_across_files.is_none_or(|latest| date > latest) {
                    latest_across_files = Some(date);
                }
            }
        } else {
            missing.push(code.clone());
        }
    }
    let latest_date = calendar_date.or(latest_across_files);
    let sessions = latest_date
        .map(|latest| sessions_after(latest, input.through))
        .unwrap_or_default();
    let estimate = Estimate::eod(sessions.len() as u64, missing.len() as u64);
    Ok(Plan {
        latest_date,
        sessions,
        present,
        missing,
        estimate,
    })
}

/// Whether a provider error ends the run (the provider is gone, or the token is) or only
/// the symbol it was for (the provider answered, but not with that symbol's history).
fn ends_the_run(error: &ProviderError) -> bool {
    !matches!(error, ProviderError::Malformed(_))
}

struct Runner<'a, P: Provider> {
    provider: &'a P,
    input: &'a EodJobInput,
    budget: &'a mut CallBudget,
    log: &'a mut (dyn Write + Send),
    progress: &'a mut (dyn FnMut(Progress) + Send),
    units_done: u64,
    units_total: u64,
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
        self.budget.charge(1);
        self.calls += 1;
    }

    fn unit_done(&mut self) {
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

    fn outcome(
        self,
        state: JobState,
        estimate: Option<Estimate>,
        error: Option<String>,
        sessions: Vec<NaiveDate>,
        backfills_left: u64,
    ) -> Outcome {
        Outcome {
            state,
            estimate,
            calls: self.calls,
            added: self.added,
            updated: self.updated,
            skipped: self.skipped,
            error,
            sessions,
            backfills_left,
        }
    }

    /// One symbol's history from the from-date, charged; `Ok(None)` when the provider
    /// answered but not with a history (the symbol is skipped with the reason recorded),
    /// `Err` when the run must end.
    async fn history(&mut self, code: &str, what: &str) -> Result<Option<Vec<Bar>>, String> {
        let symbol = self.symbol(code);
        let fetched = self
            .provider
            .eod_history(&symbol, self.input.from_date)
            .await;
        self.charge();
        match fetched {
            Ok(bars) if bars.is_empty() => {
                self.skip(&symbol, format!("{what}: no history from the provider"));
                Ok(None)
            }
            Ok(bars) => Ok(Some(bars)),
            Err(error) if ends_the_run(&error) => Err(format!("{what} of {symbol}: {error}")),
            Err(error) => {
                self.skip(&symbol, format!("{what}: {error}"));
                Ok(None)
            }
        }
    }

    fn skip(&mut self, symbol: &str, reason: String) {
        self.log(&format!("skipped {symbol}: {reason}"));
        self.skipped.push(Skipped {
            symbol: symbol.to_owned(),
            reason,
        });
    }
}

/// Runs the job against `provider` with `budget`, logging lines to `log` and reporting
/// progress after each unit of work. The outcome says how it ended; the budget is charged
/// for every call made.
pub async fn run<P: Provider>(
    provider: &P,
    input: &EodJobInput,
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
        calls: 0,
        added: 0,
        updated: 0,
        skipped: Vec::new(),
    };
    let exchange = input.exchange.clone();

    let plan = match plan(input) {
        Ok(plan) => plan,
        Err(refusal) => {
            runner.log(&format!("refused: {refusal}"));
            return runner.outcome(JobState::Failed, None, Some(refusal), Vec::new(), 0);
        }
    };
    let estimate = plan.estimate;
    runner.log(&format!(
        "dataset {} on {exchange}: {} symbols, {} with a file, {} to backfill, latest date {}, {} sessions through {} ({})",
        input.folder.display(),
        input.symbols.len(),
        plan.present.len(),
        plan.missing.len(),
        plan.latest_date
            .map(|d| d.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        plan.sessions.len(),
        input.through,
        plan.sessions
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    runner.log(&format!(
        "estimate: {} mandatory calls, {} optional; budget {} used of {} with {} in reserve",
        estimate.mandatory,
        estimate.optional,
        runner.budget.used,
        runner.budget.limit,
        runner.budget.reserve
    ));
    if let Err(refused) = runner.budget.can_start(estimate.mandatory) {
        let text = refused.to_string();
        runner.log(&format!("refused: {text}"));
        return runner.outcome(JobState::Failed, Some(estimate), Some(text), Vec::new(), 0);
    }
    runner.units_total =
        plan.sessions.len() as u64 * 2 + plan.present.len() as u64 + plan.missing.len() as u64;
    runner.report();

    let delisted: HashMap<&str, bool> = input
        .symbols
        .iter()
        .map(|symbol| (symbol.listing.code.as_str(), symbol.delisted))
        .collect();
    let present: BTreeSet<&str> = plan.present.iter().map(String::as_str).collect();

    // 2. Every session's bars and splits, before any write.
    let mut new_rows: BTreeMap<String, Vec<Bar>> = BTreeMap::new();
    let mut split_codes: BTreeSet<String> = BTreeSet::new();
    let mut sessions = Vec::new();
    for date in &plan.sessions {
        let fetched = provider.bulk_eod(&exchange, *date).await;
        runner.charge();
        runner.unit_done();
        let rows = match fetched {
            Ok(rows) => rows,
            Err(error) => {
                let text = format!("bulk {date}: {error}");
                runner.log(&text);
                return runner.outcome(JobState::Failed, Some(estimate), Some(text), sessions, 0);
            }
        };
        let count = rows.len() as u64;
        if rows.is_empty() {
            runner.log(&format!("{date}: no bars from the provider; not a session"));
            runner.unit_done();
            continue;
        }
        if count < input.min_bulk_rows {
            let text = format!(
                "bulk {date} has {count} rows, under the dataset's minimum of {}; nothing written",
                input.min_bulk_rows
            );
            runner.log(&text);
            return runner.outcome(JobState::Failed, Some(estimate), Some(text), sessions, 0);
        }
        if let Some(calendar) = input.calendar_code.as_deref()
            && !rows.iter().any(|row| row.code == calendar)
        {
            let text = format!(
                "bulk {date} has {count} rows but no {calendar}.{exchange}; nothing written"
            );
            runner.log(&text);
            return runner.outcome(JobState::Failed, Some(estimate), Some(text), sessions, 0);
        }
        let mut kept = 0u64;
        for row in rows {
            let Some(&is_delisted) = delisted.get(row.code.as_str()) else {
                continue;
            };
            if is_delisted || !present.contains(row.code.as_str()) {
                continue;
            }
            new_rows.entry(row.code).or_default().push(row.bar);
            kept += 1;
        }
        runner.log(&format!(
            "{date}: {count} rows from the provider, {kept} for files of this dataset"
        ));

        let fetched = provider.splits(&exchange, *date).await;
        runner.charge();
        runner.unit_done();
        match fetched {
            Ok(splits) => {
                for split in splits {
                    if present.contains(split.code.as_str())
                        && delisted.get(split.code.as_str()) == Some(&false)
                    {
                        runner.log(&format!(
                            "{date}: {}.{exchange} split {}; its history will be refetched",
                            split.code, split.ratio
                        ));
                        split_codes.insert(split.code);
                    }
                }
            }
            Err(error) => {
                let text = format!("splits {date}: {error}");
                runner.log(&text);
                return runner.outcome(JobState::Failed, Some(estimate), Some(text), sessions, 0);
            }
        }
        sessions.push(*date);
    }

    // 3. The files: replaced after a split, appended otherwise.
    for code in &plan.present {
        let path = input.folder.join(file_name(code, &exchange));
        if split_codes.contains(code) {
            match runner.history(code, "split refetch").await {
                Ok(Some(bars)) => {
                    if let Err(error) = write_history(&path, &bars) {
                        let text = format!("write {}: {error}", path.display());
                        runner.log(&text);
                        return runner.outcome(
                            JobState::Failed,
                            Some(estimate),
                            Some(text),
                            sessions,
                            0,
                        );
                    }
                    runner.updated += 1;
                    runner.log(&format!(
                        "replaced {} with {} rows on the new basis",
                        path.display(),
                        bars.len()
                    ));
                }
                Ok(None) => {}
                Err(text) => {
                    runner.log(&text);
                    return runner.outcome(
                        JobState::Failed,
                        Some(estimate),
                        Some(text),
                        sessions,
                        0,
                    );
                }
            }
        } else if let Some(bars) = new_rows.get(code) {
            match append_bars(&path, bars) {
                Ok(0) => {}
                Ok(appended) => {
                    runner.updated += 1;
                    runner.log(&format!("appended {appended} rows to {}", path.display()));
                }
                Err(error) => {
                    let text = format!("write {}: {error}", path.display());
                    runner.log(&text);
                    return runner.outcome(
                        JobState::Failed,
                        Some(estimate),
                        Some(text),
                        sessions,
                        0,
                    );
                }
            }
        }
        runner.unit_done();
    }

    // 4. The backfill, up to the reserve.
    let mut backfills_left = 0u64;
    for (index, code) in plan.missing.iter().enumerate() {
        if runner.budget.at_reserve() {
            backfills_left = (plan.missing.len() - index) as u64;
            runner.log(&format!(
                "stopped at reserve: {backfills_left} symbols left to backfill, from {}",
                runner.symbol(code)
            ));
            break;
        }
        let path = input.folder.join(file_name(code, &exchange));
        match runner.history(code, "backfill").await {
            Ok(Some(bars)) => {
                if let Err(error) = write_history(&path, &bars) {
                    let text = format!("write {}: {error}", path.display());
                    runner.log(&text);
                    return runner.outcome(
                        JobState::Failed,
                        Some(estimate),
                        Some(text),
                        sessions,
                        0,
                    );
                }
                runner.added += 1;
                runner.log(&format!(
                    "backfilled {} with {} rows from {}",
                    path.display(),
                    bars.len(),
                    input.from_date
                ));
            }
            Ok(None) => {}
            Err(text) => {
                runner.log(&text);
                return runner.outcome(JobState::Failed, Some(estimate), Some(text), sessions, 0);
            }
        }
        runner.unit_done();
    }

    // 5. The catalog files.
    if let Err(error) = write_catalog(&input.catalog_dir, &exchange, &input.catalog) {
        let text = format!(
            "write the catalog files in {}: {error}",
            input.catalog_dir.display()
        );
        runner.log(&text);
        return runner.outcome(
            JobState::Failed,
            Some(estimate),
            Some(text),
            sessions,
            backfills_left,
        );
    }
    runner.log(&format!(
        "catalog: {} listings written to {}",
        input.catalog.len(),
        input.catalog_dir.display()
    ));
    runner.units_done = runner.units_total;
    runner.report();
    runner.log(&format!(
        "complete: {} calls, {} files added, {} updated, {} skipped{}",
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
    runner.outcome(
        JobState::Complete,
        Some(estimate),
        None,
        sessions,
        backfills_left,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(text: &str) -> NaiveDate {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
    }

    fn bar(date: &str, close: f64) -> Bar {
        Bar {
            date: day(date),
            open: close - 1.0,
            high: close + 1.0,
            low: close - 2.0,
            close,
            adjusted_close: close / 2.0,
            volume: 1000.0,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tessera-eod-job-{tag}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sessions_are_the_weekdays_after_the_latest_date_through_the_given_one() {
        // 2026-09-11 is a Friday.
        assert_eq!(
            sessions_after(day("2026-09-09"), day("2026-09-15")),
            vec![
                day("2026-09-10"),
                day("2026-09-11"),
                day("2026-09-14"),
                day("2026-09-15")
            ]
        );
        assert_eq!(sessions_after(day("2026-09-11"), day("2026-09-13")), vec![]);
        assert_eq!(sessions_after(day("2026-09-11"), day("2026-09-01")), vec![]);
        assert_eq!(
            sessions_after(day("2026-09-11"), day("2026-09-11")),
            Vec::<NaiveDate>::new()
        );
    }

    #[test]
    fn rows_print_in_the_daily_layout_and_the_last_date_is_read_from_the_tail() {
        let line = csv_line(&bar("2026-09-11", 150.5));
        assert_eq!(line, "2026-09-11,149.5,151.5,148.5,150.5,75.25,1000");
        assert_eq!(
            last_date_in_text("Date,Close\n2026-09-10,1\n2026-09-11,2\n\n"),
            Some(day("2026-09-11"))
        );
        assert_eq!(last_date_in_text("Date,Close\n"), None);
        assert_eq!(last_date_in_text(""), None);

        let dir = scratch("tail");
        let path = dir.join("SPY.US.csv");
        fs::write(&path, "Date,Close\n2026-09-10,1\n2026-09-11,2\n").unwrap();
        assert_eq!(last_date_of(&path), Some(day("2026-09-11")));
        assert_eq!(last_date_of(&dir.join("none.csv")), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn appends_skip_dates_already_in_the_file_and_leave_no_part_file() {
        let dir = scratch("append");
        let path = dir.join("AAPL.US.csv");
        fs::write(&path, format!("{DAILY_HEADER}\n2026-09-09,1,2,0,1,1,5")).unwrap();
        let appended = append_bars(
            &path,
            &[
                bar("2026-09-11", 11.0),
                bar("2026-09-09", 9.0),
                bar("2026-09-10", 10.0),
                bar("2026-09-10", 10.0),
            ],
        )
        .unwrap();
        assert_eq!(appended, 2);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[1].starts_with("2026-09-09,1,2,0,1,1,5"));
        assert!(lines[2].starts_with("2026-09-10,"));
        assert!(lines[3].starts_with("2026-09-11,"));
        assert!(text.ends_with('\n'));
        assert_eq!(append_bars(&path, &[bar("2026-09-11", 11.0)]).unwrap(), 0);
        assert!(!part_path(&path).exists());

        // An empty file gets the header before its first row.
        let empty = dir.join("NEW.US.csv");
        fs::write(&empty, "").unwrap();
        assert_eq!(append_bars(&empty, &[bar("2026-09-11", 1.0)]).unwrap(), 1);
        let text = fs::read_to_string(&empty).unwrap();
        assert!(text.starts_with(DAILY_HEADER), "{text}");

        write_history(&path, &[bar("2026-09-01", 1.0), bar("2026-09-02", 2.0)]).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(text.starts_with(DAILY_HEADER));
        assert!(!part_path(&path).exists());
        // A write into a folder that is gone leaves nothing behind.
        let gone = dir.join("gone").join("X.US.csv");
        assert!(write_history(&gone, &[bar("2026-09-01", 1.0)]).is_err());
        assert!(!part_path(&gone).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_folder_is_refused_when_missing_a_file_or_read_only_and_never_created() {
        let dir = scratch("folder");
        let missing = dir.join("eod");
        let refusal = check_folder(&missing).unwrap_err();
        assert!(
            refusal.contains("missing") && refusal.contains("never creates"),
            "{refusal}"
        );
        assert!(!missing.exists(), "the folder was created");
        let file = dir.join("file");
        fs::write(&file, "x").unwrap();
        assert!(check_folder(&file).unwrap_err().contains("not a folder"));
        fs::create_dir_all(&missing).unwrap();
        assert_eq!(check_folder(&missing), Ok(()));
        // A read-only folder is refused, unless the process is root and may write anyway.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&missing, fs::Permissions::from_mode(0o555)).unwrap();
        let outcome = check_folder(&missing);
        fs::set_permissions(&missing, fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: geteuid takes nothing and reads nothing but the process's own id.
        if unsafe { libc::geteuid() } != 0 {
            assert!(outcome.unwrap_err().contains("not writable"));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_catalog_files_carry_todays_columns_and_the_universe_lists_by_type() {
        let listing = |code: &str, kind: &str| Listing {
            code: code.into(),
            name: format!("{code} Inc, Ltd"),
            kind: kind.into(),
            currency: "USD".into(),
            country: "USA".into(),
            venue: "NASDAQ".into(),
        };
        let rows = [
            listing("SPY", "ETF"),
            listing("MSFT", "Common Stock"),
            listing("AAPL", "Common Stock"),
            listing("VFIAX", "Fund"),
        ];
        let [
            (catalog_name, catalog),
            (stocks_name, stocks),
            (etfs_name, etfs),
        ] = catalog_files("US", &rows).unwrap();
        assert_eq!(
            (
                catalog_name.as_str(),
                stocks_name.as_str(),
                etfs_name.as_str()
            ),
            ("catalog.csv", "stocks.txt", "etfs.txt")
        );
        let catalog = String::from_utf8(catalog).unwrap();
        let lines: Vec<&str> = catalog.lines().collect();
        assert_eq!(lines[0], "Code,Name,Country,Exchange,Currency,Type");
        assert_eq!(
            lines[1],
            "AAPL,\"AAPL Inc, Ltd\",USA,NASDAQ,USD,Common Stock"
        );
        assert_eq!(lines.len(), 5);
        assert!(lines[4].starts_with("VFIAX,"));
        assert_eq!(String::from_utf8(stocks).unwrap(), "AAPL.US\nMSFT.US\n");
        assert_eq!(String::from_utf8(etfs).unwrap(), "SPY.US\n");

        let dir = scratch("catalog");
        let catalog_dir = dir.join("catalog");
        write_catalog(&catalog_dir, "US", &rows).unwrap();
        assert!(catalog_dir.join("catalog.csv").is_file());
        assert_eq!(
            fs::read_to_string(catalog_dir.join("etfs.txt")).unwrap(),
            "SPY.US\n"
        );
        assert!(!part_path(&catalog_dir.join("stocks.txt")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plan_takes_the_latest_date_from_the_calendar_file_and_lists_the_missing_symbols() {
        let dir = scratch("plan");
        let folder = dir.join("eod");
        fs::create_dir_all(&folder).unwrap();
        let symbol = |code: &str, delisted: bool| DatasetSymbol {
            listing: Listing {
                code: code.into(),
                name: String::new(),
                kind: STOCK_KIND.into(),
                currency: "USD".into(),
                country: String::new(),
                venue: String::new(),
            },
            delisted,
        };
        let mut input = EodJobInput {
            exchange: "US".into(),
            folder: folder.clone(),
            catalog_dir: dir.join("catalog"),
            from_date: day("2020-01-01"),
            through: day("2026-09-15"),
            min_bulk_rows: 3,
            calendar_code: Some("SPY".into()),
            symbols: vec![
                symbol("AAPL", false),
                symbol("SPY", false),
                symbol("NVDA", false),
                symbol("YHOO", true),
            ],
            catalog: Vec::new(),
        };
        fs::write(
            folder.join("SPY.US.csv"),
            format!("{DAILY_HEADER}\n2026-09-09,1,1,1,1,1,1\n"),
        )
        .unwrap();
        // A file ahead of the calendar does not move the latest date.
        fs::write(
            folder.join("AAPL.US.csv"),
            format!("{DAILY_HEADER}\n2026-09-11,1,1,1,1,1,1\n"),
        )
        .unwrap();
        let plan = plan(&input).unwrap();
        assert_eq!(plan.latest_date, Some(day("2026-09-09")));
        assert_eq!(
            plan.sessions,
            vec![
                day("2026-09-10"),
                day("2026-09-11"),
                day("2026-09-14"),
                day("2026-09-15")
            ]
        );
        assert_eq!(plan.present, vec!["AAPL", "SPY"]);
        assert_eq!(plan.missing, vec!["NVDA", "YHOO"]);
        assert_eq!(plan.estimate, Estimate::eod(4, 2));

        // Without the calendar symbol on this exchange, the files' latest date rules.
        input.calendar_code = None;
        let plan = super::plan(&input).unwrap();
        assert_eq!(plan.latest_date, Some(day("2026-09-11")));
        assert_eq!(plan.sessions, vec![day("2026-09-14"), day("2026-09-15")]);

        // No files at all: nothing to increment, everything to backfill.
        fs::remove_file(folder.join("SPY.US.csv")).unwrap();
        fs::remove_file(folder.join("AAPL.US.csv")).unwrap();
        let plan = super::plan(&input).unwrap();
        assert_eq!(plan.latest_date, None);
        assert!(plan.sessions.is_empty());
        assert_eq!(plan.missing.len(), 4);
        assert_eq!(plan.estimate, Estimate::eod(0, 4));

        fs::remove_dir_all(&folder).unwrap();
        assert!(super::plan(&input).unwrap_err().contains("missing"));
        assert!(!folder.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn job_states_name_themselves_and_only_a_malformed_answer_spares_the_run() {
        assert_eq!(JobState::Queued.as_str(), "Queued");
        assert_eq!(JobState::Running.as_str(), "Running");
        assert_eq!(JobState::Complete.as_str(), "Complete");
        assert_eq!(JobState::Failed.as_str(), "Failed");
        assert!(!ends_the_run(&ProviderError::Malformed("HTTP 404".into())));
        assert!(ends_the_run(&ProviderError::Unreachable("503".into())));
        assert!(ends_the_run(&ProviderError::CredentialsRejected(
            "no".into()
        )));
        assert_eq!(file_name("BRK-B", "US"), "BRK-B.US.csv");
    }
}
