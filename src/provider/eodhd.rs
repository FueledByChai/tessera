//! The EODHD adapter (decision 0020): the user, exchanges-list, and exchange-symbol-list
//! endpoints, and the download endpoints the jobs use (eod-bulk-last-day for bars and for
//! splits, eod for one symbol's history, intraday for one symbol's bars in a window), over
//! reqwest with rustls.
//!
//! Every call sends the token as the `api_token` query parameter and asks for `fmt=json`
//! (the list endpoints answer CSV without it). The base URL is configurable so tests run
//! against a stub server (`tests/provider_eodhd.rs`) and nothing in the crate calls
//! eodhd.com unless the service does. No line here logs, and every error message is
//! scrubbed of the token before it leaves the adapter: reqwest's own errors would print the
//! request URL, token included, so they are stripped of it first.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::{
    Account, Bar, BulkBar, Exchange, IntradayBar, Listing, Provider, ProviderError, Split,
};

/// Where the public API lives; a stub server replaces it in tests.
pub const DEFAULT_BASE_URL: &str = "https://eodhd.com";

/// Bar resolutions, in the engine's names, that every EODHD exchange offers.
const COMMON_RESOLUTIONS: [&str; 3] = ["daily", "1h", "5m"];
/// Exchanges with 1-minute intraday history as well.
const ONE_MINUTE_EXCHANGES: [&str; 3] = ["US", "FOREX", "CC"];

/// The intraday intervals EODHD serves, by the engine's resolution name, with the longest
/// span in days one request may cover (the API's own limits: 120 days of 1-minute bars,
/// 600 of 5-minute, 7200 of hourly).
const INTRADAY_INTERVALS: [(&str, &str, u32); 3] =
    [("1m", "1m", 120), ("5m", "5m", 600), ("1h", "1h", 7200)];

/// The provider's interval name and window for `resolution`; `None` for a resolution it
/// has no intraday bars at (daily included).
fn intraday_interval(resolution: &str) -> Option<(&'static str, u32)> {
    INTRADAY_INTERVALS
        .iter()
        .find(|(name, _, _)| *name == resolution)
        .map(|(_, interval, days)| (*interval, *days))
}

/// An EODHD account: a base URL, the token the listing calls use, and an HTTP client.
#[derive(Debug, Clone)]
pub struct Eodhd {
    base_url: String,
    token: String,
    client: reqwest::Client,
}

impl Eodhd {
    /// An adapter over `base_url` (no trailing slash needed) using `token` for its calls.
    pub fn new(base_url: &str, token: &str) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client with default TLS");
        Eodhd {
            base_url: base_url.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
            client,
        }
    }

    /// An adapter over the public API.
    pub fn public(token: &str) -> Self {
        Eodhd::new(DEFAULT_BASE_URL, token)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// GET `path` with the token, `fmt=json`, and `extra` query parameters, decoded as `T`.
    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        extra: &[(&str, &str)],
    ) -> Result<T, ProviderError> {
        let mut query: Vec<(&str, &str)> = vec![("api_token", token), ("fmt", "json")];
        query.extend_from_slice(extra);
        let response = self
            .client
            .get(format!("{}{path}", self.base_url))
            .query(&query)
            .send()
            .await
            .map_err(|e| ProviderError::Unreachable(scrub(&e.without_url().to_string(), token)))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| ProviderError::Unreachable(scrub(&e.without_url().to_string(), token)))?;
        if let Some(err) = classify(status, &body, token) {
            return Err(err);
        }
        serde_json::from_str(&body)
            .map_err(|e| ProviderError::Malformed(scrub(&format!("{path}: {e}"), token)))
    }
}

