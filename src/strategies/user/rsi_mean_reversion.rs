//! RSI Mean Reversion — the reference one-file SDK strategy.
//!
//! Buy when RSI drops below the oversold level, exit when it rises above the
//! overbought level. Everything outside this file (data, replay, fills, costs,
//! accounting, reports) is handled by the platform.

use crate::sdk::prelude::*;

pub struct RsiMeanReversion {
    rsi: Rsi,
    oversold: f64,
    overbought: f64,
    stop_percent: f64,
}

impl Strategy for RsiMeanReversion {
    fn manifest() -> Manifest {
        Manifest::new("rsi_mean_reversion", "RSI Mean Reversion", "v1")
            .describe("Buys when RSI falls below the oversold level and exits when it climbs above the overbought level.")
            .rule("Compute Wilder RSI over the configured length from completed closes only.")
            .rule("When flat and RSI < oversold, buy at the next bar open using the platform position size.")
            .rule("When long and RSI > overbought, exit at the next bar open.")
            .rule("An optional protective stop sits a fixed percent below the entry price.")
            .asset_scope("Any")
            .warmup_bars(200)
            .param(Param::int("length", 14).range(2.0, 200.0).unit("bars").help("RSI lookback"))
            .param(Param::decimal("oversold", 10.0).range(1.0, 50.0).help("Enter below this RSI"))
            .param(Param::decimal("overbought", 90.0).range(50.0, 99.0).help("Exit above this RSI"))
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
        Ok(Self {
            rsi: Rsi::new(params.usize("length")?),
            oversold,
            overbought,
            stop_percent: params.decimal("stop_percent")? / 100.0,
        })
    }

    fn on_bar(&mut self, ctx: &mut Ctx, bar: &Bar) -> Result<()> {
        let Some(rsi) = self.rsi.update(bar.close) else {
            return Ok(());
        };
        if ctx.is_flat() && rsi < self.oversold {
            let stop = (self.stop_percent > 0.0).then(|| bar.close * (1.0 - self.stop_percent));
            ctx.tag("rsi", format!("{rsi:.1}"));
            ctx.buy_with(Size::Default, Exec::NextBarOpen, stop, None);
        } else if ctx.is_long() && rsi > self.overbought {
            ctx.close("rsi_overbought");
        }
        Ok(())
    }
}

pub fn entry() -> StrategyEntry {
    StrategyEntry::of::<RsiMeanReversion>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_declares_the_expected_parameters() {
        let manifest = RsiMeanReversion::manifest();
        manifest.validate().unwrap();
        let names = manifest
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["length", "oversold", "overbought", "stop_percent"]);
    }

    #[test]
    fn rejects_inverted_thresholds() {
        let manifest = RsiMeanReversion::manifest();
        let mut supplied = serde_json::Map::new();
        supplied.insert("oversold".into(), serde_json::json!(45));
        supplied.insert("overbought".into(), serde_json::json!(55));
        let params = manifest.resolve(&supplied).unwrap();
        assert!(RsiMeanReversion::new(&params, "TEST.US").is_ok());
        supplied.insert("overbought".into(), serde_json::json!(50));
        supplied.insert("oversold".into(), serde_json::json!(50));
        let params = manifest.resolve(&supplied).unwrap();
        assert!(RsiMeanReversion::new(&params, "TEST.US").is_err());
    }
}
