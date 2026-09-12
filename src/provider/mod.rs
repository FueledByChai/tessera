//! Market-data providers behind one trait (decision 0020).
//!
//! A provider is a Rust module implementing [`Provider`]: verify a token and report the
//! account's usage and limits, list the exchanges it covers, list an exchange's
//! instruments, and fetch what the download jobs need: an exchange's bulk end-of-day bars
//! for one session, one symbol's daily history, and an exchange's splits for one session
//! (the intraday windows come with the intraday job). A second provider is another module
//! behind the same trait and nothing else. The first adapter is [`eodhd`]; [`budget`] is the
//! call budget every job checks against the usage a provider reports (decision 0022), and
//! [`jobs`] holds the jobs themselves, written against the trait alone.
//!
//! Every method fails with a [`ProviderError`] whose three variants are what a caller acts on:
//! the token is wrong (ask for another), the provider cannot be reached (try later), or the
//! provider answered something this adapter cannot read (a bug or an API change, report it).
//! An error's text never carries the token: it is the only thing the service will log.

pub mod budget;
pub mod eodhd;
pub mod jobs;

use std::fmt;
use std::future::Future;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// What a provider says about the account behind a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The provider's name for the subscription (EODHD: `subscriptionType`).
    pub plan: String,
    /// API requests counted against today's limit so far.
    pub requests_today: u64,
    /// The account's daily request limit.
    pub daily_limit: u64,
    /// When the counter next resets, in UTC.
    pub resets_at: DateTime<Utc>,
}

/// An exchange the provider covers and the bar resolutions it offers there, named as the
/// engine names them (`daily`, `1h`, `5m`, `1m`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exchange {
    pub code: String,
    pub name: String,
    pub country: String,
    pub resolutions: Vec<String>,
}

/// One instrument listed on an exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub code: String,
    pub name: String,
    /// The provider's classification (`Common Stock`, `ETF`, `Fund`, ...), kept verbatim:
    /// decision 0012 segments inventory by it.
    pub kind: String,
    pub currency: String,
    /// The listing's country as the provider names it (`USA`); empty when it says nothing.
    #[serde(default)]
    pub country: String,
    /// The venue the provider lists under the exchange (EODHD's `Exchange` field: `NASDAQ`,
    /// `NYSE ARCA` under `US`); empty when it says nothing. The regenerated `catalog.csv`
    /// carries it as its `Exchange` column.
    #[serde(default)]
    pub venue: String,
}

/// One daily bar as the engine's daily files carry it (`docs/DATA_SOURCES.md`): the raw
/// prints, the provider's adjusted close, and the volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bar {
    pub date: NaiveDate,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub adjusted_close: f64,
    /// Shares traded; a provider that reports none gives zero.
    pub volume: f64,
}

/// One row of an exchange's bulk end-of-day answer: whose bar it is and the bar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BulkBar {
    pub code: String,
    /// The exchange code the row belongs to, as the provider reports it.
    pub exchange: String,
    pub bar: Bar,
}

/// A split the provider reports for one session: the symbol and the ratio as the provider
/// writes it (EODHD: `4.000000/1.000000`). A job refetches the symbol's whole history after
/// one, so the ratio is informational.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Split {
    pub code: String,
    pub exchange: String,
    pub date: NaiveDate,
    pub ratio: String,
}

