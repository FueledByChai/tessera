//! The local date at an exchange (DS-15).
//!
//! A download job fetches sessions through a date, and that date is a *local* one: Tokyo's
//! Tuesday is Tuesday in Tokyo, which starts while it is still Monday in New York. The job
//! used to be given New York's date for every exchange, so an Asian exchange's newest
//! session — already closed and already published — was not asked for until New York caught
//! up, a day later.
//!
//! Nothing at the provider says what time it is at an exchange: the exchanges list gives a
//! country and nothing else. So the country is mapped to an IANA timezone here, and a
//! country the map does not know falls back to New York, which is what every exchange got
//! before. The map is data, not a guess about the venue: where a country's exchanges sit in
//! one timezone it is right, and where they do not (the USA, Canada, Australia) it names the
//! one the country's main exchange keeps time by.

use chrono::{DateTime, NaiveDate, Utc};

/// The timezone an exchange with no country, or an unknown one, is read in: where the
/// console's own calendar day is counted.
pub const NEW_YORK: &str = "America/New_York";

/// The IANA timezone of a country's main exchanges, in the names the provider's countries
/// come as (`USA`, `Japan`); `None` for a country the map does not carry, whose exchanges
/// then keep New York's date.
pub fn timezone_for_country(country: &str) -> Option<&'static str> {
    let country = country.trim().to_lowercase();
    let timezone = match country.as_str() {
        "usa" | "united states" | "united states of america" => "America/New_York",
        "canada" => "America/Toronto",
        "mexico" => "America/Mexico_City",
        "brazil" => "America/Sao_Paulo",
        "argentina" => "America/Argentina/Buenos_Aires",
        "chile" => "America/Santiago",
        "colombia" => "America/Bogota",
        "peru" => "America/Lima",

        "uk" | "united kingdom" | "great britain" | "england" => "Europe/London",
        "ireland" => "Europe/Dublin",
        "iceland" => "Atlantic/Reykjavik",
        "germany" => "Europe/Berlin",
        "france" => "Europe/Paris",
        "netherlands" => "Europe/Amsterdam",
        "belgium" => "Europe/Brussels",
        "luxembourg" => "Europe/Luxembourg",
        "switzerland" => "Europe/Zurich",
        "austria" => "Europe/Vienna",
        "spain" => "Europe/Madrid",
        "portugal" => "Europe/Lisbon",
        "italy" => "Europe/Rome",
        "greece" => "Europe/Athens",
        "sweden" => "Europe/Stockholm",
        "norway" => "Europe/Oslo",
        "denmark" => "Europe/Copenhagen",
        "finland" => "Europe/Helsinki",
        "poland" => "Europe/Warsaw",
        "czech republic" | "czechia" => "Europe/Prague",
        "hungary" => "Europe/Budapest",
        "romania" => "Europe/Bucharest",
        "bulgaria" => "Europe/Sofia",
        "croatia" => "Europe/Zagreb",
        "slovenia" => "Europe/Ljubljana",
        "slovakia" => "Europe/Bratislava",
        "estonia" => "Europe/Tallinn",
        "latvia" => "Europe/Riga",
        "lithuania" => "Europe/Vilnius",
        "ukraine" => "Europe/Kyiv",
        "russia" => "Europe/Moscow",
        "turkey" => "Europe/Istanbul",
        "cyprus" => "Asia/Nicosia",
        "malta" => "Europe/Malta",

        "israel" => "Asia/Jerusalem",
        "egypt" => "Africa/Cairo",
        "south africa" => "Africa/Johannesburg",
        "nigeria" => "Africa/Lagos",
        "kenya" => "Africa/Nairobi",
        "qatar" => "Asia/Qatar",
        "saudi arabia" => "Asia/Riyadh",
        "uae" | "united arab emirates" => "Asia/Dubai",

        "india" => "Asia/Kolkata",
        "pakistan" => "Asia/Karachi",
        "bangladesh" => "Asia/Dhaka",
        "sri lanka" => "Asia/Colombo",
        "kazakhstan" => "Asia/Almaty",

        "china" => "Asia/Shanghai",
        "hong kong" => "Asia/Hong_Kong",
        "taiwan" => "Asia/Taipei",
        "japan" => "Asia/Tokyo",
        "korea" | "south korea" => "Asia/Seoul",
        "singapore" => "Asia/Singapore",
        "malaysia" => "Asia/Kuala_Lumpur",
        "thailand" => "Asia/Bangkok",
        "indonesia" => "Asia/Jakarta",
        "philippines" => "Asia/Manila",
        "vietnam" => "Asia/Ho_Chi_Minh",

        "australia" => "Australia/Sydney",
        "new zealand" => "Pacific/Auckland",
        _ => return None,
    };
    Some(timezone)
}

