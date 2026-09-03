//! __STRATEGY_NAME__
//!
//! One-file SDK strategy. Declare parameters in `manifest()`, build indicator
//! state in `new()`, and express rules in `on_bar()` through the context.
//! The platform owns data loading, replay, fills, costs, accounting, and reports.

use crate::sdk::prelude::*;

pub struct __STRUCT_NAME__ {
    // TODO: indicator state and resolved parameters, for example:
    // rsi: Rsi,
    // oversold: f64,
    fast: Sma,
    slow: Sma,
    cross: Crossover,
}

impl Strategy for __STRUCT_NAME__ {
    /// Parameters declared here appear on the run form automatically.
    fn manifest() -> Manifest {
        Manifest::new("__STRATEGY_ID__", "__STRATEGY_NAME__", "v1")
            .describe("TODO: one sentence describing the edge.")
            .rule("TODO: one line per rule, shown on the strategy page.")
            .asset_scope("Any")
            // Bars replayed before the requested start so indicators are warm.
            .warmup_bars(200)
            .param(Param::int("fast_length", 20).range(1.0, 500.0).unit("bars"))
            .param(Param::int("slow_length", 100).range(2.0, 1000.0).unit("bars"))
            // .param(Param::decimal("threshold", 30.0).range(0.0, 100.0).help("..."))
            // .param(Param::bool("long_only", true).advanced())
            // .param(Param::choice("mode", "close", &["close", "open"]))
            // .allows_short()
    }

    /// Runs once per symbol with validated parameters.
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

    /// Called once per completed bar. Use `ctx` to read the account and place orders.
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
        // Other tools:
        //   ctx.buy_with(Size::Percent(0.5), Exec::NextBarOpen, Some(stop), Some(target));
        //   ctx.sell_short(Size::Default);   // needs `.allows_short()` on the manifest
        //   ctx.set_stop(price);
        //   ctx.position(), ctx.equity(), ctx.bar_index(), ctx.date(), ctx.time()
        Ok(())
    }

    // Optional hooks:
    // fn on_session_start(&mut self, ctx: &mut Ctx) -> Result<()> { Ok(()) }
    // fn on_session_end(&mut self, ctx: &mut Ctx) -> Result<()> { Ok(()) }
    // fn on_fill(&mut self, ctx: &mut Ctx, fill: &Fill) -> Result<()> { Ok(()) }
}

/// Registers this file with the engine. Leave as is.
pub fn entry() -> StrategyEntry {
    StrategyEntry::of::<__STRUCT_NAME__>()
}
