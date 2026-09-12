//! The EODHD adapter against a stub server (DS-02, decision 0020).
//!
//! An axum server on a free port serves the recorded fixtures under `tests/fixtures/eodhd/`
//! (token scrubbed, trimmed to a few rows) and records every query string it receives. The
//! tests assert the parsed account, exchanges, and listings, and (DS-08) the bulk bars, one
//! symbol's history, and the splits with the query each sends; that a 401 body is
//! `CredentialsRejected`, that a 503 and a dropped connection are `Unreachable`, that a
//! non-JSON body is `Malformed`; and that the token travels as the `api_token` query
//! parameter while no error message (the only text the adapter hands anyone to log) carries
//! it. Nothing here reaches eodhd.com.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{NaiveDate, TimeZone, Utc};
use tessera::provider::eodhd::Eodhd;
use tessera::provider::{Provider, ProviderError};

fn day(text: &str) -> NaiveDate {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").unwrap()
}

/// A placeholder, never a real token. The stub accepts exactly this one.
const TOKEN: &str = "stub-token-0000";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/eodhd/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

type Queries = Arc<Mutex<Vec<String>>>;

#[derive(Clone)]
struct Stub {
    queries: Queries,
}

fn json(status: StatusCode, body: String) -> Response {
    (status, [("content-type", "application/json")], body).into_response()
}

fn record(state: &Stub, path: &str, query: &Query<Vec<(String, String)>>) -> Option<Response> {
    let raw = query
        .0
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    state.queries.lock().unwrap().push(format!("{path}?{raw}"));
    let token = query
        .0
        .iter()
        .find(|(k, _)| k == "api_token")
        .map(|(_, v)| v.as_str());
    if token != Some(TOKEN) {
        return Some(json(
            StatusCode::UNAUTHORIZED,
            r#"{"message":"Unauthenticated","code":401}"#.to_owned(),
        ));
    }
    None
}

async fn user(State(state): State<Stub>, query: Query<Vec<(String, String)>>) -> Response {
    if let Some(rejected) = record(&state, "/api/user", &query) {
        return rejected;
    }
    json(StatusCode::OK, fixture("user"))
}

async fn exchanges(State(state): State<Stub>, query: Query<Vec<(String, String)>>) -> Response {
    if let Some(rejected) = record(&state, "/api/exchanges-list/", &query) {
        return rejected;
    }
    json(StatusCode::OK, fixture("exchanges-list"))
}

async fn symbols(
    State(state): State<Stub>,
    Path(exchange): Path<String>,
    query: Query<Vec<(String, String)>>,
) -> Response {
    if let Some(rejected) = record(
        &state,
        &format!("/api/exchange-symbol-list/{exchange}"),
        &query,
    ) {
        return rejected;
    }
    if exchange == "US" {
        json(StatusCode::OK, fixture("exchange-symbol-list-US"))
    } else {
        json(
            StatusCode::NOT_FOUND,
            r#"{"message":"Unknown exchange"}"#.to_owned(),
        )
    }
}

/// `eod-bulk-last-day/{exchange}`: the recorded bars for US on 2026-09-11, the recorded
/// splits with `type=splits`, an empty list for any other date (a holiday), the provider's
/// 404 for another exchange.
async fn bulk(
    State(state): State<Stub>,
    Path(exchange): Path<String>,
    query: Query<Vec<(String, String)>>,
) -> Response {
    if let Some(rejected) = record(
        &state,
        &format!("/api/eod-bulk-last-day/{exchange}"),
        &query,
    ) {
        return rejected;
    }
    let param = |name: &str| {
        query
            .0
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    if exchange != "US" {
        return json(
            StatusCode::NOT_FOUND,
            r#"{"message":"Unknown exchange"}"#.to_owned(),
        );
    }
    let splits = param("type") == Some("splits");
    if param("date") != Some("2026-09-11") {
        return json(StatusCode::OK, "[]".to_owned());
    }
    if splits {
        json(StatusCode::OK, fixture("eod-bulk-last-day-US-splits"))
    } else {
        json(StatusCode::OK, fixture("eod-bulk-last-day-US"))
    }
}

/// `eod/{symbol}`: AAPL.US's recorded three rows, the rows from `from` on; 404 for another
/// symbol, as the provider answers for one it does not know.
async fn history(
    State(state): State<Stub>,
    Path(symbol): Path<String>,
    query: Query<Vec<(String, String)>>,
) -> Response {
    if let Some(rejected) = record(&state, &format!("/api/eod/{symbol}"), &query) {
        return rejected;
    }
    if symbol != "AAPL.US" {
        return json(
            StatusCode::NOT_FOUND,
            r#"{"message":"Symbol not found"}"#.to_owned(),
        );
    }
    let from = query
        .0
        .iter()
        .find(|(k, _)| k == "from")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&fixture("eod-AAPL.US")).unwrap();
    let kept: Vec<serde_json::Value> = rows
        .into_iter()
        .filter(|row| row["date"].as_str().unwrap() >= from.as_str())
        .collect();
    json(StatusCode::OK, serde_json::to_string(&kept).unwrap())
}