/// The calendar date at an exchange at `now`: its own local date, and New York's when the
/// exchange has no timezone or one this build cannot read.
pub fn local_date(now: DateTime<Utc>, timezone: Option<&str>) -> NaiveDate {
    match timezone.unwrap_or(NEW_YORK).parse::<chrono_tz::Tz>() {
        Ok(zone) => now.with_timezone(&zone).date_naive(),
        Err(_) => now
            .with_timezone(&chrono_tz::America::New_York)
            .date_naive(),
    }
}

/// [`local_date`] now, for the exchange whose timezone is `timezone`.
pub fn today_in(timezone: Option<&str>) -> NaiveDate {
    local_date(Utc::now(), timezone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("a fixed instant")
            .with_timezone(&Utc)
    }

    /// DS-15: while it is still the previous evening in New York it is already the next
    /// morning in Tokyo, so the exchange's own date — not New York's — is the one a run
    /// fetches through.
    #[test]
    fn an_exchange_whose_day_is_ahead_sees_the_later_date() {
        // 22:30 on Monday the 14th in New York, 11:30 on Tuesday the 15th in Tokyo.
        let now = at("2026-09-15T02:30:00Z");
        assert_eq!(
            local_date(now, Some("America/New_York")),
            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
        );
        assert_eq!(
            local_date(now, Some("Asia/Tokyo")),
            NaiveDate::from_ymd_opt(2026, 9, 15).unwrap()
        );
        // An exchange with no timezone at all keeps the old answer.
        assert_eq!(
            local_date(now, None),
            NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
        );
        assert_eq!(
            local_date(now, Some("Nowhere/Nothing")),
            local_date(now, None)
        );
    }

    #[test]
    fn the_countries_the_console_serves_have_a_timezone() {
        assert_eq!(timezone_for_country("USA"), Some("America/New_York"));
        assert_eq!(timezone_for_country(" japan "), Some("Asia/Tokyo"));
        assert_eq!(timezone_for_country("UK"), Some("Europe/London"));
        assert_eq!(timezone_for_country("Germany"), Some("Europe/Berlin"));
        assert_eq!(timezone_for_country("Hong Kong"), Some("Asia/Hong_Kong"));
        assert_eq!(timezone_for_country("Australia"), Some("Australia/Sydney"));
        // A country the map does not carry is not guessed at: it keeps New York's day.
        assert_eq!(timezone_for_country(""), None);
        assert_eq!(timezone_for_country("Atlantis"), None);
        // Every name the map hands out is one this build can read.
        for country in [
            "USA",
            "Canada",
            "Brazil",
            "UK",
            "Ireland",
            "Iceland",
            "Germany",
            "France",
            "Netherlands",
            "Belgium",
            "Luxembourg",
            "Switzerland",
            "Austria",
            "Spain",
            "Portugal",
            "Italy",
            "Greece",
            "Sweden",
            "Norway",
            "Denmark",
            "Finland",
            "Poland",
            "Czech Republic",
            "Hungary",
            "Romania",
            "Bulgaria",
            "Croatia",
            "Slovenia",
            "Slovakia",
            "Estonia",
            "Latvia",
            "Lithuania",
            "Ukraine",
            "Russia",
            "Turkey",
            "Cyprus",
            "Malta",
            "Israel",
            "Egypt",
            "South Africa",
            "Nigeria",
            "Kenya",
            "Qatar",
            "Saudi Arabia",
            "UAE",
            "India",
            "Pakistan",
            "Bangladesh",
            "Sri Lanka",
            "Kazakhstan",
            "China",
            "Hong Kong",
            "Taiwan",
            "Japan",
            "South Korea",
            "Singapore",
            "Malaysia",
            "Thailand",
            "Indonesia",
            "Philippines",
            "Vietnam",
            "Australia",
            "New Zealand",
            "Mexico",
            "Argentina",
            "Chile",
            "Colombia",
            "Peru",
        ] {
            let timezone = timezone_for_country(country)
                .unwrap_or_else(|| panic!("{country} has no timezone"));
            assert!(
                timezone.parse::<chrono_tz::Tz>().is_ok(),
                "{country} maps to an unreadable timezone {timezone}"
            );
        }
    }
}