/// Why a provider call failed. The text is safe to log: no variant carries the token or a URL
/// that would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The provider refused the token (HTTP 401 or 403). Carries the provider's message.
    CredentialsRejected(String),
    /// The provider could not be reached or did not answer usefully (a connection failure,
    /// a timeout, a 5xx, a rate limit).
    Unreachable(String),
    /// The provider answered, but not in the shape this adapter reads (an unexpected status,
    /// a body that is not the expected JSON).
    Malformed(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderError::CredentialsRejected(m) => write!(f, "credentials rejected: {m}"),
            ProviderError::Unreachable(m) => write!(f, "provider unreachable: {m}"),
            ProviderError::Malformed(m) => write!(f, "malformed provider response: {m}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// A market-data provider's calls: the read-only ones the console shows, and the download
/// ones the jobs make.
///
/// `verify` takes the token to check so a caller can test a candidate before storing it; the
/// other calls use the token the adapter was built with. Futures are `Send` so a service task
/// can await them from any runtime thread. Each call is one request against the provider's
/// daily limit unless the adapter's documentation says otherwise.
pub trait Provider: Send + Sync {
    /// Checks `token` with the provider and reports the account behind it.
    fn verify(&self, token: &str) -> impl Future<Output = Result<Account, ProviderError>> + Send;

    /// The exchanges the provider covers, with the resolutions each offers.
    fn exchanges(&self) -> impl Future<Output = Result<Vec<Exchange>, ProviderError>> + Send;

    /// The instruments listed on `exchange`; `delisted` asks for the ones no longer trading
    /// instead of the active ones.
    fn symbols(
        &self,
        exchange: &str,
        delisted: bool,
    ) -> impl Future<Output = Result<Vec<Listing>, ProviderError>> + Send;

    /// Every instrument's bar on `exchange` for the session `date`; empty when the provider
    /// has no bars for that date (a weekend, a holiday, a session not published yet).
    fn bulk_eod(
        &self,
        exchange: &str,
        date: NaiveDate,
    ) -> impl Future<Output = Result<Vec<BulkBar>, ProviderError>> + Send;

    /// The daily history of `symbol` (the provider's full form, `AAPL.US`) from `from`
    /// through the latest session, oldest first, adjusted closes on one basis.
    fn eod_history(
        &self,
        symbol: &str,
        from: NaiveDate,
    ) -> impl Future<Output = Result<Vec<Bar>, ProviderError>> + Send;

    /// The splits on `exchange` effective on the session `date`.
    fn splits(
        &self,
        exchange: &str,
        date: NaiveDate,
    ) -> impl Future<Output = Result<Vec<Split>, ProviderError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_display_their_kind_and_message() {
        let cases = [
            (
                ProviderError::CredentialsRejected("Unauthenticated".into()),
                "credentials rejected: Unauthenticated",
            ),
            (
                ProviderError::Unreachable("HTTP 503".into()),
                "provider unreachable: HTTP 503",
            ),
            (
                ProviderError::Malformed("expected a list".into()),
                "malformed provider response: expected a list",
            ),
        ];
        for (err, text) in cases {
            assert_eq!(err.to_string(), text);
            let boxed: Box<dyn std::error::Error> = Box::new(err.clone());
            assert_eq!(boxed.to_string(), text);
        }
    }

    #[test]
    fn records_round_trip_through_serde() {
        let account = Account {
            plan: "Fundamentals".into(),
            requests_today: 7,
            daily_limit: 20,
            resets_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        };
        let text = serde_json::to_string(&account).unwrap();
        assert_eq!(serde_json::from_str::<Account>(&text).unwrap(), account);

        let exchange = Exchange {
            code: "US".into(),
            name: "USA Stocks".into(),
            country: "USA".into(),
            resolutions: vec!["daily".into(), "1m".into()],
        };
        let text = serde_json::to_string(&exchange).unwrap();
        assert_eq!(serde_json::from_str::<Exchange>(&text).unwrap(), exchange);

        let listing = Listing {
            code: "SPY".into(),
            name: "SPDR S&P 500 ETF Trust".into(),
            kind: "ETF".into(),
            currency: "USD".into(),
            country: "USA".into(),
            venue: "NYSE ARCA".into(),
        };
        let text = serde_json::to_string(&listing).unwrap();
        assert!(text.contains("\"kind\":\"ETF\""));
        assert_eq!(serde_json::from_str::<Listing>(&text).unwrap(), listing);
        // A listing recorded before the country and venue existed reads back with them empty.
        let old: Listing =
            serde_json::from_str(r#"{"code":"SPY","name":"SPDR","kind":"ETF","currency":"USD"}"#)
                .unwrap();
        assert_eq!((old.country.as_str(), old.venue.as_str()), ("", ""));

        let bar = Bar {
            date: NaiveDate::from_ymd_opt(2026, 9, 11).unwrap(),
            open: 1.0,
            high: 2.0,
            low: 0.5,
            close: 1.5,
            adjusted_close: 1.5,
            volume: 100.0,
        };
        let bulk = BulkBar {
            code: "SPY".into(),
            exchange: "US".into(),
            bar: bar.clone(),
        };
        let text = serde_json::to_string(&bulk).unwrap();
        assert_eq!(serde_json::from_str::<BulkBar>(&text).unwrap(), bulk);
        let split = Split {
            code: "AAPL".into(),
            exchange: "US".into(),
            date: bar.date,
            ratio: "4.000000/1.000000".into(),
        };
        let text = serde_json::to_string(&split).unwrap();
        assert!(text.contains("\"date\":\"2026-09-11\""));
        assert_eq!(serde_json::from_str::<Split>(&text).unwrap(), split);
    }
}
