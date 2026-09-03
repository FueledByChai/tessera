//! Moving Average Cross — the second bundled example strategy.
//!
//! Buy when the fast simple moving average crosses above the slow one, exit when it
//! crosses back below. Works on any bar resolution the run form selects.

use crate::sdk::prelude::*;

pub struct MovingAverageCross {
    fast: Sma,
    slow: Sma,
    cross: Crossover,
}

impl Strategy for MovingAverageCross {
    fn manifest() -> Manifest {
        Manifest::new("moving_average_cross", "Moving Average Cross", "v1")
            .describe("Goes long when the fast simple moving average crosses above the slow one and exits when it crosses back below.")
            .rule("Compute fast and slow simple moving averages from completed closes only.")
            .rule("After the fast SMA crosses above the slow SMA, buy at the next bar open using the platform position size.")
            .rule("After the fast SMA crosses below the slow SMA, exit at the next bar open.")
            .asset_scope("Any")
            .warmup_bars(250)
            .run_defaults(&["DEMO.US"], "daily")
            .param(Param::int("fast_length", 20).range(1.0, 500.0).unit("bars"))
            .param(Param::int("slow_length", 100).range(2.0, 1000.0).unit("bars"))
    }

    fn new(params: &Params, _symbol: &str) -> Result<Self> {
        let fast = params.usize("fast_length")?;
        let slow = params.usize("slow_length")?;
        ensure!(fast < slow, "fast_length must be below slow_length");
        Ok(Self {
            fast: Sma::new(fast),
            slow: Sma::new(slow),
            cross: Crossover::new(),
        })
    }

    fn on_bar(&mut self, ctx: &mut Ctx, bar: &Bar) -> Result<()> {
        let (Some(fast), Some(slow)) = (self.fast.update(bar.close), self.slow.update(bar.close))
        else {
            return Ok(());
        };
        match self.cross.update(fast, slow) {
            Some(true) if ctx.is_flat() => ctx.buy(Size::Default),
            Some(false) if ctx.is_long() => ctx.close("fast_below_slow"),
            _ => {}
        }
        Ok(())
    }
}

pub fn entry() -> StrategyEntry {
    StrategyEntry::of::<MovingAverageCross>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_inverted_lengths() {
        let manifest = MovingAverageCross::manifest();
        manifest.validate().unwrap();
        let mut supplied = serde_json::Map::new();
        supplied.insert("fast_length".into(), serde_json::json!(50));
        supplied.insert("slow_length".into(), serde_json::json!(20));
        let params = manifest.resolve(&supplied).unwrap();
        assert!(MovingAverageCross::new(&params, "DEMO.US").is_err());
    }
}