/// The error a non-success status stands for, or `None` for a 2xx.
fn classify(status: StatusCode, body: &str, token: &str) -> Option<ProviderError> {
    if status.is_success() {
        return None;
    }
    let message = scrub(&message_of(body, status), token);
    Some(match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            ProviderError::CredentialsRejected(message)
        }
        StatusCode::TOO_MANY_REQUESTS => {
            ProviderError::Unreachable(format!("HTTP {status}: {message}"))
        }
        s if s.is_server_error() => ProviderError::Unreachable(format!("HTTP {status}: {message}")),
        _ => ProviderError::Malformed(format!("HTTP {status}: {message}")),
    })
}

/// The provider's message for an error body: its JSON `message` field when there is one,
/// otherwise a short prefix of the body, otherwise the status reason.
fn message_of(body: &str, status: StatusCode) -> String {
    #[derive(Deserialize)]
    struct WithMessage {
        message: String,
    }
    if let Ok(WithMessage { message }) = serde_json::from_str::<WithMessage>(body) {
        return message;
    }
    let text: String = body.trim().chars().take(120).collect();
    if text.is_empty() {
        status.canonical_reason().unwrap_or("no body").to_owned()
    } else {
        text
    }
}

/// `text` with every occurrence of the token replaced, so no message can print it.
fn scrub(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_owned()
    } else {
        text.replace(token, "[token]")
    }
}

