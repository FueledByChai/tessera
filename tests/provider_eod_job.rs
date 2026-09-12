//! The native EOD download job over a stub server and a temp root (DS-08, BT-1205;
//! decisions 0020 and 0022).
//!
//! The stub is an axum server whose answers the test sets per date and symbol
//! (`eod-bulk-last-day/{exchange}` for bars and, with `type=splits`, for splits;
//! `eod/{symbol}` for a history) and which records every call. The job runs through the real
//! EODHD adapter against it, over a folder of three seeded daily files, and the tests prove
//! each clause of the ticket's done line: two sessions appended to the three files and a
//! fourth listed symbol backfilled from the from-date; a seeded split rewriting that
//! symbol's whole file on the new basis; a bulk day without the calendar symbol (or under
//! the minimum) writing nothing and failing with the row count; a rerun adding and updating
//! zero; a reserve admitting one backfill leaving no part file, with the rerun continuing
//! from the symbol left; a removed folder refused before any call; the catalog files
//! matching the listing. Nothing here reaches eodhd.com or any real data folder.

use std::collections::HashMap;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{NaiveDate, Utc};
use tessera::provider::Listing;
use tessera::provider::budget::{CallBudget, Estimate};
use tessera::provider::eodhd::Eodhd;
use tessera::provider::jobs::eod::{
    self, DAILY_HEADER, DatasetSymbol, EodJobInput, JobState, Outcome, Progress,
};

/// A placeholder, never a real token.
const TOKEN: &str = "stub-token-0000";

fn day(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

/// What the stub answers: bars per date, splits per date, a history per symbol. A date or
/// symbol not set answers as the provider does: an empty list for a bulk day, 404 for a
/// symbol it does not know.
#[derive(Default)]
struct Answers {
    bulk: HashMap<String, Vec<serde_json::Value>>,
    splits: HashMap<String, Vec<serde_json::Value>>,
    history: HashMap<String, Vec<serde_json::Value>>,
}

#[derive(Clone)]
struct Stub {
    answers: Arc<Mutex<Answers>>,
    calls: Arc<Mutex<Vec<String>>>,
    /// While set, every history call answers 503: the provider going away mid-run.
    history_down: Arc<AtomicBool>,
}

fn json(status: StatusCode, body: String) -> Response {
    (status, [("content-type", "application/json")], body).into_response()
}

fn param<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Records the call as `path?k=v&...` without the token and refuses a wrong token.
fn record(stub: &Stub, path: &str, query: &[(String, String)]) -> Option<Response> {
    let shown = query
        .iter()
        .filter(|(k, _)| k != "api_token" && k != "fmt")
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    stub.calls.lock().unwrap().push(format!("{path}?{shown}"));
    if param(query, "api_token") != Some(TOKEN) {
        return Some(json(
            StatusCode::UNAUTHORIZED,
            r#"{"message":"Unauthenticated"}"#.to_owned(),
        ));
    }
    None
}

async fn bulk(
    State(stub): State<Stub>,
    AxumPath(exchange): AxumPath<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    if let Some(refused) = record(&stub, &format!("/api/eod-bulk-last-day/{exchange}"), &query) {
        return refused;
    }
    let date = param(&query, "date").unwrap_or("").to_owned();
    let answers = stub.answers.lock().unwrap();
    let rows = if param(&query, "type") == Some("splits") {
        answers.splits.get(&date)
    } else {
        answers.bulk.get(&date)
    };
    json(
        StatusCode::OK,
        serde_json::to_string(&rows.cloned().unwrap_or_default()).unwrap(),
    )
}

async fn history(
    State(stub): State<Stub>,
    AxumPath(symbol): AxumPath<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    if let Some(refused) = record(&stub, &format!("/api/eod/{symbol}"), &query) {
        return refused;
    }
    if stub.history_down.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response();
    }
    let from = param(&query, "from").unwrap_or("").to_owned();
    let answers = stub.answers.lock().unwrap();
    match answers.history.get(&symbol) {
        Some(rows) => {
            let kept: Vec<&serde_json::Value> = rows
                .iter()
                .filter(|row| row["date"].as_str().unwrap() >= from.as_str())
                .collect();
            json(StatusCode::OK, serde_json::to_string(&kept).unwrap())
        }
        None => json(
            StatusCode::NOT_FOUND,
            r#"{"message":"Symbol not found"}"#.to_owned(),
        ),
    }
}

