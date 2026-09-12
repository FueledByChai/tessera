//! The call budget every provider job obeys (decision 0022).
//!
//! A [`CallBudget`] is the provider's daily limit, what the account has used today, and the
//! reserve the source keeps back for the console's own reads. A job estimates its calls
//! before it starts ([`Estimate`]: the mandatory increment and the optional backfill),
//! asks [`CallBudget::can_start`] with the mandatory part, and is refused with both numbers
//! when that part exceeds what remains above the reserve; it charges every call it makes and
//! stops the optional part as soon as [`CallBudget::at_reserve`] says the reserve is reached.
//! The numbers come from the provider's own usage report, refreshed by the service before a
//! job starts and after it ends; the budget itself never talks to a provider.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The default share of the daily limit a source keeps back, in percent.
pub const DEFAULT_RESERVE_PCT: f64 = 5.0;

/// Requests an intraday window costs at EODHD (decision 0022).
pub const INTRADAY_CALL_COST: u64 = 5;

/// A day's request budget for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallBudget {
    /// The account's daily request limit.
    pub limit: u64,
    /// Requests already counted against it today.
    pub used: u64,
    /// Requests kept back from jobs, in calls (not percent).
    pub reserve: u64,
}

/// A job's estimate of its calls: the part it must make to do its work at all, and the part
/// it may stop early (backfill), resumed on the next run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Estimate {
    pub mandatory: u64,
    pub optional: u64,
}

impl Estimate {
    /// An EOD job: one bulk call per session plus one splits call per session are mandatory;
    /// one history call per symbol to backfill is optional.
    pub fn eod(sessions: u64, backfills: u64) -> Self {
        Estimate {
            mandatory: sessions.saturating_mul(2),
            optional: backfills,
        }
    }

    /// An intraday job: every window for every symbol, each at the intraday cost; all of it
    /// mandatory, since a window left out is a gap in the file.
    pub fn intraday(windows: u64, symbols: u64) -> Self {
        Estimate {
            mandatory: windows
                .saturating_mul(symbols)
                .saturating_mul(INTRADAY_CALL_COST),
            optional: 0,
        }
    }

    /// The intraday job as it runs (decision 0022): the windows that extend the files that
    /// exist are mandatory, the windows that backfill the symbols without one are optional,
    /// each at the intraday cost.
    pub fn intraday_windows(increment_windows: u64, backfill_windows: u64) -> Self {
        Estimate {
            mandatory: increment_windows.saturating_mul(INTRADAY_CALL_COST),
            optional: backfill_windows.saturating_mul(INTRADAY_CALL_COST),
        }
    }

    pub fn total(&self) -> u64 {
        self.mandatory.saturating_add(self.optional)
    }
}

/// Why a job did not start: it needed more calls than the budget had above its reserve.
/// The text says both numbers, as the job record and the console must.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub needed: u64,
    pub available: u64,
    pub reserve: u64,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "needs {} calls, {} available above the reserve of {}",
            self.needed, self.available, self.reserve
        )
    }
}

impl std::error::Error for Refused {}

impl CallBudget {
    /// A budget with the reserve in calls.
    pub fn new(limit: u64, used: u64, reserve: u64) -> Self {
        CallBudget {
            limit,
            used,
            reserve: reserve.min(limit),
        }
    }

    /// A budget whose reserve is `reserve_pct` percent of the limit, rounded up so a
    /// non-zero percentage of a small limit still keeps a call back. A percentage that is
    /// not a number or negative keeps nothing; one over 100 keeps the whole limit.
    pub fn with_reserve_pct(limit: u64, used: u64, reserve_pct: f64) -> Self {
        Self::new(limit, used, reserve_calls(limit, reserve_pct))
    }

    /// Requests left before the provider refuses.
    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }

    /// Requests a job may still spend: what remains above the reserve.
    pub fn available(&self) -> u64 {
        self.remaining().saturating_sub(self.reserve)
    }

    /// Whether a job needing `mandatory_calls` may start: yes when they fit in what is
    /// available above the reserve, exactly filling it included.
    pub fn can_start(&self, mandatory_calls: u64) -> Result<(), Refused> {
        let available = self.available();
        if mandatory_calls <= available {
            Ok(())
        } else {
            Err(Refused {
                needed: mandatory_calls,
                available,
                reserve: self.reserve,
            })
        }
    }

    /// Counts `calls` made; saturates rather than wrapping when a job overshoots the limit.
    pub fn charge(&mut self, calls: u64) {
        self.used = self.used.saturating_add(calls);
    }

    /// Whether the reserve is reached: nothing is left above it, so the optional part of a
    /// job stops here.
    pub fn at_reserve(&self) -> bool {
        self.available() == 0
    }
}