/// The resolutions EODHD offers on `exchange`, in the engine's names.
fn resolutions_for(exchange: &str) -> Vec<String> {
    let mut out: Vec<String> = COMMON_RESOLUTIONS.iter().map(|s| (*s).to_owned()).collect();
    if ONE_MINUTE_EXCHANGES.contains(&exchange) {
        out.push("1m".to_owned());
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserResponse {
    subscription_type: String,
    api_requests: u64,
    api_requests_date: String,
    daily_rate_limit: u64,
}

/// The counter resets at midnight UTC after the day it counts.
fn account_of(user: UserResponse) -> Result<Account, ProviderError> {
    let counted = NaiveDate::parse_from_str(&user.api_requests_date, "%Y-%m-%d").map_err(|e| {
        ProviderError::Malformed(format!("apiRequestsDate {:?}: {e}", user.api_requests_date))
    })?;
    let resets_at: DateTime<Utc> = counted
        .succ_opt()
        .ok_or_else(|| ProviderError::Malformed("apiRequestsDate out of range".to_owned()))?
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc();
    Ok(Account {
        plan: user.subscription_type,
        requests_today: user.api_requests,
        daily_limit: user.daily_rate_limit,
        resets_at,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ExchangeRow {
    code: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    country: String,
}

impl From<ExchangeRow> for Exchange {
    fn from(row: ExchangeRow) -> Self {
        let resolutions = resolutions_for(&row.code);
        Exchange {
            code: row.code,
            name: row.name,
            country: row.country,
            resolutions,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListingRow {
    code: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "Type")]
    kind: String,
    #[serde(default)]
    currency: String,
    #[serde(default)]
    country: String,
    #[serde(default)]
    exchange: String,
}

impl From<ListingRow> for Listing {
    fn from(row: ListingRow) -> Self {
        Listing {
            code: row.code,
            name: row.name,
            kind: row.kind,
            currency: row.currency,
            country: row.country,
            venue: row.exchange,
        }
    }
}

/// A bar as the eod and eod-bulk-last-day endpoints write it. Prices come as numbers;
/// a missing volume (some instruments report none) is zero. A row missing a price is
/// dropped by the caller: nothing is invented for it.
#[derive(Deserialize)]
struct BarRow {
    date: String,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    close: Option<f64>,
    adjusted_close: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
}

impl BarRow {
    /// The bar, or `None` when a price is missing or the date is not a date.
    fn bar(&self) -> Option<Bar> {
        Some(Bar {
            date: NaiveDate::parse_from_str(&self.date, "%Y-%m-%d").ok()?,
            open: self.open?,
            high: self.high?,
            low: self.low?,
            close: self.close?,
            adjusted_close: self.adjusted_close?,
            volume: self.volume.unwrap_or(0.0),
        })
    }
}

/// A bulk row: a bar with the instrument it belongs to.
#[derive(Deserialize)]
struct BulkRow {
    code: String,
    #[serde(default)]
    exchange_short_name: String,
    #[serde(flatten)]
    bar: BarRow,
}

/// A split row from eod-bulk-last-day with `type=splits`.
#[derive(Deserialize)]
struct SplitRow {
    code: String,
    #[serde(default)]
    exchange: String,
    date: String,
    split: String,
}

/// A bar as the intraday endpoint writes it: the start as UTC epoch seconds, the offset
/// (zero), the same instant as text, the prints, and the volume. A row missing its
/// timestamp or a price is dropped by the caller; the text is not read (the file derives
/// its own from the timestamp).
#[derive(Deserialize)]
struct IntradayRow {
    timestamp: Option<i64>,
    #[serde(default)]
    gmtoffset: Option<i64>,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
    close: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
}

impl IntradayRow {
    fn bar(&self) -> Option<IntradayBar> {
        Some(IntradayBar {
            timestamp: self.timestamp?,
            gmtoffset: self.gmtoffset.unwrap_or(0),
            open: self.open?,
            high: self.high?,
            low: self.low?,
            close: self.close?,
            volume: self.volume.unwrap_or(0.0),
        })
    }
}

/// The bars of `rows` in the order given, rows with a missing price or an unreadable date
/// left out.
fn bars_of(rows: Vec<BarRow>) -> Vec<Bar> {
    rows.iter().filter_map(BarRow::bar).collect()
}

fn intraday_bars_of(rows: Vec<IntradayRow>) -> Vec<IntradayBar> {
    rows.iter().filter_map(IntradayRow::bar).collect()
}

fn bulk_bars_of(rows: Vec<BulkRow>) -> Vec<BulkBar> {
    rows.into_iter()
        .filter_map(|row| {
            let bar = row.bar.bar()?;
            Some(BulkBar {
                code: row.code,
                exchange: row.exchange_short_name,
                bar,
            })
        })
        .collect()
}

fn splits_of(rows: Vec<SplitRow>) -> Result<Vec<Split>, ProviderError> {
    rows.into_iter()
        .map(|row| {
            let date = NaiveDate::parse_from_str(&row.date, "%Y-%m-%d").map_err(|e| {
                ProviderError::Malformed(format!("split date {:?} for {}: {e}", row.date, row.code))
            })?;
            Ok(Split {
                code: row.code,
                exchange: row.exchange,
                date,
                ratio: row.split,
            })
        })
        .collect()
}

impl Provider for Eodhd {
    async fn verify(&self, token: &str) -> Result<Account, ProviderError> {
        let user: UserResponse = self.get_json("/api/user", token, &[]).await?;
        account_of(user)
    }

    async fn exchanges(&self) -> Result<Vec<Exchange>, ProviderError> {
        let rows: Vec<ExchangeRow> = self
            .get_json("/api/exchanges-list/", &self.token, &[])
            .await?;
        Ok(rows.into_iter().map(Exchange::from).collect())
    }

    async fn symbols(&self, exchange: &str, delisted: bool) -> Result<Vec<Listing>, ProviderError> {
        let path = format!("/api/exchange-symbol-list/{exchange}");
        let extra: &[(&str, &str)] = if delisted { &[("delisted", "1")] } else { &[] };
        let rows: Vec<ListingRow> = self.get_json(&path, &self.token, extra).await?;
        Ok(rows.into_iter().map(Listing::from).collect())
    }

    async fn bulk_eod(
        &self,
        exchange: &str,
        date: NaiveDate,
    ) -> Result<Vec<BulkBar>, ProviderError> {
        let path = format!("/api/eod-bulk-last-day/{exchange}");
        let date = date.to_string();
        let rows: Vec<BulkRow> = self
            .get_json(&path, &self.token, &[("date", date.as_str())])
            .await?;
        Ok(bulk_bars_of(rows))
    }

    async fn eod_history(&self, symbol: &str, from: NaiveDate) -> Result<Vec<Bar>, ProviderError> {
        let path = format!("/api/eod/{symbol}");
        let from = from.to_string();
        let rows: Vec<BarRow> = self
            .get_json(
                &path,
                &self.token,
                &[("from", from.as_str()), ("period", "d")],
            )
            .await?;
        Ok(bars_of(rows))
    }

    async fn splits(&self, exchange: &str, date: NaiveDate) -> Result<Vec<Split>, ProviderError> {
        let path = format!("/api/eod-bulk-last-day/{exchange}");
        let date = date.to_string();
        let rows: Vec<SplitRow> = self
            .get_json(
                &path,
                &self.token,
                &[("type", "splits"), ("date", date.as_str())],
            )
            .await?;
        splits_of(rows)
    }

    fn intraday_window_days(&self, resolution: &str) -> Option<u32> {
        intraday_interval(resolution).map(|(_, days)| days)
    }

    async fn intraday(
        &self,
        symbol: &str,
        resolution: &str,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<IntradayBar>, ProviderError> {
        let Some((interval, _)) = intraday_interval(resolution) else {
            return Err(ProviderError::Malformed(format!(
                "no intraday bars at {resolution:?}; the intervals are 1m, 5m, and 1h"
            )));
        };
        let path = format!("/api/intraday/{symbol}");
        let from = from.timestamp().to_string();
        let to = to.timestamp().to_string();
        let rows: Vec<IntradayRow> = self
            .get_json(
                &path,
                &self.token,
                &[
                    ("interval", interval),
                    ("from", from.as_str()),
                    ("to", to.as_str()),
                ],
            )
            .await?;
        Ok(intraday_bars_of(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn the_base_url_loses_its_trailing_slash_and_public_points_at_eodhd() {
        assert_eq!(
            Eodhd::new("http://127.0.0.1:1/", "t").base_url(),
            "http://127.0.0.1:1"
        );
        assert_eq!(Eodhd::public("t").base_url(), DEFAULT_BASE_URL);
    }

    #[test]
    fn statuses_map_to_the_three_errors() {
        let t = "secret";
        assert_eq!(classify(StatusCode::OK, "", t), None);
        assert_eq!(
            classify(
                StatusCode::UNAUTHORIZED,
                r#"{"message":"Unauthenticated"}"#,
                t
            ),
            Some(ProviderError::CredentialsRejected("Unauthenticated".into()))
        );
        assert!(matches!(
            classify(StatusCode::FORBIDDEN, "Forbidden", t),
            Some(ProviderError::CredentialsRejected(m)) if m == "Forbidden"
        ));
        assert!(matches!(
            classify(StatusCode::SERVICE_UNAVAILABLE, "", t),
            Some(ProviderError::Unreachable(m)) if m == "HTTP 503 Service Unavailable: Service Unavailable"
        ));
        assert!(matches!(
            classify(StatusCode::TOO_MANY_REQUESTS, "slow down", t),
            Some(ProviderError::Unreachable(m)) if m.contains("429") && m.contains("slow down")
        ));
        assert!(matches!(
            classify(StatusCode::NOT_FOUND, "", t),
            Some(ProviderError::Malformed(m)) if m.starts_with("HTTP 404")
        ));
    }

    #[test]
    fn messages_are_scrubbed_of_the_token_and_kept_short() {
        let t = "secret";
        let echoed = format!("bad request for ?api_token={t}&fmt=json");
        assert!(matches!(
            classify(StatusCode::BAD_REQUEST, &echoed, t),
            Some(ProviderError::Malformed(m)) if !m.contains(t) && m.contains("[token]")
        ));
        let long = "x".repeat(500);
        assert_eq!(message_of(&long, StatusCode::BAD_GATEWAY).len(), 120);
        assert_eq!(message_of("  ", StatusCode::BAD_GATEWAY), "Bad Gateway");
        assert_eq!(scrub("nothing", ""), "nothing");
    }

    #[test]
    fn intraday_intervals_carry_the_providers_window_and_daily_has_none() {
        assert_eq!(intraday_interval("1m"), Some(("1m", 120)));
        assert_eq!(intraday_interval("5m"), Some(("5m", 600)));
        assert_eq!(intraday_interval("1h"), Some(("1h", 7200)));
        assert_eq!(intraday_interval("daily"), None);
        assert_eq!(intraday_interval("15m"), None);
        let adapter = Eodhd::new("http://127.0.0.1:1", "t");
        assert_eq!(adapter.intraday_window_days("5m"), Some(600));
        assert_eq!(adapter.intraday_window_days("1m"), Some(120));
        assert_eq!(adapter.intraday_window_days("daily"), None);
    }

    #[test]
    fn intraday_rows_drop_a_missing_timestamp_or_price_and_read_a_missing_volume_as_zero() {
        let rows: Vec<IntradayRow> = serde_json::from_str(
            r#"[{"timestamp":1704205800,"gmtoffset":0,"datetime":"2024-01-02 14:30:00","open":277.4155,"high":277.6767,"low":277.3743,"close":277.6451,"volume":11881},
                {"timestamp":null,"gmtoffset":0,"datetime":"2024-01-02 14:35:00","open":1,"high":2,"low":0.5,"close":1.5,"volume":100},
                {"timestamp":1704206400,"gmtoffset":0,"datetime":"2024-01-02 14:40:00","open":1,"high":2,"low":0.5,"close":null,"volume":100},
                {"timestamp":1704206700,"open":1,"high":2,"low":0.5,"close":1.5}]"#,
        )
        .unwrap();
        let bars = intraday_bars_of(rows);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].timestamp, 1_704_205_800);
        assert_eq!((bars[0].gmtoffset, bars[0].volume), (0, 11_881.0));
        assert_eq!(bars[0].close, 277.6451);
        assert_eq!(bars[1].timestamp, 1_704_206_700);
        assert_eq!((bars[1].gmtoffset, bars[1].volume), (0, 0.0));
    }

    #[test]
    fn one_minute_history_is_the_us_forex_and_crypto_exchanges() {
        assert_eq!(resolutions_for("US"), ["daily", "1h", "5m", "1m"]);
        assert_eq!(resolutions_for("FOREX"), ["daily", "1h", "5m", "1m"]);
        assert_eq!(resolutions_for("CC"), ["daily", "1h", "5m", "1m"]);
        assert_eq!(resolutions_for("LSE"), ["daily", "1h", "5m"]);
    }

    #[test]
    fn the_account_resets_at_midnight_utc_after_the_counted_day() {
        let user: UserResponse = serde_json::from_str(
            r#"{"subscriptionType":"Free","apiRequests":3,"apiRequestsDate":"2026-12-31","dailyRateLimit":20,"email":"x"}"#,
        )
        .unwrap();
        let account = account_of(user).unwrap();
        assert_eq!(account.plan, "Free");
        assert_eq!((account.requests_today, account.daily_limit), (3, 20));
        assert_eq!(
            account.resets_at,
            Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap()
        );

        let user: UserResponse = serde_json::from_str(
            r#"{"subscriptionType":"Free","apiRequests":3,"apiRequestsDate":"yesterday","dailyRateLimit":20}"#,
        )
        .unwrap();
        assert!(
            matches!(account_of(user), Err(ProviderError::Malformed(m)) if m.contains("yesterday"))
        );
    }

    #[test]
    fn rows_become_records_with_missing_optional_fields_empty() {
        let row: ExchangeRow =
            serde_json::from_str(r#"{"Code":"US","Name":"USA Stocks"}"#).unwrap();
        let exchange = Exchange::from(row);
        assert_eq!(
            (exchange.code.as_str(), exchange.country.as_str()),
            ("US", "")
        );
        assert_eq!(exchange.resolutions.len(), 4);

        let row: ListingRow =
            serde_json::from_str(r#"{"Code":"SPY","Type":"ETF","Currency":"USD","Isin":null}"#)
                .unwrap();
        let listing = Listing::from(row);
        assert_eq!(
            (listing.code.as_str(), listing.kind.as_str()),
            ("SPY", "ETF")
        );
        assert_eq!(
            (listing.name.as_str(), listing.currency.as_str()),
            ("", "USD")
        );
        assert_eq!((listing.country.as_str(), listing.venue.as_str()), ("", ""));
        let row: ListingRow = serde_json::from_str(
            r#"{"Code":"AAPL","Country":"USA","Exchange":"NASDAQ","Type":"Common Stock"}"#,
        )
        .unwrap();
        let listing = Listing::from(row);
        assert_eq!(
            (listing.country.as_str(), listing.venue.as_str()),
            ("USA", "NASDAQ")
        );
        assert!(serde_json::from_str::<ListingRow>(r#"{"Name":"no code"}"#).is_err());
    }

    #[test]
    fn bars_keep_their_order_drop_rows_missing_a_price_and_read_a_missing_volume_as_zero() {
        let rows: Vec<BarRow> = serde_json::from_str(
            r#"[{"date":"2026-09-10","open":1,"high":2,"low":0.5,"close":1.5,"adjusted_close":1.5,"volume":100},
                {"date":"2026-09-11","open":1,"high":2,"low":0.5,"close":null,"adjusted_close":1.5,"volume":100},
                {"date":"not a date","open":1,"high":2,"low":0.5,"close":1.5,"adjusted_close":1.5,"volume":100},
                {"date":"2026-09-14","open":1,"high":2,"low":0.5,"close":1.5,"adjusted_close":1.4}]"#,
        )
        .unwrap();
        let bars = bars_of(rows);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].date, NaiveDate::from_ymd_opt(2026, 9, 10).unwrap());
        assert_eq!(bars[0].volume, 100.0);
        assert_eq!(bars[1].date, NaiveDate::from_ymd_opt(2026, 9, 14).unwrap());
        assert_eq!((bars[1].adjusted_close, bars[1].volume), (1.4, 0.0));

        let rows: Vec<BulkRow> = serde_json::from_str(
            r#"[{"code":"SPY","exchange_short_name":"US","date":"2026-09-11","open":1,"high":2,"low":0.5,"close":1.5,"adjusted_close":1.5,"volume":7},
                {"code":"BAD","exchange_short_name":"US","date":"2026-09-11","open":null,"high":2,"low":0.5,"close":1.5,"adjusted_close":1.5,"volume":7}]"#,
        )
        .unwrap();
        let bulk = bulk_bars_of(rows);
        assert_eq!(bulk.len(), 1);
        assert_eq!(
            (bulk[0].code.as_str(), bulk[0].exchange.as_str()),
            ("SPY", "US")
        );
        assert_eq!(bulk[0].bar.volume, 7.0);
    }

    #[test]
    fn splits_parse_their_date_and_a_bad_one_is_malformed() {
        let rows: Vec<SplitRow> = serde_json::from_str(
            r#"[{"code":"AAPL","exchange":"US","date":"2026-09-11","split":"4.000000/1.000000"}]"#,
        )
        .unwrap();
        let splits = splits_of(rows).unwrap();
        assert_eq!(splits[0].code, "AAPL");
        assert_eq!(
            splits[0].date,
            NaiveDate::from_ymd_opt(2026, 9, 11).unwrap()
        );
        assert_eq!(splits[0].ratio, "4.000000/1.000000");
        let rows: Vec<SplitRow> =
            serde_json::from_str(r#"[{"code":"AAPL","date":"soon","split":"2/1"}]"#).unwrap();
        assert!(matches!(
            splits_of(rows),
            Err(ProviderError::Malformed(m)) if m.contains("soon") && m.contains("AAPL")
        ));
    }
}