async fn serve(stub: Stub) -> String {
    let router = Router::new()
        .route("/api/eod-bulk-last-day/{exchange}", get(bulk))
        .route("/api/eod/{symbol}", get(history))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

fn bulk_row(code: &str, date: &str, close: f64) -> serde_json::Value {
    serde_json::json!({
        "code": code, "exchange_short_name": "US", "date": date,
        "open": close - 1.0, "high": close + 1.0, "low": close - 2.0, "close": close,
        "adjusted_close": close, "volume": 1000
    })
}

fn history_row(date: &str, close: f64, adjusted: f64) -> serde_json::Value {
    serde_json::json!({
        "date": date, "open": close - 1.0, "high": close + 1.0, "low": close - 2.0,
        "close": close, "adjusted_close": adjusted, "volume": 500
    })
}

fn listing(code: &str, name: &str, kind: &str) -> Listing {
    Listing {
        code: code.into(),
        name: name.into(),
        kind: kind.into(),
        currency: "USD".into(),
        country: "USA".into(),
        venue: if kind == "ETF" { "NYSE ARCA" } else { "NASDAQ" }.into(),
    }
}

/// The seeded file of a symbol: the header and one row per date, closes rising.
fn seeded(dates: &[&str]) -> String {
    let mut text = format!("{DAILY_HEADER}\n");
    for (i, date) in dates.iter().enumerate() {
        let close = 100.0 + i as f64;
        text.push_str(&format!(
            "{date},{},{},{},{close},{close},1000\n",
            close - 1.0,
            close + 1.0,
            close - 2.0
        ));
    }
    text
}

const SEED_DATES: [&str; 3] = ["2026-09-07", "2026-09-08", "2026-09-09"];

/// A temp root with `eod/` holding AAPL, SPY, and MSFT through 2026-09-09, a listing of
/// those plus NVDA (no file), a stub, and the adapter over it.
struct Scenario {
    root: PathBuf,
    stub: Stub,
    provider: Eodhd,
    symbols: Vec<DatasetSymbol>,
    log: String,
    progress: Vec<Progress>,
}

impl Scenario {
    async fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tessera-eod-job-{tag}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let folder = root.join("eod");
        fs::create_dir_all(&folder).unwrap();
        for code in ["AAPL", "SPY", "MSFT"] {
            fs::write(folder.join(format!("{code}.US.csv")), seeded(&SEED_DATES)).unwrap();
        }
        let stub = Stub {
            answers: Arc::new(Mutex::new(Answers::default())),
            calls: Arc::new(Mutex::new(Vec::new())),
            history_down: Arc::new(AtomicBool::new(false)),
        };
        let base = serve(stub.clone()).await;
        let symbols = [
            ("AAPL", "Apple Inc", "Common Stock"),
            ("SPY", "SPDR S&P 500 ETF Trust", "ETF"),
            ("MSFT", "Microsoft Corp", "Common Stock"),
            ("NVDA", "NVIDIA Corp", "Common Stock"),
        ]
        .into_iter()
        .map(|(code, name, kind)| DatasetSymbol {
            listing: listing(code, name, kind),
            delisted: false,
        })
        .collect();
        Scenario {
            root,
            stub,
            provider: Eodhd::new(&base, TOKEN),
            symbols,
            log: String::new(),
            progress: Vec::new(),
        }
    }

    fn folder(&self) -> PathBuf {
        self.root.join("eod")
    }

    fn catalog_dir(&self) -> PathBuf {
        self.root.join("catalog")
    }

    fn file(&self, code: &str) -> PathBuf {
        self.folder().join(format!("{code}.US.csv"))
    }

    fn input(&self, through: &str) -> EodJobInput {
        EodJobInput {
            exchange: "US".into(),
            folder: self.folder(),
            catalog_dir: self.catalog_dir(),
            from_date: day("2020-01-01"),
            through: day(through),
            min_bulk_rows: 4,
            calendar_code: Some("SPY".into()),
            symbols: self.symbols.clone(),
            catalog: self.symbols.iter().map(|s| s.listing.clone()).collect(),
        }
    }

    fn answers(&self) -> std::sync::MutexGuard<'_, Answers> {
        self.stub.answers.lock().unwrap()
    }

    /// A bulk day with a row for every symbol given, plus one for ZZZ, which no dataset
    /// lists.
    fn set_bulk(&self, date: &str, codes: &[&str], close: f64) {
        let mut rows: Vec<serde_json::Value> = codes
            .iter()
            .map(|code| bulk_row(code, date, close))
            .collect();
        rows.push(bulk_row("ZZZ", date, 1.0));
        self.answers().bulk.insert(date.to_owned(), rows);
    }

    fn set_history(&self, symbol: &str, rows: Vec<serde_json::Value>) {
        self.answers().history.insert(symbol.to_owned(), rows);
    }

    fn calls(&self) -> Vec<String> {
        self.stub.calls.lock().unwrap().clone()
    }

    fn take_calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.stub.calls.lock().unwrap())
    }

    async fn run(&mut self, through: &str, budget: &mut CallBudget) -> Outcome {
        let input = self.input(through);
        let mut log: Vec<u8> = Vec::new();
        let mut progress = Vec::new();
        let outcome = eod::run(&self.provider, &input, budget, &mut log, &mut |p| {
            progress.push(p)
        })
        .await;
        self.log = String::from_utf8(log).unwrap();
        self.progress = progress;
        outcome
    }

    fn read(&self, code: &str) -> String {
        fs::read_to_string(self.file(code)).unwrap()
    }

    fn no_part_files(&self) {
        for dir in [self.folder(), self.catalog_dir()] {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                assert!(!name.ends_with(".part"), "part file left behind: {name}");
            }
        }
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn plenty() -> CallBudget {
    CallBudget::new(100_000, 1_234, 5_000)
}