/// `intraday/{symbol}`: SPY.US's recorded three 5-minute rows, those whose timestamp lies
/// in `from..=to`, for `interval=5m`; an empty list at another interval; 404 for another
/// symbol.
async fn intraday(
    State(state): State<Stub>,
    Path(symbol): Path<String>,
    query: Query<Vec<(String, String)>>,
) -> Response {
    if let Some(rejected) = record(&state, &format!("/api/intraday/{symbol}"), &query) {
        return rejected;
    }
    if symbol != "SPY.US" {
        return json(
            StatusCode::NOT_FOUND,
            r#"{"message":"Symbol not found"}"#.to_owned(),
        );
    }
    let param = |name: &str| -> Option<i64> {
        query
            .0
            .iter()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.parse().ok())
    };
    let interval = query
        .0
        .iter()
        .find(|(k, _)| k == "interval")
        .map(|(_, v)| v.as_str());
    if interval != Some("5m") {
        return json(StatusCode::OK, "[]".to_owned());
    }
    let (from, to) = (param("from").unwrap_or(0), param("to").unwrap_or(i64::MAX));
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&fixture("intraday-SPY.US-5m")).unwrap();
    let kept: Vec<serde_json::Value> = rows
        .into_iter()
        .filter(|row| {
            let ts = row["timestamp"].as_i64().unwrap();
            ts >= from && ts <= to
        })
        .collect();
    json(StatusCode::OK, serde_json::to_string(&kept).unwrap())
}

fn stub_router() -> (Router, Queries) {
    let queries: Queries = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/api/user", get(user))
        .route("/api/exchanges-list/", get(exchanges))
        .route("/api/exchange-symbol-list/{exchange}", get(symbols))
        .route("/api/eod-bulk-last-day/{exchange}", get(bulk))
        .route("/api/eod/{symbol}", get(history))
        .route("/api/intraday/{symbol}", get(intraday))
        .with_state(Stub {
            queries: queries.clone(),
        });
    (router, queries)
}

/// Binds a free port, serves `router` on it in the background, and returns the base URL.
async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

fn assert_never_prints_token(err: &ProviderError, secrets: &[&str]) {
    let display = err.to_string();
    let debug = format!("{err:?}");
    for secret in secrets {
        assert!(
            !display.contains(secret),
            "display leaks the token: {display}"
        );
        assert!(!debug.contains(secret), "debug leaks the token: {debug}");
    }
}

#[tokio::test]
async fn verify_parses_the_account_and_sends_the_token_as_a_query_parameter() {
    let (router, queries) = stub_router();
    let base = serve(router).await;
    let provider = Eodhd::new(&base, TOKEN);

    let account = provider.verify(TOKEN).await.unwrap();
    assert_eq!(account.plan, "All-In-One");
    assert_eq!(account.requests_today, 1234);
    assert_eq!(account.daily_limit, 100_000);
    assert_eq!(
        account.resets_at,
        Utc.with_ymd_and_hms(2026, 9, 12, 0, 0, 0).unwrap()
    );

    let seen = queries.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("/api/user?"), "{}", seen[0]);
    assert!(
        seen[0].contains(&format!("api_token={TOKEN}")),
        "{}",
        seen[0]
    );
    assert!(seen[0].contains("fmt=json"), "{}", seen[0]);
}

