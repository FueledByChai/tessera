//! The native intraday increment job over a stub server and a temp root (DS-09, BT-1206;
//! decisions 0020 and 0022).
//!
//! The stub is an axum server answering `intraday/{symbol}` with the rows the test sets per
//! symbol, cut to the `from..=to` of each request as the provider does, recording every
//! call (path, interval, from, to). The job runs through the real EODHD adapter against it,
//! over a folder of seeded 5-minute files, and the tests prove each clause of the ticket's
//! done line: a file 700 days behind is extended across two 600-day windows with two
//! requests (ten calls) and a file one bar behind with one, the bar the file already holds
//! never duplicated; a missing symbol is backfilled from the from-date; a symbol the
//! provider has no bars for is skipped with the reason; a rerun at the same instant writes
//! nothing; a reserve stops the backfill cleanly (before the first backfill request, or
//! between a symbol's windows with the file holding what was fetched and no part file) and
//! the rerun continues; a budget short of the increments refuses before any call; a
//! removed folder and a resolution the provider has no bars at are refused with no call;
//! a rejected token or a provider outage fails the job with the files whole. Nothing here
//! reaches eodhd.com or any real data folder.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, NaiveDate, Utc};
use tessera::provider::Listing;
use tessera::provider::budget::{CallBudget, Estimate, INTRADAY_CALL_COST};
use tessera::provider::eodhd::Eodhd;
use tessera::provider::jobs::eod::{DatasetSymbol, JobState, Progress};
use tessera::provider::jobs::intraday::{
    self, INTRADAY_HEADER, IntradayJobInput, Outcome, Window, windows,
};

/// A placeholder, never a real token.
const TOKEN: &str = "stub-token-0000";

/// 2024-01-02 14:30:00 UTC, the first 5-minute bar of the year, and the instant 700 days
/// on that the runs fetch through: two 600-day windows apart.
const T0: i64 = 1_704_205_800;
const THROUGH: i64 = T0 + 700 * 86_400;
const DAY: i64 = 86_400;
/// Midnight UTC of the from-date, 2024-01-01.
const FROM_DATE_EPOCH: i64 = 1_704_067_200;

fn at(epoch: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(epoch, 0).unwrap()
}

/// What the stub answers per symbol: rows, cut to each request's span. A symbol not set
/// answers as the provider does for one it does not know: 404.
#[derive(Default)]
struct Answers {
    rows: HashMap<String, Vec<serde_json::Value>>,
    /// Symbols whose every request answers 503: the provider going away mid-run.
    down: HashSet<String>,
}