/// `reserve_pct` percent of `limit`, in calls, rounded up and capped at the limit.
pub fn reserve_calls(limit: u64, reserve_pct: f64) -> u64 {
    if !reserve_pct.is_finite() || reserve_pct <= 0.0 {
        return 0;
    }
    let share = (limit as f64 * reserve_pct / 100.0).ceil();
    if share >= limit as f64 {
        limit
    } else {
        share as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserve_is_a_rounded_up_share_capped_at_the_limit() {
        assert_eq!(reserve_calls(100_000, 5.0), 5_000);
        assert_eq!(reserve_calls(20, 5.0), 1);
        assert_eq!(reserve_calls(20, 0.0), 0);
        assert_eq!(reserve_calls(20, -3.0), 0);
        assert_eq!(reserve_calls(20, f64::NAN), 0);
        assert_eq!(reserve_calls(20, 100.0), 20);
        assert_eq!(reserve_calls(20, 250.0), 20);
        assert_eq!(reserve_calls(0, 5.0), 0);
        assert_eq!(
            CallBudget::with_reserve_pct(100_000, 1_234, DEFAULT_RESERVE_PCT),
            CallBudget::new(100_000, 1_234, 5_000)
        );
        assert_eq!(CallBudget::new(10, 0, 50).reserve, 10);
    }

    #[test]
    fn can_start_admits_a_job_that_exactly_fills_the_available_calls_and_refuses_one_more() {
        let budget = CallBudget::new(1_000, 400, 50);
        assert_eq!((budget.remaining(), budget.available()), (600, 550));
        assert_eq!(budget.can_start(0), Ok(()));
        assert_eq!(budget.can_start(550), Ok(()));
        let refused = budget.can_start(551).unwrap_err();
        assert_eq!(
            refused,
            Refused {
                needed: 551,
                available: 550,
                reserve: 50
            }
        );
        assert_eq!(
            refused.to_string(),
            "needs 551 calls, 550 available above the reserve of 50"
        );
        let boxed: Box<dyn std::error::Error> = Box::new(refused);
        assert!(boxed.to_string().contains("551"));
    }

    #[test]
    fn charging_past_the_reserve_reaches_it_and_never_wraps() {
        let mut budget = CallBudget::new(100, 90, 5);
        assert!(!budget.at_reserve());
        assert_eq!(budget.can_start(5), Ok(()));
        budget.charge(4);
        assert!(!budget.at_reserve(), "one call is still available");
        assert_eq!(budget.available(), 1);
        budget.charge(1);
        assert!(budget.at_reserve(), "the reserve is reached exactly");
        assert_eq!(budget.can_start(1).unwrap_err().available, 0);
        budget.charge(20);
        assert!(budget.at_reserve());
        assert_eq!(
            (budget.used, budget.remaining(), budget.available()),
            (115, 0, 0)
        );
        budget.charge(u64::MAX);
        assert_eq!(budget.used, u64::MAX);
        assert_eq!(budget.remaining(), 0);
    }

    #[test]
    fn a_limit_of_zero_admits_no_call_and_is_already_at_its_reserve() {
        let mut budget = CallBudget::with_reserve_pct(0, 0, DEFAULT_RESERVE_PCT);
        assert_eq!(budget.reserve, 0);
        assert!(budget.at_reserve());
        assert_eq!(budget.can_start(0), Ok(()));
        assert_eq!(
            budget.can_start(1),
            Err(Refused {
                needed: 1,
                available: 0,
                reserve: 0
            })
        );
        budget.charge(3);
        assert_eq!((budget.used, budget.remaining()), (3, 0));
        assert!(budget.at_reserve());
        // Usage past the limit (a provider reports more than the plan allows) is not negative.
        assert_eq!(CallBudget::new(10, 12, 1).available(), 0);
    }

    #[test]
    fn estimates_follow_the_call_rules() {
        let eod = Estimate::eod(3, 40);
        assert_eq!((eod.mandatory, eod.optional, eod.total()), (6, 40, 46));
        let intraday = Estimate::intraday(4, 25);
        assert_eq!((intraday.mandatory, intraday.optional), (500, 0));
        let windows = Estimate::intraday_windows(3, 2);
        assert_eq!(
            (windows.mandatory, windows.optional, windows.total()),
            (15, 10, 25)
        );
        assert_eq!(Estimate::intraday_windows(u64::MAX, 0).mandatory, u64::MAX);
        assert_eq!(Estimate::eod(u64::MAX, 1).mandatory, u64::MAX);
        assert_eq!(Estimate::eod(u64::MAX, 1).total(), u64::MAX);
        let budget = CallBudget::new(100_000, 99_600, 5_000);
        assert!(budget.can_start(eod.mandatory).is_err());
        assert!(
            CallBudget::new(100_000, 0, 5_000)
                .can_start(eod.mandatory)
                .is_ok()
        );
    }

    #[test]
    fn budgets_round_trip_through_serde() {
        let budget = CallBudget::new(100, 7, 5);
        let text = serde_json::to_string(&budget).unwrap();
        assert_eq!(text, r#"{"limit":100,"used":7,"reserve":5}"#);
        assert_eq!(serde_json::from_str::<CallBudget>(&text).unwrap(), budget);
        let refused = budget.can_start(200).unwrap_err();
        let text = serde_json::to_string(&refused).unwrap();
        assert_eq!(serde_json::from_str::<Refused>(&text).unwrap(), refused);
    }
}