#[tokio::test]
async fn exchanges_and_symbols_parse_the_recorded_lists() {
    let (router, queries) = stub_router();
    let base = serve(router).await;
    let provider = Eodhd::new(&base, TOKEN);

    let exchanges = provider.exchanges().await.unwrap();
    let codes: Vec<&str> = exchanges.iter().map(|e| e.code.as_str()).collect();
    assert_eq!(codes, ["US", "LSE", "TO", "CC"]);
    let us = &exchanges[0];
    assert_eq!(us.name, "USA Stocks");
    assert_eq!(us.country, "USA");
    assert_eq!(us.resolutions, ["daily", "1h", "5m", "1m"]);
    let lse = &exchanges[1];
    assert_eq!(
        (lse.name.as_str(), lse.country.as_str()),
        ("London Exchange", "UK")
    );
    assert_eq!(lse.resolutions, ["daily", "1h", "5m"]);
    assert_eq!(exchanges[3].resolutions, ["daily", "1h", "5m", "1m"]);

    let listings = provider.symbols("US", false).await.unwrap();
    assert_eq!(listings.len(), 3);
    assert_eq!(listings[0].code, "AAPL");
    assert_eq!(listings[0].name, "Apple Inc");
    assert_eq!(listings[0].kind, "Common Stock");
    assert_eq!(listings[0].currency, "USD");
    assert_eq!(listings[0].country, "USA");
    assert_eq!(listings[0].venue, "NASDAQ");
    assert_eq!(listings[1].code, "SPY");
    assert_eq!(listings[1].kind, "ETF");
    assert_eq!(listings[1].venue, "NYSE ARCA");
    assert_eq!(listings[2].code, "BRK-B");

    let delisted = provider.symbols("US", true).await.unwrap();
    assert_eq!(delisted.len(), 3);

    let seen = queries.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].starts_with("/api/exchanges-list/?"), "{}", seen[0]);
    assert!(
        seen[1].starts_with("/api/exchange-symbol-list/US?"),
        "{}",
        seen[1]
    );
    assert!(!seen[1].contains("delisted"), "{}", seen[1]);
    assert!(seen[2].contains("delisted=1"), "{}", seen[2]);
    for line in &seen {
        assert!(line.contains(&format!("api_token={TOKEN}")), "{line}");
        assert!(line.contains("fmt=json"), "{line}");
    }
}

/// DS-08: the three download calls parse the recorded answers and send the query the
/// provider documents: `date=` for a bulk day, `type=splits&date=` for its splits, `from=`
/// for a history; a date the provider has no bars for is an empty list, not an error.
#[tokio::test]
async fn bulk_bars_history_and_splits_parse_the_recorded_answers_and_send_their_queries() {
    let (router, queries) = stub_router();
    let base = serve(router).await;
    let provider = Eodhd::new(&base, TOKEN);

    let bars = provider.bulk_eod("US", day("2026-09-11")).await.unwrap();
    assert_eq!(bars.len(), 3);
    assert_eq!(
        (bars[0].code.as_str(), bars[0].exchange.as_str()),
        ("AAPL", "US")
    );
    assert_eq!(bars[0].bar.date, day("2026-09-11"));
    assert_eq!(bars[0].bar.close, 232.5);
    assert_eq!(bars[0].bar.volume, 51_234_567.0);
    assert_eq!(bars[1].code, "SPY");
    assert_eq!(bars[2].code, "BRK-B");
    assert_eq!(bars[2].bar.low, 469.1);
    let holiday = provider.bulk_eod("US", day("2026-09-12")).await.unwrap();
    assert!(holiday.is_empty());

    let history = provider
        .eod_history("AAPL.US", day("2026-09-10"))
        .await
        .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].date, day("2026-09-10"));
    assert_eq!(history[0].adjusted_close, 57.6);
    assert_eq!(history[1].date, day("2026-09-11"));
    assert_eq!((history[1].open, history[1].high), (230.1, 233.4));

    let splits = provider.splits("US", day("2026-09-11")).await.unwrap();
    assert_eq!(splits.len(), 1);
    assert_eq!(
        (splits[0].code.as_str(), splits[0].exchange.as_str()),
        ("AAPL", "US")
    );
    assert_eq!(splits[0].date, day("2026-09-11"));
    assert_eq!(splits[0].ratio, "4.000000/1.000000");
    assert!(
        provider
            .splits("US", day("2026-09-10"))
            .await
            .unwrap()
            .is_empty()
    );

    let seen = queries.lock().unwrap().clone();
    assert_eq!(seen.len(), 5);
    assert!(
        seen[0].starts_with("/api/eod-bulk-last-day/US?"),
        "{}",
        seen[0]
    );
    assert!(seen[0].contains("date=2026-09-11"), "{}", seen[0]);
    assert!(!seen[0].contains("type="), "{}", seen[0]);
    assert!(seen[1].contains("date=2026-09-12"), "{}", seen[1]);
    assert!(seen[2].starts_with("/api/eod/AAPL.US?"), "{}", seen[2]);
    assert!(seen[2].contains("from=2026-09-10"), "{}", seen[2]);
    assert!(seen[3].contains("type=splits"), "{}", seen[3]);
    assert!(seen[3].contains("date=2026-09-11"), "{}", seen[3]);
    for line in &seen {
        assert!(line.contains(&format!("api_token={TOKEN}")), "{line}");
        assert!(line.contains("fmt=json"), "{line}");
    }

    // A symbol the provider does not know is its 404: Malformed, not the network.
    let err = provider
        .eod_history("NOPE.US", day("2026-09-10"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("404"), "{err}");
    assert_never_prints_token(&err, &[TOKEN]);
    let err = provider
        .bulk_eod("MARS", day("2026-09-11"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Malformed(_)), "{err:?}");
    assert_never_prints_token(&err, &[TOKEN]);

    // A wrong token is rejected on every download call, the token in no message.
    let wrong = "wrong-token-9999";
    let provider = Eodhd::new(&base, wrong);
    for err in [
        provider
            .bulk_eod("US", day("2026-09-11"))
            .await
            .unwrap_err(),
        provider
            .eod_history("AAPL.US", day("2026-09-10"))
            .await
            .unwrap_err(),
        provider.splits("US", day("2026-09-11")).await.unwrap_err(),
    ] {
        assert!(
            matches!(err, ProviderError::CredentialsRejected(_)),
            "{err:?}"
        );
        assert_never_prints_token(&err, &[TOKEN, wrong]);
    }
}