#[derive(Clone)]
struct Stub {
    answers: Arc<Mutex<Answers>>,
    calls: Arc<Mutex<Vec<String>>>,
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

async fn intraday_route(
    State(stub): State<Stub>,
    AxumPath(symbol): AxumPath<String>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let shown = ["interval", "from", "to"]
        .iter()
        .filter_map(|name| param(&query, name).map(|v| format!("{name}={v}")))
        .collect::<Vec<_>>()
        .join("&");
    stub.calls
        .lock()
        .unwrap()
        .push(format!("/api/intraday/{symbol}?{shown}"));
    assert_eq!(param(&query, "fmt"), Some("json"));
    if param(&query, "api_token") != Some(TOKEN) {
        return json(
            StatusCode::UNAUTHORIZED,
            r#"{"message":"Unauthenticated"}"#.to_owned(),
        );
    }
    let from: i64 = param(&query, "from").unwrap().parse().unwrap();
    let to: i64 = param(&query, "to").unwrap().parse().unwrap();
    assert!(from <= to, "{symbol}: from {from} after to {to}");
    let answers = stub.answers.lock().unwrap();
    if answers.down.contains(&symbol) {
        return (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response();
    }
    match answers.rows.get(&symbol) {
        Some(rows) => {
            let kept: Vec<&serde_json::Value> = rows
                .iter()
                .filter(|row| {
                    let ts = row["timestamp"].as_i64().unwrap();
                    ts >= from && ts <= to
                })
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
        .route("/api/intraday/{symbol}", get(intraday_route))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

fn row(timestamp: i64, close: f64) -> serde_json::Value {
    serde_json::json!({
        "timestamp": timestamp, "gmtoffset": 0,
        "datetime": at(timestamp).format("%Y-%m-%d %H:%M:%S").to_string(),
        "open": close - 1.0, "high": close + 1.0, "low": close - 2.0, "close": close,
        "volume": 100
    })
}

/// The line the job writes for `row(timestamp, close)`.
fn line(timestamp: i64, close: f64) -> String {
    format!(
        "{timestamp},0,{},{},{},{},{close},100",
        at(timestamp).format("%Y-%m-%d %H:%M:%S"),
        close - 1.0,
        close + 1.0,
        close - 2.0
    )
}

fn listing(code: &str) -> Listing {
    Listing {
        code: code.into(),
        name: format!("{code} Inc"),
        kind: "ETF".into(),
        currency: "USD".into(),
        country: "USA".into(),
        venue: "NYSE ARCA".into(),
    }
}

/// A seeded file: the header and one bar per timestamp, closes rising from 100.
fn seeded(timestamps: &[i64]) -> String {
    let mut text = format!("{INTRADAY_HEADER}\n");
    for (i, ts) in timestamps.iter().enumerate() {
        text.push_str(&line(*ts, 100.0 + i as f64));
        text.push('\n');
    }
    text
}

/// A temp root with `5m/` holding the files the test seeds, a listing, a stub, and the
/// adapter over it.
struct Scenario {
    root: PathBuf,
    stub: Stub,
    provider: Eodhd,
    symbols: Vec<DatasetSymbol>,
    log: String,
    progress: Vec<Progress>,
}

impl Scenario {
    async fn new(tag: &str, codes: &[&str]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "tessera-intraday-job-{tag}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(root.join("5m")).unwrap();
        let stub = Stub {
            answers: Arc::new(Mutex::new(Answers::default())),
            calls: Arc::new(Mutex::new(Vec::new())),
        };
        let base = serve(stub.clone()).await;
        Scenario {
            root,
            stub,
            provider: Eodhd::new(&base, TOKEN),
            symbols: codes
                .iter()
                .map(|code| DatasetSymbol {
                    listing: listing(code),
                    delisted: false,
                })
                .collect(),
            log: String::new(),
            progress: Vec::new(),
        }
    }

    fn folder(&self) -> PathBuf {
        self.root.join("5m")
    }

    fn file(&self, code: &str) -> PathBuf {
        self.folder().join(format!("{code}.US.csv"))
    }

    fn seed(&self, code: &str, timestamps: &[i64]) {
        fs::write(self.file(code), seeded(timestamps)).unwrap();
    }

    fn set_rows(&self, code: &str, rows: Vec<serde_json::Value>) {
        self.stub
            .answers
            .lock()
            .unwrap()
            .rows
            .insert(format!("{code}.US"), rows);
    }

    fn input(&self, through: i64) -> IntradayJobInput {
        IntradayJobInput {
            exchange: "US".into(),
            resolution: "5m".into(),
            folder: self.folder(),
            from_date: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            through: at(through),
            symbols: self.symbols.clone(),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.stub.calls.lock().unwrap().clone()
    }

    fn take_calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.stub.calls.lock().unwrap())
    }

    async fn run(&mut self, through: i64, budget: &mut CallBudget) -> Outcome {
        let input = self.input(through);
        let mut log: Vec<u8> = Vec::new();
        let mut progress = Vec::new();
        let outcome = intraday::run(&self.provider, &input, budget, &mut log, &mut |p| {
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

    fn snapshot(&self) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = fs::read_dir(self.folder())
            .unwrap()
            .flatten()
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let text = fs::read_to_string(entry.path()).unwrap();
                (name, text)
            })
            .collect();
        files.sort();
        files
    }

    fn no_part_files(&self) {
        for entry in fs::read_dir(self.folder()).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(!name.ends_with(".part"), "part file left behind: {name}");
        }
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn plenty() -> CallBudget {
    CallBudget::new(100_000, 1_234, 5_000)
}

fn call(code: &str, from: i64, to: i64) -> String {
    format!("/api/intraday/{code}.US?interval=5m&from={from}&to={to}")
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
fn the_intraday_layout_is_the_documented_one_and_the_windows_meet() {
    assert_eq!(
        INTRADAY_HEADER,
        "Timestamp,Gmtoffset,Datetime,Open,High,Low,Close,Volume"
    );
    assert_eq!(
        line(T0, 100.0),
        "1704205800,0,2024-01-02 14:30:00,99,101,98,100,100"
    );
    assert!(seeded(&[T0, T0 + 300]).starts_with(INTRADAY_HEADER));
    assert_eq!(seeded(&[T0, T0 + 300]).lines().count(), 3);
    assert_eq!(INTRADAY_CALL_COST, 5);
    let two = windows(at(T0), at(THROUGH), 600);
    assert_eq!(
        two,
        vec![
            Window {
                from: at(T0),
                to: at(T0 + 600 * DAY - 1)
            },
            Window {
                from: at(T0 + 600 * DAY),
                to: at(THROUGH)
            }
        ]
    );
    assert_eq!(windows(at(T0), at(T0 + 120 * DAY), 120).len(), 2);
    assert_eq!(windows(at(T0), at(T0 + 120 * DAY - 1), 120).len(), 1);
    assert!(windows(at(T0), at(T0 - 1), 120).is_empty());
}

/// SPY, 700 days behind, is extended across two 600-day windows with two requests (ten
/// calls), AAPL, one bar behind, with one; the bar each file already holds comes back from
/// the provider and is not written twice; NVDA, with no file, is backfilled from the
/// from-date across two windows, the provider's bar before the from-date never asked for;
/// NORO, which the provider has no bars for, is skipped with the reason after its windows.
/// Then a rerun at the same instant asks each file's last bar back and writes nothing.
#[tokio::test]
async fn a_file_is_extended_across_two_windows_a_missing_symbol_backfilled_and_a_rerun_writes_nothing()
 {
    let mut s = Scenario::new("windows", &["SPY", "AAPL", "NVDA", "NORO"]).await;
    s.seed("SPY", &[T0 - 300, T0]);
    s.seed("AAPL", &[THROUGH - 600, THROUGH - 300]);
    s.set_rows(
        "SPY",
        vec![
            row(T0, 101.0),
            row(T0 + 300, 102.0),
            row(T0 + 600 * DAY + 300, 103.0),
            row(THROUGH + 300, 104.0),
        ],
    );
    s.set_rows("AAPL", vec![row(THROUGH - 300, 101.0), row(THROUGH, 102.0)]);
    s.set_rows(
        "NVDA",
        vec![
            row(FROM_DATE_EPOCH - 300, 9.0),
            row(FROM_DATE_EPOCH + 3_600, 10.0),
            row(FROM_DATE_EPOCH + 600 * DAY + 600, 11.0),
        ],
    );
    s.set_rows("NORO", Vec::new());
    let spy_before = s.read("SPY");
    let aapl_before = s.read("AAPL");

    let mut budget = plenty();
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(outcome.error, None);
    assert_eq!(
        (
            outcome.requests,
            outcome.calls,
            outcome.added,
            outcome.updated
        ),
        (7, 35, 1, 2),
        "{outcome:?}\n{}",
        s.log
    );
    assert_eq!(outcome.estimate, Some(Estimate::intraday_windows(3, 4)));
    assert_eq!(outcome.backfills_left, 0);
    assert_eq!(budget.used, 1_234 + 35, "every request was charged five");
    assert_eq!(outcome.skipped.len(), 1, "{outcome:?}");
    assert_eq!(outcome.skipped[0].symbol, "NORO.US");
    assert!(
        outcome.skipped[0].reason.starts_with("backfill: no bars from the provider between 2024-01-01T00:00:00Z and 2025-12-02T14:30:00Z"),
        "{outcome:?}"
    );
    assert_eq!(
        s.calls(),
        [
            call("SPY", T0, T0 + 600 * DAY - 1),
            call("SPY", T0 + 600 * DAY, THROUGH),
            call("AAPL", THROUGH - 300, THROUGH),
            call("NVDA", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1),
            call("NVDA", FROM_DATE_EPOCH + 600 * DAY, THROUGH),
            call("NORO", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1),
            call("NORO", FROM_DATE_EPOCH + 600 * DAY, THROUGH),
        ]
    );

    let spy = s.read("SPY");
    assert_eq!(
        spy,
        format!(
            "{spy_before}{}\n{}\n",
            line(T0 + 300, 102.0),
            line(T0 + 600 * DAY + 300, 103.0)
        ),
        "SPY gained the two windows' bars after its last one, and that one once"
    );
    assert_eq!(
        s.read("AAPL"),
        format!("{aapl_before}{}\n", line(THROUGH, 102.0))
    );
    assert_eq!(
        s.read("NVDA"),
        format!(
            "{INTRADAY_HEADER}\n{}\n{}\n",
            line(FROM_DATE_EPOCH + 3_600, 10.0),
            line(FROM_DATE_EPOCH + 600 * DAY + 600, 11.0)
        ),
        "the backfill starts at the from-date"
    );
    assert!(!s.file("NORO").exists());
    s.no_part_files();
    assert_progress_is_monotonic_and_ends_at_100(&s.progress);
    let last = s.progress.last().unwrap();
    assert_eq!(
        (last.calls, last.added, last.updated, last.skipped),
        (35, 1, 2, 1)
    );
    assert!(s.log.contains("appended 2 bars"), "{}", s.log);
    assert!(s.log.contains("appended 1 bars"), "{}", s.log);
    assert!(s.log.contains("backfilled"), "{}", s.log);
    assert!(
        s.log.contains("complete: 7 requests (35 calls)"),
        "{}",
        s.log
    );

    // The rerun: every file is asked from its last bar, which comes back and is not written;
    // NORO is asked again (a skip lasts the job, not the dataset) and skipped again.
    let snapshot = s.snapshot();
    s.take_calls();
    let mut budget = plenty();
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (
            outcome.requests,
            outcome.calls,
            outcome.added,
            outcome.updated
        ),
        (5, 25, 0, 0),
        "{outcome:?}"
    );
    assert_eq!(outcome.estimate, Some(Estimate::intraday_windows(3, 2)));
    assert_eq!(outcome.skipped.len(), 1);
    assert_eq!(
        s.calls(),
        [
            call("SPY", T0 + 600 * DAY + 300, THROUGH),
            call("AAPL", THROUGH, THROUGH),
            call("NVDA", FROM_DATE_EPOCH + 600 * DAY + 600, THROUGH),
            call("NORO", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1),
            call("NORO", FROM_DATE_EPOCH + 600 * DAY, THROUGH),
        ]
    );
    assert_eq!(s.snapshot(), snapshot, "a rerun touched a file");
    assert!(s.log.contains("is current; nothing appended"), "{}", s.log);
    s.no_part_files();
    assert_progress_is_monotonic_and_ends_at_100(&s.progress);

    // An instant before the from-date and the files' last bars: nothing is asked at all.
    s.take_calls();
    let outcome = s.run(FROM_DATE_EPOCH - 1, &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (outcome.requests, outcome.added, outcome.updated),
        (0, 0, 0)
    );
    assert!(s.calls().is_empty(), "{:?}", s.calls());
    assert_eq!(s.snapshot(), snapshot, "a file was made for nothing");
    assert!(!s.file("NORO").exists());
    assert!(outcome.skipped.is_empty(), "{outcome:?}");
    assert_eq!(s.progress.last().unwrap().percent, 100);
    s.cleanup();
}

/// With the files current, a budget that reaches its reserve on the increments makes no
/// backfill request and counts both symbols left; one that admits one more request fetches
/// NVDA's first window, writes the file from it with no part file, and counts TSLA left;
/// the rerun with a fresh budget extends NVDA from the bar it holds and backfills TSLA. A
/// budget short of the increments refuses before any call, saying both numbers.
#[tokio::test]
async fn the_reserve_stops_the_backfill_cleanly_and_the_rerun_continues_from_the_symbol_left() {
    let mut s = Scenario::new("reserve", &["SPY", "AAPL", "NVDA", "TSLA"]).await;
    s.seed("SPY", &[THROUGH - 300, THROUGH]);
    s.seed("AAPL", &[THROUGH - 300, THROUGH]);
    s.set_rows("SPY", vec![row(THROUGH, 101.0)]);
    s.set_rows("AAPL", vec![row(THROUGH, 101.0)]);
    s.set_rows(
        "NVDA",
        vec![
            row(FROM_DATE_EPOCH + 3_600, 10.0),
            row(FROM_DATE_EPOCH + 600 * DAY + 600, 11.0),
        ],
    );
    s.set_rows(
        "TSLA",
        vec![
            row(FROM_DATE_EPOCH + 3_600, 20.0),
            row(FROM_DATE_EPOCH + 600 * DAY + 600, 21.0),
        ],
    );

    // Ten calls available: the two increments fill them and the backfill never starts.
    let mut budget = CallBudget::new(100, 0, 90);
    assert_eq!(budget.available(), 10);
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (
            outcome.requests,
            outcome.calls,
            outcome.added,
            outcome.updated
        ),
        (2, 10, 0, 0)
    );
    assert_eq!(outcome.backfills_left, 2, "{outcome:?}");
    assert_eq!(outcome.estimate, Some(Estimate::intraday_windows(2, 4)));
    assert!(budget.at_reserve());
    assert_eq!(
        s.calls(),
        [
            call("SPY", THROUGH, THROUGH),
            call("AAPL", THROUGH, THROUGH)
        ]
    );
    assert!(!s.file("NVDA").exists() && !s.file("TSLA").exists());
    assert!(
        s.log
            .contains("stopped at reserve: 2 symbols left to backfill, from NVDA.US"),
        "{}",
        s.log
    );
    s.no_part_files();

    // Fifteen available: one backfill request fits, NVDA's first window; its file holds
    // that window and the next run extends it.
    s.take_calls();
    let mut budget = CallBudget::new(100, 0, 85);
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (
            outcome.requests,
            outcome.calls,
            outcome.added,
            outcome.updated
        ),
        (3, 15, 1, 0)
    );
    assert_eq!(outcome.backfills_left, 1, "{outcome:?}");
    assert!(budget.at_reserve());
    assert_eq!(
        s.calls()[2],
        call("NVDA", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1)
    );
    assert_eq!(
        s.read("NVDA"),
        format!(
            "{INTRADAY_HEADER}\n{}\n",
            line(FROM_DATE_EPOCH + 3_600, 10.0)
        )
    );
    assert!(!s.file("TSLA").exists());
    s.no_part_files();
    assert!(
        s.log.contains(
            "stopped at reserve: NVDA.US holds 1 of 2 windows and the next run extends it; 1 symbols left to backfill"
        ),
        "{}",
        s.log
    );
    assert!(
        s.log.contains("stopped at reserve with 1 left"),
        "{}",
        s.log
    );

    // A fresh day's budget: NVDA is extended from its bar (two windows from it), TSLA is
    // backfilled whole, nothing is left.
    s.take_calls();
    let mut budget = CallBudget::new(100, 0, 5);
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (
            outcome.requests,
            outcome.calls,
            outcome.added,
            outcome.updated
        ),
        (6, 30, 1, 1),
        "{outcome:?}"
    );
    assert_eq!(outcome.backfills_left, 0);
    assert_eq!(
        s.calls()[2..],
        [
            call(
                "NVDA",
                FROM_DATE_EPOCH + 3_600,
                FROM_DATE_EPOCH + 3_600 + 600 * DAY - 1
            ),
            call("NVDA", FROM_DATE_EPOCH + 3_600 + 600 * DAY, THROUGH),
            call("TSLA", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1),
            call("TSLA", FROM_DATE_EPOCH + 600 * DAY, THROUGH),
        ]
    );
    assert_eq!(
        s.read("NVDA"),
        format!(
            "{INTRADAY_HEADER}\n{}\n{}\n",
            line(FROM_DATE_EPOCH + 3_600, 10.0),
            line(FROM_DATE_EPOCH + 600 * DAY + 600, 11.0)
        )
    );
    assert_eq!(
        s.read("TSLA"),
        format!(
            "{INTRADAY_HEADER}\n{}\n{}\n",
            line(FROM_DATE_EPOCH + 3_600, 20.0),
            line(FROM_DATE_EPOCH + 600 * DAY + 600, 21.0)
        )
    );
    s.no_part_files();

    // Four increments now need twenty calls; fifteen available is a refusal with both
    // numbers and no call.
    s.take_calls();
    let mut budget = CallBudget::new(100, 0, 85);
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    assert_eq!(
        outcome.error.as_deref(),
        Some("needs 20 calls, 15 available above the reserve of 85")
    );
    assert_eq!(outcome.estimate, Some(Estimate::intraday_windows(4, 0)));
    assert_eq!((outcome.requests, outcome.calls), (0, 0));
    assert!(s.calls().is_empty(), "{:?}", s.calls());
    assert_eq!(budget.used, 0);
    s.cleanup();
}

/// A dataset folder that has been removed is refused before any call and not created; so
/// is a resolution the provider has no intraday bars at.
#[tokio::test]
async fn a_removed_folder_or_a_daily_resolution_is_refused_with_no_call_made() {
    let mut s = Scenario::new("folder", &["SPY"]).await;
    s.seed("SPY", &[T0]);
    s.set_rows("SPY", vec![row(T0, 101.0), row(T0 + 300, 102.0)]);

    let mut input = s.input(THROUGH);
    input.resolution = "daily".into();
    let mut log: Vec<u8> = Vec::new();
    let mut budget = plenty();
    let outcome = intraday::run(&s.provider, &input, &mut budget, &mut log, &mut |_| {}).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    assert!(
        outcome
            .error
            .as_deref()
            .unwrap()
            .contains("no intraday bars at \"daily\""),
        "{outcome:?}"
    );
    assert_eq!(outcome.estimate, None);
    assert!(s.calls().is_empty());
    assert_eq!(budget.used, 1_234);
    assert_eq!(s.read("SPY"), seeded(&[T0]));

    fs::remove_dir_all(s.folder()).unwrap();
    let outcome = s.run(THROUGH, &mut budget).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(
        error.contains("missing") && error.contains("never creates"),
        "{error}"
    );
    assert_eq!((outcome.requests, outcome.calls), (0, 0));
    assert_eq!(outcome.estimate, None);
    assert!(s.calls().is_empty(), "a call was made: {:?}", s.calls());
    assert_eq!(budget.used, 1_234);
    assert!(!s.folder().exists(), "the folder was created");
    assert!(s.log.contains("refused"), "{}", s.log);
    assert_eq!(s.progress, Vec::<Progress>::new());
    s.cleanup();
}

/// A wrong token fails the job on its first request before any write, the token in no
/// message; a provider that goes away mid-run fails the job on the request that failed and
/// keeps every file written before it whole; a symbol the provider does not know (its 404)
/// is skipped with the status while the others proceed.
#[tokio::test]
async fn a_provider_failure_mid_run_fails_the_job_and_keeps_the_files_whole() {
    let mut s = Scenario::new("outage", &["SPY", "UNKN", "NVDA"]).await;
    s.seed("SPY", &[T0]);
    s.set_rows("SPY", vec![row(T0, 101.0), row(T0 + 300, 102.0)]);
    s.set_rows("NVDA", vec![row(FROM_DATE_EPOCH + 3_600, 10.0)]);
    let before = s.read("SPY");

    let wrong = Eodhd::new(s.provider.base_url(), "wrong-token-9999");
    let input = s.input(THROUGH);
    let mut log: Vec<u8> = Vec::new();
    let outcome = intraday::run(&wrong, &input, &mut plenty(), &mut log, &mut |_| {}).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}");
    let error = outcome.error.unwrap();
    assert!(
        error.starts_with("increment of SPY.US: credentials rejected"),
        "{error}"
    );
    assert!(!error.contains("wrong-token"), "{error}");
    assert_eq!((outcome.requests, outcome.calls), (1, 5));
    assert_eq!(s.read("SPY"), before);
    assert!(!String::from_utf8(log).unwrap().contains("wrong-token"));

    // NVDA answers 503: SPY's append stands, UNKN's 404 is a skip, the job fails on NVDA
    // with no file for it and no part file.
    s.take_calls();
    s.stub
        .answers
        .lock()
        .unwrap()
        .down
        .insert("NVDA.US".to_owned());
    let outcome = s.run(THROUGH, &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Failed, "{outcome:?}\n{}", s.log);
    let error = outcome.error.clone().unwrap();
    assert!(
        error.starts_with("backfill of NVDA.US: provider unreachable"),
        "{error}"
    );
    assert_eq!(
        (
            outcome.requests,
            outcome.calls,
            outcome.added,
            outcome.updated
        ),
        (4, 20, 0, 1),
        "{outcome:?}"
    );
    assert_eq!(outcome.skipped.len(), 1, "{outcome:?}");
    assert_eq!(outcome.skipped[0].symbol, "UNKN.US");
    assert!(
        outcome.skipped[0].reason.contains("backfill") && outcome.skipped[0].reason.contains("404"),
        "{outcome:?}"
    );
    assert_eq!(
        s.calls(),
        [
            call("SPY", T0, T0 + 600 * DAY - 1),
            call("SPY", T0 + 600 * DAY, THROUGH),
            call("UNKN", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1),
            call("NVDA", FROM_DATE_EPOCH, FROM_DATE_EPOCH + 600 * DAY - 1),
        ],
        "UNKN's second window is never asked after its 404"
    );
    assert_eq!(
        s.read("SPY"),
        format!("{before}{}\n", line(T0 + 300, 102.0))
    );
    assert!(!s.file("NVDA").exists());
    assert!(!s.file("UNKN").exists());
    s.no_part_files();

    // Back up, the rerun has SPY's windows from its last bar (still two: it is one bar
    // on), UNKN's 404, and NVDA's backfill to do.
    s.stub.answers.lock().unwrap().down.clear();
    s.take_calls();
    let outcome = s.run(THROUGH, &mut plenty()).await;
    assert_eq!(outcome.state, JobState::Complete, "{outcome:?}\n{}", s.log);
    assert_eq!(
        (outcome.requests, outcome.added, outcome.updated),
        (5, 1, 0),
        "{outcome:?}"
    );
    assert_eq!(
        s.calls()[0],
        call("SPY", T0 + 300, T0 + 300 + 600 * DAY - 1)
    );
    assert_eq!(
        s.read("NVDA"),
        format!(
            "{INTRADAY_HEADER}\n{}\n",
            line(FROM_DATE_EPOCH + 3_600, 10.0)
        )
    );
    s.cleanup();
}