fn assert_progress_is_monotonic_and_ends_at_100(progress: &[Progress]) {
    assert!(!progress.is_empty(), "no progress was reported");
    let mut last = 0;
    for p in progress {
        assert!(p.percent >= last, "percent went back: {progress:?}");
        assert!(p.percent <= 100);
        last = p.percent;
    }
    assert_eq!(progress.last().unwrap().percent, 100, "{progress:?}");
}

#[test]
fn the_daily_layout_is_the_documented_one() {
    assert_eq!(
        DAILY_HEADER,
        "Date,Open,High,Low,Close,Adjusted_close,Volume"
    );
    assert!(seeded(&SEED_DATES).starts_with(DAILY_HEADER));
    assert_eq!(seeded(&SEED_DATES).lines().count(), 4);
    assert_eq!(eod::file_name("SPY", "US"), "SPY.US.csv");
}

/// Two sessions after the latest date are appended to the three files, the fourth listed
/// symbol is backfilled from the from-date, the calls are one bulk and one splits call per
/// session plus one history call, every write went through a part file, and the catalog
/// files match the listing. Then the rerun (through a weekend and a holiday Monday) adds
/// and updates zero.
#[tokio::test]
async fn two_sessions_are_appended_a_missing_symbol_is_backfilled_and_a_rerun_writes_nothing() {
    let mut s = Scenario::new("sessions").await;
    s.set_bulk("2026-09-10", &["AAPL", "SPY", "MSFT", "NVDA"], 110.0);
    s.set_bulk("2026-09-11", &["AAPL", "SPY", "MSFT", "NVDA"], 111.0);
    s.set_history(
        "NVDA.US",
        vec![
            history_row("2019-12-31", 9.0, 9.0),
            history_row("2020-01-02", 10.0, 10.0),
            history_row("2020-01-03", 10.5, 10.5),
            history_row("2026-09-11", 111.0, 111.0),
        ],
    );
    let before = s.read("AAPL");

    let mut budget = plenty();
    let outcome = s.run("2026-09-11", &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.sessions, vec![day("2026-09-10"), day("2026-09-11")]);
    assert_eq!(
        (outcome.calls, outcome.added, outcome.updated),
        (5, 1, 3),
        "{outcome:?}"
    );
    assert!(outcome.skipped.is_empty(), "{outcome:?}");
    assert_eq!(outcome.backfills_left, 0);
    assert_eq!(outcome.estimate, Some(Estimate::eod(2, 1)));
    assert_eq!(budget.used, 1_234 + 5, "every call was charged");
    assert_eq!(
        s.calls(),
        [
            "/api/eod-bulk-last-day/US?date=2026-09-10",
            "/api/eod-bulk-last-day/US?type=splits&date=2026-09-10",
            "/api/eod-bulk-last-day/US?date=2026-09-11",
            "/api/eod-bulk-last-day/US?type=splits&date=2026-09-11",
            "/api/eod/NVDA.US?from=2020-01-01&period=d",
        ]
    );

    for code in ["AAPL", "SPY", "MSFT"] {
        let text = s.read(code);
        assert!(text.starts_with(&before), "{code} lost its rows:\n{text}");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 6, "{code}:\n{text}");
        assert_eq!(lines[4], "2026-09-10,109,111,108,110,110,1000");
        assert_eq!(lines[5], "2026-09-11,110,112,109,111,111,1000");
    }
    let nvda = s.read("NVDA");
    assert_eq!(
        nvda,
        format!(
            "{DAILY_HEADER}\n2020-01-02,9,11,8,10,10,500\n2020-01-03,9.5,11.5,8.5,10.5,10.5,500\n2026-09-11,110,112,109,111,111,500\n"
        ),
        "the backfill starts at the from-date"
    );
    assert!(
        !s.folder().join("ZZZ.US.csv").exists(),
        "a row for an unlisted symbol made a file"
    );
    s.no_part_files();
    assert_progress_is_monotonic_and_ends_at_100(&s.progress);
    let last = s.progress.last().unwrap();
    assert_eq!(
        (last.calls, last.added, last.updated, last.skipped),
        (5, 1, 3, 0)
    );
    assert!(s.log.contains("appended 2 rows"), "{}", s.log);
    assert!(s.log.contains("backfilled"), "{}", s.log);
    assert!(s.log.contains("complete: 5 calls"), "{}", s.log);

    // The catalog files match the listing, in today's columns.
    assert_eq!(
        fs::read_to_string(s.catalog_dir().join("catalog.csv")).unwrap(),
        "Code,Name,Country,Exchange,Currency,Type\n\
         AAPL,Apple Inc,USA,NASDAQ,USD,Common Stock\n\
         MSFT,Microsoft Corp,USA,NASDAQ,USD,Common Stock\n\
         NVDA,NVIDIA Corp,USA,NASDAQ,USD,Common Stock\n\
         SPY,SPDR S&P 500 ETF Trust,USA,NYSE ARCA,USD,ETF\n"
    );
    assert_eq!(
        fs::read_to_string(s.catalog_dir().join("stocks.txt")).unwrap(),
        "AAPL.US\nMSFT.US\nNVDA.US\n"
    );
    assert_eq!(
        fs::read_to_string(s.catalog_dir().join("etfs.txt")).unwrap(),
        "SPY.US\n"
    );

    // The rerun: the latest date is now 2026-09-11 (Friday); Monday the 14th has no bars.
    let snapshot: Vec<String> = ["AAPL", "SPY", "MSFT", "NVDA"]
        .iter()
        .map(|code| s.read(code))
        .collect();
    s.take_calls();
    let mut budget = plenty();
    let outcome = s.run("2026-09-14", &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (outcome.calls, outcome.added, outcome.updated),
        (1, 0, 0),
        "{outcome:?}"
    );
    assert!(outcome.sessions.is_empty(), "{outcome:?}");
    assert_eq!(s.calls(), ["/api/eod-bulk-last-day/US?date=2026-09-14"]);
    assert!(s.log.contains("not a session"), "{}", s.log);
    let after: Vec<String> = ["AAPL", "SPY", "MSFT", "NVDA"]
        .iter()
        .map(|code| s.read(code))
        .collect();
    assert_eq!(snapshot, after, "a rerun touched a file");
    s.no_part_files();
    assert_progress_is_monotonic_and_ends_at_100(&s.progress);

    // Through the weekend only: nothing is even asked.
    s.take_calls();
    let outcome = s.run("2026-09-13", &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Complete);
    assert_eq!((outcome.calls, outcome.added, outcome.updated), (0, 0, 0));
    assert!(s.calls().is_empty());
    assert_eq!(s.progress.last().unwrap().percent, 100);
    s.cleanup();
}