/// The intraday endpoint (DS-09): the recorded 5-minute rows parse with a null volume as
/// zero, the query carries the interval and the window as epoch seconds, the window the
/// provider offers per request is its own table, a daily resolution has no interval and
/// makes no call, an unknown symbol is the provider's 404, and a wrong token is rejected
/// with the token in no message.
#[tokio::test]
async fn intraday_bars_parse_the_recorded_answer_and_send_the_interval_and_window() {
    let (router, queries) = stub_router();
    let base = serve(router).await;
    let provider = Eodhd::new(&base, TOKEN);
    let from = Utc.with_ymd_and_hms(2024, 1, 2, 14, 30, 0).unwrap();
    let to = Utc.with_ymd_and_hms(2024, 1, 2, 14, 40, 0).unwrap();

    let bars = provider.intraday("SPY.US", "5m", from, to).await.unwrap();
    assert_eq!(bars.len(), 3);
    assert_eq!(bars[0].timestamp, 1_704_205_800);
    assert_eq!(bars[0].gmtoffset, 0);
    assert_eq!((bars[0].open, bars[0].close), (472.16, 473.2));
    assert_eq!(bars[0].volume, 1_523_401.0);
    assert_eq!(bars[2].timestamp, 1_704_206_400);
    assert_eq!(bars[2].volume, 0.0, "a null volume reads as zero");
    let later = provider
        .intraday("SPY.US", "5m", from + chrono::Duration::seconds(1), to)
        .await
        .unwrap();
    assert_eq!(later.len(), 2);
    assert_eq!(provider.intraday_window_days("5m"), Some(600));
    assert_eq!(provider.intraday_window_days("1m"), Some(120));
    assert_eq!(provider.intraday_window_days("daily"), None);

    let seen = queries.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].starts_with("/api/intraday/SPY.US?"), "{}", seen[0]);
    assert!(seen[0].contains("interval=5m"), "{}", seen[0]);
    assert!(seen[0].contains("from=1704205800"), "{}", seen[0]);
    assert!(seen[0].contains("to=1704206400"), "{}", seen[0]);
    assert!(seen[0].contains("fmt=json"), "{}", seen[0]);
    assert!(seen[1].contains("from=1704205801"), "{}", seen[1]);

    let err = provider
        .intraday("SPY.US", "daily", from, to)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("no intraday bars"), "{err}");
    assert_eq!(queries.lock().unwrap().len(), 2, "a daily request was sent");
    let err = provider
        .intraday("NOPE.US", "5m", from, to)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("404"), "{err}");
    assert_never_prints_token(&err, &[TOKEN]);

    let wrong = "wrong-token-9999";
    let provider = Eodhd::new(&base, wrong);
    let err = provider
        .intraday("SPY.US", "5m", from, to)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::CredentialsRejected(_)),
        "{err:?}"
    );
    assert_never_prints_token(&err, &[TOKEN, wrong]);
}

