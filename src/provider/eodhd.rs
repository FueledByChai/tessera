//! The EODHD adapter (decision 0020): the user, exchanges-list, and exchange-symbol-list
//! endpoints over reqwest with rustls.
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

use super::{Account, Exchange, Listing, Provider, ProviderError};

/// Where the public API lives; a stub server replaces it in tests.
pub const DEFAULT_BASE_URL: &str = "https://eodhd.com";

/// Bar resolutions, in the engine's names, that every EODHD exchange offers.
const COMMON_RESOLUTIONS: [&str; 3] = ["daily", "1h", "5m"];
/// Exchanges with 1-minute intraday history as well.
const ONE_MINUTE_EXCHANGES: [&str; 3] = ["US", "FOREX", "CC"];

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
}

impl From<ListingRow> for Listing {
    fn from(row: ListingRow) -> Self {
        Listing {
            code: row.code,
            name: row.name,
            kind: row.kind,
            currency: row.currency,
        }
    }
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
        assert!(serde_json::from_str::<ListingRow>(r#"{"Name":"no code"}"#).is_err());
    }
}