/// A split reported for the session rewrites that symbol's whole file with the history the
/// provider gives on the new basis; the other files are appended. A symbol whose history
/// the provider does not know is skipped with the reason, not a failure.
#[tokio::test]
async fn a_seeded_split_rewrites_the_symbols_whole_file_on_the_new_basis() {
    let mut s = Scenario::new("split").await;
    s.set_bulk("2026-09-10", &["AAPL", "SPY", "MSFT"], 110.0);
    s.answers().splits.insert(
        "2026-09-10".to_owned(),
        vec![serde_json::json!({
            "code": "AAPL", "exchange": "US", "date": "2026-09-10", "split": "4.000000/1.000000"
        })],
    );
    // The rebased history: the old closes divided by four, from before the from-date.
    s.set_history(
        "AAPL.US",
        vec![
            history_row("2019-12-31", 20.0, 5.0),
            history_row("2026-09-07", 100.0, 25.0),
            history_row("2026-09-08", 101.0, 25.25),
            history_row("2026-09-09", 102.0, 25.5),
            history_row("2026-09-10", 27.5, 27.5),
        ],
    );

    let outcome = s.run("2026-09-10", &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(outcome.sessions, vec![day("2026-09-10")]);
    // Bulk, splits, the AAPL refetch, and the NVDA backfill the stub knows nothing of.
    assert_eq!(
        (outcome.calls, outcome.added, outcome.updated),
        (4, 0, 3),
        "{outcome:?}"
    );
    assert_eq!(outcome.skipped.len(), 1, "{outcome:?}");
    assert_eq!(outcome.skipped[0].symbol, "NVDA.US");
    assert!(
        outcome.skipped[0].reason.contains("backfill") && outcome.skipped[0].reason.contains("404"),
        "{outcome:?}"
    );
    assert!(
        s.calls()
            .contains(&"/api/eod/AAPL.US?from=2020-01-01&period=d".to_owned())
    );
    assert_eq!(
        s.read("AAPL"),
        format!(
            "{DAILY_HEADER}\n\
             2026-09-07,99,101,98,100,25,500\n\
             2026-09-08,100,102,99,101,25.25,500\n\
             2026-09-09,101,103,100,102,25.5,500\n\
             2026-09-10,26.5,28.5,25.5,27.5,27.5,500\n"
        ),
        "the whole file is the refetched history"
    );
    let msft = s.read("MSFT");
    assert_eq!(msft.lines().count(), 5);
    assert!(
        msft.ends_with("2026-09-10,109,111,108,110,110,1000\n"),
        "{msft}"
    );
    assert!(!s.file("NVDA").exists());
    s.no_part_files();
    assert!(s.log.contains("split 4.000000/1.000000"), "{}", s.log);
    assert!(s.log.contains("replaced"), "{}", s.log);
    s.cleanup();
}

/// A bulk day with rows but without the calendar symbol, or under the dataset's minimum,
/// writes nothing and fails with the row count; the splits call for it is never made.
#[tokio::test]
async fn a_bulk_day_without_the_calendar_symbol_or_under_the_minimum_writes_nothing() {
    let mut s = Scenario::new("refused-day").await;
    let before: Vec<String> = ["AAPL", "SPY", "MSFT"]
        .iter()
        .map(|code| s.read(code))
        .collect();
    s.set_history("NVDA.US", vec![history_row("2020-01-02", 10.0, 10.0)]);

    // Four rows (AAPL, MSFT, NVDA, ZZZ) reach the minimum, but SPY is not among them.
    s.set_bulk("2026-09-10", &["AAPL", "MSFT", "NVDA"], 110.0);
    let mut budget = plenty();
    let outcome = s.run("2026-09-10", &mut budget).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    let error = outcome.error.clone().unwrap();
    assert!(
        error.contains("4 rows")
            && error.contains("no SPY.US")
            && error.contains("nothing written"),
        "{error}"
    );
    assert_eq!((outcome.calls, outcome.added, outcome.updated), (1, 0, 0));
    assert_eq!(budget.used, 1_234 + 1);
    assert_eq!(s.calls(), ["/api/eod-bulk-last-day/US?date=2026-09-10"]);
    let after: Vec<String> = ["AAPL", "SPY", "MSFT"]
        .iter()
        .map(|code| s.read(code))
        .collect();
    assert_eq!(before, after, "a refused day wrote a row");
    assert!(!s.file("NVDA").exists(), "the backfill ran after a refusal");
    assert!(
        !s.catalog_dir().exists(),
        "the catalog was written after a refusal"
    );
    s.no_part_files();
    assert!(s.log.contains("nothing written"), "{}", s.log);

    // With SPY but only three rows against a minimum of four, the day is short.
    s.take_calls();
    s.answers().bulk.insert(
        "2026-09-10".to_owned(),
        vec![
            bulk_row("AAPL", "2026-09-10", 110.0),
            bulk_row("SPY", "2026-09-10", 110.0),
            bulk_row("MSFT", "2026-09-10", 110.0),
        ],
    );
    let outcome = s.run("2026-09-10", &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(
        error.contains("3 rows") && error.contains("minimum of 4"),
        "{error}"
    );
    assert_eq!(outcome.calls, 1);
    assert_eq!(s.calls().len(), 1);
    let after: Vec<String> = ["AAPL", "SPY", "MSFT"]
        .iter()
        .map(|code| s.read(code))
        .collect();
    assert_eq!(before, after);

    // A second good session after a bad first one is not written either: the run fails
    // before any write.
    s.take_calls();
    s.set_bulk("2026-09-11", &["AAPL", "SPY", "MSFT", "NVDA"], 111.0);
    let outcome = s.run("2026-09-11", &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Failed);
    assert_eq!(outcome.calls, 1, "{:?}", s.calls());
    let after: Vec<String> = ["AAPL", "SPY", "MSFT"]
        .iter()
        .map(|code| s.read(code))
        .collect();
    assert_eq!(before, after);
    s.cleanup();
}

/// A budget that admits only one backfill makes that one call, stops at the reserve with
/// the count left and no part file, and the rerun continues from the symbol that was left.
/// A budget that cannot cover the mandatory calls refuses the run before any call.
#[tokio::test]
async fn the_reserve_stops_the_backfill_cleanly_and_the_rerun_continues_from_the_missing_symbol() {
    let mut s = Scenario::new("reserve").await;
    s.symbols.push(DatasetSymbol {
        listing: listing("TSLA", "Tesla Inc", "Common Stock"),
        delisted: false,
    });
    s.set_history("NVDA.US", vec![history_row("2020-01-02", 10.0, 10.0)]);
    s.set_history("TSLA.US", vec![history_row("2020-01-02", 20.0, 20.0)]);

    // The files are current through the 9th; NVDA and TSLA are missing. One call is
    // available above the reserve.
    let mut budget = CallBudget::new(10, 4, 5);
    assert_eq!(budget.available(), 1);
    let outcome = s.run("2026-09-09", &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!((outcome.calls, outcome.added, outcome.updated), (1, 1, 0));
    assert_eq!(outcome.backfills_left, 1, "{outcome:?}");
    assert_eq!(outcome.estimate, Some(Estimate::eod(0, 2)));
    assert!(budget.at_reserve());
    assert_eq!(s.calls(), ["/api/eod/NVDA.US?from=2020-01-01&period=d"]);
    assert!(s.file("NVDA").is_file());
    assert!(!s.file("TSLA").exists());
    s.no_part_files();
    assert!(
        s.log
            .contains("stopped at reserve: 1 symbols left to backfill, from TSLA.US"),
        "{}",
        s.log
    );
    assert!(s.catalog_dir().join("catalog.csv").is_file());

    // The rerun, with a fresh day's budget, picks up at TSLA and nothing else.
    s.take_calls();
    let mut budget = CallBudget::new(10, 0, 5);
    let outcome = s.run("2026-09-09", &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!((outcome.calls, outcome.added, outcome.updated), (1, 1, 0));
    assert_eq!(outcome.backfills_left, 0);
    assert_eq!(s.calls(), ["/api/eod/TSLA.US?from=2020-01-01&period=d"]);
    assert_eq!(
        s.read("TSLA"),
        format!("{DAILY_HEADER}\n2020-01-02,19,21,18,20,20,500\n")
    );
    s.no_part_files();

    // Two sessions need four mandatory calls; three available is a refusal with both
    // numbers and no call.
    s.take_calls();
    s.set_bulk("2026-09-10", &["AAPL", "SPY", "MSFT", "NVDA"], 110.0);
    s.set_bulk("2026-09-11", &["AAPL", "SPY", "MSFT", "NVDA"], 111.0);
    let mut budget = CallBudget::new(10, 2, 5);
    let outcome = s.run("2026-09-11", &mut budget).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    assert_eq!(
        outcome.error.as_deref(),
        Some("needs 4 calls, 3 available above the reserve of 5")
    );
    assert_eq!(outcome.estimate, Some(Estimate::eod(2, 0)));
    assert_eq!(outcome.calls, 0);
    assert!(s.calls().is_empty(), "{:?}", s.calls());
    assert_eq!(budget.used, 2);
    s.cleanup();
}

/// A dataset folder that has been removed (or made read-only) is refused before any call,
/// and is not created.
#[tokio::test]
async fn a_removed_folder_is_refused_with_no_call_made() {
    let mut s = Scenario::new("folder").await;
    s.set_bulk("2026-09-10", &["AAPL", "SPY", "MSFT", "NVDA"], 110.0);
    fs::remove_dir_all(s.folder()).unwrap();

    let mut budget = plenty();
    let outcome = s.run("2026-09-10", &mut budget).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(
        error.contains("missing") && error.contains("never creates"),
        "{error}"
    );
    assert_eq!(outcome.calls, 0);
    assert_eq!(outcome.estimate, None);
    assert!(s.calls().is_empty(), "a call was made: {:?}", s.calls());
    assert_eq!(budget.used, 1_234);
    assert!(!s.folder().exists(), "the folder was created");
    assert!(!s.catalog_dir().exists());
    assert!(s.log.contains("refused"), "{}", s.log);
    assert_eq!(s.progress, Vec::<Progress>::new());

    // Read-only: refused the same way, unless the process is root and may write anyway.
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(s.folder()).unwrap();
    fs::set_permissions(s.folder(), fs::Permissions::from_mode(0o555)).unwrap();
    let outcome = s.run("2026-09-10", &mut plenty()).await;
    fs::set_permissions(s.folder(), fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: geteuid takes nothing and reads nothing but the process's own id.
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
        assert!(outcome.error.unwrap().contains("not writable"));
        assert!(s.calls().is_empty(), "{:?}", s.calls());
    }
    s.cleanup();
}

/// A wrong token fails the job on its first call before any write, the token in no message;
/// a provider that goes away mid-run fails the job on the call that failed and keeps every
/// file written before it whole.
#[tokio::test]
async fn a_provider_failure_mid_run_fails_the_job_and_keeps_the_files_whole() {
    let mut s = Scenario::new("outage").await;
    s.set_bulk("2026-09-10", &["AAPL", "SPY", "MSFT", "NVDA"], 110.0);
    s.set_history("NVDA.US", vec![history_row("2020-01-02", 10.0, 10.0)]);
    let before = s.read("AAPL");
    let wrong = Eodhd::new(s.provider.base_url(), "wrong-token-9999");
    let input = s.input("2026-09-10");
    let mut log: Vec<u8> = Vec::new();
    let outcome = eod::run(&wrong, &input, &mut plenty(), &mut log, &mut |_| {}).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(
        error.starts_with("bulk 2026-09-10: credentials rejected"),
        "{error}"
    );
    assert!(!error.contains("wrong-token"), "{error}");
    assert_eq!(outcome.calls, 1);
    assert_eq!(s.read("AAPL"), before);
    assert!(!String::from_utf8(log).unwrap().contains("wrong-token"));

    // The provider answers 503 from the backfill on: the three appends before it stand,
    // the job is Failed on that call, and no part file is left.
    s.take_calls();
    s.stub.history_down.store(true, Ordering::SeqCst);
    let outcome = s.run("2026-09-10", &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}\n{}", s.log);
    let error = outcome.error.unwrap();
    assert!(
        error.starts_with("backfill of NVDA.US: provider unreachable"),
        "{error}"
    );
    assert_eq!((outcome.calls, outcome.added, outcome.updated), (3, 0, 3));
    assert_eq!(outcome.sessions, vec![day("2026-09-10")]);
    let aapl = s.read("AAPL");
    assert!(
        aapl.starts_with(&before) && aapl.lines().count() == 5,
        "{aapl}"
    );
    assert!(!s.file("NVDA").exists());
    s.no_part_files();

    // Back up, the rerun has only the backfill left to do.
    s.stub.history_down.store(false, Ordering::SeqCst);
    s.take_calls();
    let outcome = s.run("2026-09-10", &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!((outcome.calls, outcome.added, outcome.updated), (1, 1, 0));
    assert_eq!(s.calls(), ["/api/eod/NVDA.US?from=2020-01-01&period=d"]);
    s.cleanup();
}
