//! Market-data providers behind one trait (decision 0020).
//!
//! A provider is a Rust module implementing [`Provider`]: verify a token and report the
//! account's usage and limits, list the exchanges it covers, and list an exchange's
//! instruments. The download calls the jobs need (bulk EOD, history, splits, intraday windows)
//! extend this trait when those tickets land; a second provider is another module behind the
//! same trait and nothing else. The first adapter is [`eodhd`]; [`budget`] is the call
//! budget every job checks against the usage a provider reports (decision 0022).
//!
//! Every method fails with a [`ProviderError`] whose three variants are what a caller acts on:
//! the token is wrong (ask for another), the provider cannot be reached (try later), or the
//! provider answered something this adapter cannot read (a bug or an API change, report it).
//! An error's text never carries the token: it is the only thing the service will log.

pub mod budget;
pub mod eodhd;

use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};
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

/// A market-data provider's read-only calls.
///
/// `verify` takes the token to check so a caller can test a candidate before storing it; the
/// other calls use the token the adapter was built with. Futures are `Send` so a service task
/// can await them from any runtime thread.
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
        };
        let text = serde_json::to_string(&listing).unwrap();
        assert!(text.contains("\"kind\":\"ETF\""));
        assert_eq!(serde_json::from_str::<Listing>(&text).unwrap(), listing);
    }
}
