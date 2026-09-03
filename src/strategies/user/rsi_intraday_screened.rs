//! RSI Intraday (Screened) — the bundled example of a screened-universe strategy.
//!
//! The daily pass is a liquidity and price screen: a symbol qualifies for the next session
//! only when its close is at least the minimum price and its trailing average dollar volume
//! is at least the minimum. The engine then loads intraday bars only for those symbol-days,
//! so the strategy can run across every US stock without holding the whole intraday library
//! in memory. Intraday, it is the plain RSI mean reversion: buy when RSI on the intraday
//! bars drops below the oversold level, exit when it climbs above the overbought level, and
//! always be flat before the close.

use crate::sdk::prelude::*;

pub struct RsiIntradayScreened {
    // Daily screen state.
    adv: RollingMean,
    minimum_price: f64,
    minimum_adv: f64,
    // Intraday state, reset every session.
    rsi: Rsi,
    length: usize,
    oversold: f64,
    overbought: f64,
    stop_percent: f64,
    exit_time: NaiveTime,
    last_entry_time: NaiveTime,
}

impl Strategy for RsiIntradayScreened {
    fn manifest() -> Manifest {
        Manifest::new("rsi_intraday_screened", "RSI Intraday (Screened)", "v1")
            .describe("Screens the daily universe for price and dollar volume, then trades intraday RSI mean reversion on the qualifying symbols, flat by the close.")
            .rule("Daily screen: adjusted close at or above the minimum price and trailing average dollar volume (raw close x raw volume) at or above the minimum; only qualifying symbols load intraday bars for the next session.")
            .rule("Intraday: Wilder RSI over the configured number of bars, restarted each session from that session's bars.")
            .rule("When flat, RSI < oversold, and the bar is before the last entry time, buy at the next bar open with the platform position size.")
            .rule("When long and RSI > overbought, exit at the next bar open. Any surviving position exits at the configured time, so nothing is held overnight.")
            .rule("An optional protective stop sits a fixed percent below the entry price.")
            .asset_scope("US common stocks")
            .screened_universe()
            .run_defaults(&["universe:stocks"], "5m")
            .entry_limits(20, "priority", 0)
            .param(Param::decimal("minimum_price", 1.0).range(0.0, 10_000.0).unit("$").help("Daily close must be at least this to qualify"))
            .param(Param::decimal("minimum_average_dollar_volume", 10_000_000.0).range(0.0, 1.0e11).unit("$").help("Trailing average of raw close x volume"))
            .param(Param::int("average_dollar_volume_period", 20).range(2.0, 250.0).unit("sessions").advanced())
            .param(Param::int("length", 14).range(2.0, 200.0).unit("bars").help("Intraday RSI lookback"))
            .param(Param::decimal("oversold", 20.0).range(1.0, 50.0).help("Enter below this RSI"))
            .param(Param::decimal("overbought", 70.0).range(50.0, 99.0).help("Exit above this RSI"))
            .param(Param::int("last_entry_minute_of_day", 900).range(570.0, 955.0).unit("min").help("No new entries after this many minutes past midnight ET (900 = 15:00)").advanced())
            .param(Param::int("exit_minute_of_day", 955).range(575.0, 960.0).unit("min").help("Flatten at this many minutes past midnight ET (955 = 15:55)").advanced())
            .param(
                Param::decimal("stop_percent", 0.0)
                    .range(0.0, 50.0)
                    .unit("%")
                    .help("Protective stop below entry; 0 disables")
                    .advanced(),
            )
    }

    fn new(params: &Params, _symbol: &str) -> Result<Self> {
        let oversold = params.decimal("oversold")?;
        let overbought = params.decimal("overbought")?;
        ensure!(oversold < overbought, "oversold must be below overbought");
        let last_entry = params.int("last_entry_minute_of_day")?;
        let exit = params.int("exit_minute_of_day")?;
        ensure!(
            last_entry < exit,
            "last_entry_minute_of_day must be before exit_minute_of_day"
        );
        let length = params.usize("length")?;
        Ok(Self {
            adv: RollingMean::new(params.usize("average_dollar_volume_period")?),
            minimum_price: params.decimal("minimum_price")?,
            minimum_adv: params.decimal("minimum_average_dollar_volume")?,
            rsi: Rsi::new(length),
            length,
            oversold,
            overbought,
            stop_percent: params.decimal("stop_percent")? / 100.0,
            exit_time: minute_of_day(exit)?,
            last_entry_time: minute_of_day(last_entry)?,
        })
    }