#[tokio::test]
async fn a_401_body_is_credentials_rejected_and_no_message_carries_the_token() {
    let (router, _queries) = stub_router();
    let base = serve(router).await;
    let wrong = "wrong-token-9999";

    let err = Eodhd::new(&base, TOKEN).verify(wrong).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::CredentialsRejected(_)),
        "{err:?}"
    );
    assert!(err.to_string().contains("Unauthenticated"), "{err}");
    assert_never_prints_token(&err, &[TOKEN, wrong]);

    let provider = Eodhd::new(&base, wrong);
    let err = provider.exchanges().await.unwrap_err();
    assert!(
        matches!(err, ProviderError::CredentialsRejected(_)),
        "{err:?}"
    );
    assert_never_prints_token(&err, &[TOKEN, wrong]);
    let err = provider.symbols("US", false).await.unwrap_err();
    assert!(
        matches!(err, ProviderError::CredentialsRejected(_)),
        "{err:?}"
    );
    assert_never_prints_token(&err, &[TOKEN, wrong]);
}

#[tokio::test]
async fn a_503_is_unreachable() {
    let router = Router::new().fallback(|| async {
        (StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response()
    });
    let base = serve(router).await;
    let provider = Eodhd::new(&base, TOKEN);

    for err in [
        provider.verify(TOKEN).await.unwrap_err(),
        provider.exchanges().await.unwrap_err(),
        provider.symbols("US", false).await.unwrap_err(),
    ] {
        assert!(matches!(err, ProviderError::Unreachable(_)), "{err:?}");
        assert!(err.to_string().contains("503"), "{err}");
        assert_never_prints_token(&err, &[TOKEN]);
    }
}

#[tokio::test]
async fn a_dropped_connection_is_unreachable() {
    // A listener that accepts each connection and closes it without answering.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dropping = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            drop(stream);
        }
    });
    let provider = Eodhd::new(&dropping, TOKEN);
    let err = provider.verify(TOKEN).await.unwrap_err();
    assert!(matches!(err, ProviderError::Unreachable(_)), "{err:?}");
    assert_never_prints_token(&err, &[TOKEN]);

    // Nothing listening at all: the port was free a moment ago and is closed now.
    let closed = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let provider = Eodhd::new(&closed, TOKEN);
    let err = provider.exchanges().await.unwrap_err();
    assert!(matches!(err, ProviderError::Unreachable(_)), "{err:?}");
    assert_never_prints_token(&err, &[TOKEN]);
}

#[tokio::test]
async fn a_body_that_is_not_the_expected_json_is_malformed() {
    let router = Router::new()
        .route("/api/user", get(|| async { "<html>maintenance</html>" }))
        .route(
            "/api/exchanges-list/",
            get(|| async { json(StatusCode::OK, r#"{"not":"a list"}"#.to_owned()) }),
        )
        .route(
            "/api/exchange-symbol-list/{exchange}",
            get(|| async { json(StatusCode::OK, r#"[{"Code":1}]"#.to_owned()) }),
        );
    let base = serve(router).await;
    let provider = Eodhd::new(&base, TOKEN);

    for err in [
        provider.verify(TOKEN).await.unwrap_err(),
        provider.exchanges().await.unwrap_err(),
        provider.symbols("US", false).await.unwrap_err(),
    ] {
        assert!(matches!(err, ProviderError::Malformed(_)), "{err:?}");
        assert_never_prints_token(&err, &[TOKEN]);
    }

    // An exchange the provider does not know answers 404: not credentials, not the network.
    let (router, _queries) = stub_router();
    let base = serve(router).await;
    let err = Eodhd::new(&base, TOKEN)
        .symbols("NOPE", false)
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Malformed(_)), "{err:?}");
    assert!(err.to_string().contains("404"), "{err}");
    assert_never_prints_token(&err, &[TOKEN]);
}