    /// Daily pass: liquidity and price screen. Dollar volume uses raw close x raw volume so
    /// splits do not fabricate liquidity; the price test uses the adjusted close.
    fn screen(&mut self, ctx: &mut Ctx, bar: &Bar) -> Result<bool> {
        let adv = self.adv.update(bar.raw_close() * bar.volume);
        if ctx.next_session().is_none() {
            return Ok(false);
        }
        let Some(adv) = adv else {
            return Ok(false);
        };
        Ok(bar.close >= self.minimum_price && adv >= self.minimum_adv)
    }

    fn on_session_start(&mut self, _ctx: &mut Ctx) -> Result<()> {
        // Intraday RSI is a within-session indicator here: a symbol may qualify on
        // non-consecutive days, so state never carries across a gap.
        self.rsi = Rsi::new(self.length);
        Ok(())
    }

    fn on_bar(&mut self, ctx: &mut Ctx, bar: &Bar) -> Result<()> {
        let Some(rsi) = self.rsi.update(bar.close) else {
            return Ok(());
        };
        if ctx.is_flat() {
            if rsi < self.oversold && bar.time < self.last_entry_time {
                let stop = (self.stop_percent > 0.0).then(|| bar.close * (1.0 - self.stop_percent));
                ctx.tag("rsi", format!("{rsi:.1}"));
                ctx.buy_with(Size::Default, Exec::NextBarOpen, stop, None);
            }
        } else if ctx.is_long() && rsi > self.overbought {
            ctx.close("rsi_overbought");
        }
        Ok(())
    }

    fn on_fill(&mut self, ctx: &mut Ctx, fill: &Fill) -> Result<()> {
        if matches!(fill, Fill::Opened(_)) {
            ctx.exit_at(self.exit_time);
        }
        Ok(())
    }
}

fn minute_of_day(minutes: i64) -> Result<NaiveTime> {
    ensure!((0..1440).contains(&minutes), "minute of day out of range");
    NaiveTime::from_hms_opt((minutes / 60) as u32, (minutes % 60) as u32, 0)
        .ok_or_else(|| anyhow::anyhow!("invalid minute of day"))
}

pub fn entry() -> StrategyEntry {
    StrategyEntry::of::<RsiIntradayScreened>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(overrides: &[(&str, serde_json::Value)]) -> Result<RsiIntradayScreened> {
        let manifest = RsiIntradayScreened::manifest();
        manifest.validate().unwrap();
        let mut supplied = serde_json::Map::new();
        for (key, value) in overrides {
            supplied.insert((*key).to_owned(), value.clone());
        }
        RsiIntradayScreened::new(&manifest.resolve(&supplied).unwrap(), "TEST.US")
    }

    #[test]
    fn manifest_is_a_screened_intraday_strategy_with_liquidity_defaults() {
        let manifest = RsiIntradayScreened::manifest();
        assert!(manifest.screen_universe);
        assert_eq!(manifest.default_resolution.as_deref(), Some("5m"));
        let strategy = build(&[]).unwrap();
        assert_eq!(strategy.minimum_price, 1.0);
        assert_eq!(strategy.minimum_adv, 10_000_000.0);
        assert_eq!(strategy.exit_time, NaiveTime::from_hms_opt(15, 55, 0).unwrap());
        assert_eq!(strategy.last_entry_time, NaiveTime::from_hms_opt(15, 0, 0).unwrap());
    }

    #[test]
    fn rejects_entries_after_the_flatten_time() {
        assert!(build(&[
            ("last_entry_minute_of_day", serde_json::json!(950)),
            ("exit_minute_of_day", serde_json::json!(940)),
        ])
        .is_err());
        assert!(build(&[
            ("oversold", serde_json::json!(50)),
            ("overbought", serde_json::json!(50)),
        ])
        .is_err());
    }

    #[test]
    fn screen_requires_price_and_dollar_volume_history() {
        let mut strategy = build(&[("average_dollar_volume_period", serde_json::json!(2))]).unwrap();
        // The screen's own arithmetic: average dollar volume of raw close x volume.
        let a = strategy.adv.update(2.0 * 3_000_000.0);
        assert!(a.is_none(), "one session is not enough history");
        let b = strategy.adv.update(2.0 * 9_000_000.0).unwrap();
        assert!((b - 12_000_000.0).abs() < 1e-6);
        assert!(b >= strategy.minimum_adv);
        assert!(0.5 < strategy.minimum_price, "a $0.50 close would be screened out");
    }
}
