//! Shared event-driven strategy runtime.
//!
//! Historical and live adapters emit the same [`MarketEvent`] values. Strategy
//! code produces broker-neutral [`OrderIntent`] values; simulated and live
//! brokers own fills, positions, cash, and reconciliation.

use std::collections::{BTreeMap, HashMap};

use anyhow::{Result, bail};
use chrono::{NaiveDate, NaiveTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyScope {
    /// One isolated strategy instance is created for every selected instrument.
    PerInstrument,
    /// One strategy instance receives synchronized slices for the whole universe.
    Portfolio,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketBar {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MarketOpen {
    pub price: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MarketEvent {
    SessionStart {
        date: NaiveDate,
        symbols: Vec<String>,
    },
    BarOpen {
        date: NaiveDate,
        time: NaiveTime,
        prices: BTreeMap<String, MarketOpen>,
    },
    BarClose {
        date: NaiveDate,
        time: NaiveTime,
        bars: BTreeMap<String, MarketBar>,
    },
    SessionEnd {
        date: NaiveDate,
        symbols: Vec<String>,
    },
}

impl MarketEvent {
    pub fn date(&self) -> NaiveDate {
        match self {
            Self::SessionStart { date, .. }
            | Self::BarOpen { date, .. }
            | Self::BarClose { date, .. }
            | Self::SessionEnd { date, .. } => *date,
        }
    }

    pub fn time(&self) -> Option<NaiveTime> {
        match self {
            Self::BarOpen { time, .. } | Self::BarClose { time, .. } => Some(*time),
            Self::SessionStart { .. } | Self::SessionEnd { .. } => None,
        }
    }

    fn for_symbol(&self, symbol: &str) -> Option<Self> {
        match self {
            Self::SessionStart { date, symbols } | Self::SessionEnd { date, symbols } => {
                if !symbols.iter().any(|value| value == symbol) {
                    return None;
                }
                let symbols = vec![symbol.to_owned()];
                Some(match self {
                    Self::SessionStart { .. } => Self::SessionStart {
                        date: *date,
                        symbols,
                    },
                    _ => Self::SessionEnd {
                        date: *date,
                        symbols,
                    },
                })
            }
            Self::BarOpen { date, time, prices } => {
                prices.get(symbol).cloned().map(|price| Self::BarOpen {
                    date: *date,
                    time: *time,
                    prices: BTreeMap::from([(symbol.to_owned(), price)]),
                })
            }
            Self::BarClose { date, time, bars } => {
                bars.get(symbol).cloned().map(|bar| Self::BarClose {
                    date: *date,
                    time: *time,
                    bars: BTreeMap::from([(symbol.to_owned(), bar)]),
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoricalSession {
    pub date: NaiveDate,
    pub symbols: Vec<String>,
    pub bars: Vec<(NaiveTime, BTreeMap<String, MarketBar>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn sign(self) -> f64 {
        match self {
            Self::Buy => 1.0,
            Self::Sell => -1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionTiming {
    Immediate,
    NextBarOpen,
    ThisBarClose,
}

/// Time-based exit attached to an entry; evaluated at bar opens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeExit {
    /// Exit at the open of the first bar at or after this wall-clock time.
    At(NaiveTime),
    /// Exit at the open of the first bar at or after entry time plus this many minutes.
    AfterMinutes(i64),
}

/// Resting limit order rules.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitRules {
    pub limit: f64,
    /// Cancel if the first observed open is at or within this fraction of the limit
    /// (a gap through the limit before the order could work).
    pub cancel_if_first_open_within: Option<f64>,
    /// Stop working at this time (no fills at or after it).
    pub expires_at: Option<NaiveTime>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OrderIntent {
    EnterBracket {
        symbol: String,
        side: Side,
        quantity: usize,
        timing: ExecutionTiming,
        stop: Option<f64>,
        target: Option<f64>,
        metadata: BTreeMap<String, String>,
    },
    /// Resting limit entry. Fills at a bar open that is through the limit, or at the limit
    /// when a bar trades through it. `stop_percent` is applied to the actual fill price.
    EnterLimit {
        symbol: String,
        side: Side,
        quantity: usize,
        rules: LimitRules,
        stop_percent: Option<f64>,
        target: Option<f64>,
        time_exit: Option<TimeExit>,
        metadata: BTreeMap<String, String>,
    },
    /// Attach or replace a time-based exit on an open position.
    SetTimeExit {
        symbol: String,
        exit: TimeExit,
    },
    CancelEntry {
        symbol: String,
    },
    ExitPosition {
        symbol: String,
        timing: ExecutionTiming,
        reason: String,
    },
    ReplaceStop {
        symbol: String,
        stop: f64,
        managed: bool,
    },
}

impl OrderIntent {
    pub fn symbol(&self) -> &str {
        match self {
            Self::EnterBracket { symbol, .. }
            | Self::EnterLimit { symbol, .. }
            | Self::SetTimeExit { symbol, .. }
            | Self::CancelEntry { symbol }
            | Self::ExitPosition { symbol, .. }
            | Self::ReplaceStop { symbol, .. } => symbol,
        }
    }
    /// Sort key used by the daily entry cap; higher wins.
    pub fn priority(&self) -> f64 {
        let metadata = match self {
            Self::EnterBracket { metadata, .. } | Self::EnterLimit { metadata, .. } => metadata,
            _ => return 0.0,
        };
        metadata
            .get("priority")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PositionSnapshot {
    pub symbol: String,
    pub side: Side,
    pub quantity: usize,
    pub entry_price: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortfolioSnapshot {
    /// Closed P&L plus starting equity. Open positions are excluded.
    pub realized_equity: f64,
    /// Realized equity plus open-position P&L marked to the latest observed price.
    pub total_equity: f64,
    pub positions: BTreeMap<String, PositionSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedTrade {
    pub symbol: String,
    pub side: Side,
    pub quantity: usize,
    pub entry_date: NaiveDate,
    pub entry_time: NaiveTime,
    pub exit_date: NaiveDate,
    pub exit_time: NaiveTime,
    pub entry_price: f64,
    pub exit_price: f64,
    pub gross_pnl: f64,
    pub commission: f64,
    pub pnl: f64,
    pub equity_at_entry: f64,
    pub exit_reason: String,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BrokerEvent {
    OrderAccepted { symbol: String },
    OrderRejected { symbol: String, reason: String },
    PositionOpened { position: PositionSnapshot },
    PositionClosed { trade: CompletedTrade },
    StopReplaced { symbol: String, stop: f64 },
}

impl BrokerEvent {
    pub fn symbol(&self) -> &str {
        match self {
            Self::OrderAccepted { symbol }
            | Self::OrderRejected { symbol, .. }
            | Self::StopReplaced { symbol, .. } => symbol,
            Self::PositionOpened { position } => &position.symbol,
            Self::PositionClosed { trade } => &trade.symbol,
        }
    }
}

pub trait EventStrategy {
    fn id(&self) -> &'static str;
    fn scope(&self) -> StrategyScope;

    fn on_market_event(
        &mut self,
        event: &MarketEvent,
        portfolio: &PortfolioSnapshot,
    ) -> Result<Vec<OrderIntent>>;

    fn on_broker_event(&mut self, _event: &BrokerEvent) -> Result<Vec<OrderIntent>> {
        Ok(Vec::new())
    }
}

pub enum StrategyHost<S: EventStrategy> {
    PerInstrument(BTreeMap<String, S>),
    Portfolio(S),
}

impl<S: EventStrategy> StrategyHost<S> {
    pub fn per_instrument(strategies: BTreeMap<String, S>) -> Result<Self> {
        if strategies.is_empty() {
            bail!("per-instrument strategy host requires at least one instrument");
        }
        if let Some((symbol, strategy)) = strategies
            .iter()
            .find(|(_, strategy)| strategy.scope() != StrategyScope::PerInstrument)
        {
            bail!(
                "strategy {} for {symbol} is not per-instrument",
                strategy.id()
            );
        }
        Ok(Self::PerInstrument(strategies))
    }

    pub fn portfolio(strategy: S) -> Result<Self> {
        if strategy.scope() != StrategyScope::Portfolio {
            bail!("strategy {} is not portfolio-aware", strategy.id());
        }
        Ok(Self::Portfolio(strategy))
    }

    pub fn strategy(&self, symbol: &str) -> Option<&S> {
        match self {
            Self::PerInstrument(strategies) => strategies.get(symbol),
            Self::Portfolio(strategy) => Some(strategy),
        }
    }

    pub fn strategy_mut(&mut self, symbol: &str) -> Option<&mut S> {
        match self {
            Self::PerInstrument(strategies) => strategies.get_mut(symbol),
            Self::Portfolio(strategy) => Some(strategy),
        }
    }

    fn on_market_event(
        &mut self,
        event: &MarketEvent,
        portfolio: &PortfolioSnapshot,
    ) -> Result<Vec<OrderIntent>> {
        match self {
            Self::Portfolio(strategy) => strategy.on_market_event(event, portfolio),
            Self::PerInstrument(strategies) => {
                let mut intents = Vec::new();
                for (symbol, strategy) in strategies {
                    if let Some(event) = event.for_symbol(symbol) {
                        intents.extend(strategy.on_market_event(&event, portfolio)?);
                    }
                }
                Ok(intents)
            }
        }
    }

    fn on_broker_events(&mut self, events: &[BrokerEvent]) -> Result<Vec<OrderIntent>> {
        let mut intents = Vec::new();
        match self {
            Self::Portfolio(strategy) => {
                for event in events {
                    intents.extend(strategy.on_broker_event(event)?);
                }
            }
            Self::PerInstrument(strategies) => {
                for event in events {
                    if let Some(strategy) = strategies.get_mut(event.symbol()) {
                        intents.extend(strategy.on_broker_event(event)?);
                    }
                }
            }
        }
        Ok(intents)
    }
}

pub trait BrokerAdapter {
    fn on_market_event(&mut self, event: &MarketEvent) -> Result<Vec<BrokerEvent>>;
    fn submit(
        &mut self,
        event: &MarketEvent,
        intents: Vec<OrderIntent>,
    ) -> Result<Vec<BrokerEvent>>;
    fn snapshot(&self) -> PortfolioSnapshot;

    /// Close or discard historical-only state at the end of a finite replay.
    /// Live adapters keep the default no-op implementation.
    fn finalize_historical(&mut self) -> Result<Vec<BrokerEvent>> {
        Ok(Vec::new())
    }
}

/// Contract for a future live adapter. It deliberately exposes reconciliation
/// and connection health in addition to the normal broker interface.
pub trait LiveBrokerAdapter: BrokerAdapter {
    fn connect(&mut self) -> Result<()>;
    fn disconnect(&mut self) -> Result<()>;
    fn is_connected(&self) -> bool;
    fn reconcile(&mut self) -> Result<Vec<BrokerEvent>>;
}

#[derive(Debug, Clone, Copy)]
pub struct SimulationCosts {
    pub tick_size: f64,
    pub entry_slippage_ticks: u32,
    pub exit_slippage_ticks: u32,
    pub commission_per_unit_per_fill: f64,
    pub apply_exit_slippage_to_targets: bool,
    /// When set, replaces ticks and per-unit commission with a notional charge of half this
    /// amount on entry and half on exit.
    pub all_in_round_trip_bps: Option<f64>,
}

impl Default for SimulationCosts {
    fn default() -> Self {
        Self {
            tick_size: 0.01,
            entry_slippage_ticks: 0,
            exit_slippage_ticks: 0,
            commission_per_unit_per_fill: 0.0,
            apply_exit_slippage_to_targets: false,
            all_in_round_trip_bps: None,
        }
    }
}

/// How simultaneous entries compete for limited daily slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TieBreak {
    /// Highest `priority` metadata first, then alphabetical.
    #[default]
    Priority,
    /// Deterministic shuffle seeded by `seed ^ date`.
    Random,
    Alphabetical,
}

/// Account-wide entry limits applied at fill time.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EntryLimits {
    pub max_entries_per_day: Option<usize>,
    pub max_open_positions: Option<usize>,
    pub tie_break: TieBreak,
    pub seed: u64,
}

#[derive(Debug, Clone)]
struct SimulatedPosition {
    snapshot: PositionSnapshot,
    entry_date: NaiveDate,
    entry_time: NaiveTime,
    entry_commission: f64,
    equity_at_entry: f64,
    managed_stop: bool,
    time_exit: Option<TimeExit>,
    metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct PendingLimit {
    intent: OrderIntent,
    first_open_seen: bool,
}

#[derive(Debug, Clone)]
struct PendingExit {
    reason: String,
}

pub struct SimulatedBroker {
    costs: SimulationCosts,
    limits: EntryLimits,
    realized_equity: f64,
    positions: BTreeMap<String, SimulatedPosition>,
    pending_entries: Vec<OrderIntent>,
    pending_limits: BTreeMap<String, PendingLimit>,
    pending_exits: HashMap<String, PendingExit>,
    completed_trades: Vec<CompletedTrade>,
    last_marks: BTreeMap<String, (NaiveDate, NaiveTime, f64)>,
    fills_today: (Option<NaiveDate>, usize),
}

impl SimulatedBroker {
    pub fn new(initial_equity: f64, costs: SimulationCosts) -> Result<Self> {
        if !(initial_equity.is_finite() && initial_equity > 0.0) {
            bail!("initial simulated equity must be positive");
        }
        if !(costs.tick_size.is_finite() && costs.tick_size > 0.0) {
            bail!("simulated broker tick size must be positive");
        }
        Ok(Self {
            costs,
            limits: EntryLimits::default(),
            realized_equity: initial_equity,
            positions: BTreeMap::new(),
            pending_entries: Vec::new(),
            pending_limits: BTreeMap::new(),
            pending_exits: HashMap::new(),
            completed_trades: Vec::new(),
            last_marks: BTreeMap::new(),
            fills_today: (None, 0),
        })
    }

    pub fn with_limits(mut self, limits: EntryLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn completed_trades(&self) -> &[CompletedTrade] {
        &self.completed_trades
    }

    fn entry_commission_for(&self, price: f64, quantity: usize) -> f64 {
        match self.costs.all_in_round_trip_bps {
            Some(bps) => price * quantity as f64 * bps / 20_000.0,
            None => quantity as f64 * self.costs.commission_per_unit_per_fill,
        }
    }

    fn exit_commission_for(&self, entry_price: f64, quantity: usize) -> f64 {
        match self.costs.all_in_round_trip_bps {
            Some(bps) => entry_price * quantity as f64 * bps / 20_000.0,
            None => quantity as f64 * self.costs.commission_per_unit_per_fill,
        }
    }

    fn slippage(&self, entry: bool) -> f64 {
        if self.costs.all_in_round_trip_bps.is_some() {
            return 0.0;
        }
        let ticks = if entry {
            self.costs.entry_slippage_ticks
        } else {
            self.costs.exit_slippage_ticks
        };
        ticks as f64 * self.costs.tick_size
    }

    fn fills_today(&mut self, date: NaiveDate) -> usize {
        if self.fills_today.0 != Some(date) {
            self.fills_today = (Some(date), 0);
        }
        self.fills_today.1
    }

    /// Applies the daily entry cap and open-position cap to simultaneous fills, returning
    /// the intents allowed to fill in order and the ones that must be dropped.
    fn arbitrate_entries(
        &mut self,
        date: NaiveDate,
        candidates: Vec<OrderIntent>,
    ) -> (Vec<OrderIntent>, Vec<OrderIntent>) {
        if candidates.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let filled_today = self.fills_today(date);
        let mut slots = usize::MAX;
        if let Some(max) = self.limits.max_entries_per_day {
            slots = slots.min(max.saturating_sub(filled_today));
        }
        if let Some(max) = self.limits.max_open_positions {
            slots = slots.min(max.saturating_sub(self.positions.len()));
        }
        if slots >= candidates.len() {
            return (candidates, Vec::new());
        }
        let mut ordered = candidates;
        match self.limits.tie_break {
            TieBreak::Priority => ordered.sort_by(|a, b| {
                b.priority()
                    .partial_cmp(&a.priority())
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.symbol().cmp(b.symbol()))
            }),
            TieBreak::Alphabetical => ordered.sort_by(|a, b| a.symbol().cmp(b.symbol())),
            TieBreak::Random => {
                use rand::SeedableRng;
                use rand::seq::SliceRandom;
                let date_seed = chrono::Datelike::num_days_from_ce(&date) as u64;
                let mut rng = rand::rngs::StdRng::seed_from_u64(self.limits.seed ^ date_seed);
                ordered.sort_by(|a, b| a.symbol().cmp(b.symbol()));
                ordered.shuffle(&mut rng);
            }
        }
        let dropped = ordered.split_off(slots);
        (ordered, dropped)
    }

    fn round_tick(&self, value: f64) -> f64 {
        (value / self.costs.tick_size).round() * self.costs.tick_size
    }

    fn open_position(
        &mut self,
        date: NaiveDate,
        time: NaiveTime,
        reference_price: f64,
        intent: OrderIntent,
    ) -> BrokerEvent {
        let (symbol, side, quantity, stop, stop_percent, target, time_exit, metadata, limit_cap) =
            match intent {
                OrderIntent::EnterBracket {
                    symbol,
                    side,
                    quantity,
                    stop,
                    target,
                    metadata,
                    ..
                } => (
                    symbol, side, quantity, stop, None, target, None, metadata, None,
                ),
                OrderIntent::EnterLimit {
                    symbol,
                    side,
                    quantity,
                    rules,
                    stop_percent,
                    target,
                    time_exit,
                    metadata,
                } => (
                    symbol,
                    side,
                    quantity,
                    None,
                    stop_percent,
                    target,
                    time_exit,
                    metadata,
                    Some(rules.limit),
                ),
                _ => unreachable!("open_position requires an entry intent"),
            };
        if quantity == 0 || self.positions.contains_key(&symbol) {
            return BrokerEvent::OrderRejected {
                symbol,
                reason: "zero quantity or existing position".to_owned(),
            };
        }
        let slip = self.slippage(true);
        let mut entry_price = reference_price + side.sign() * slip;
        if let Some(limit) = limit_cap {
            // A resting limit never fills worse than its limit price.
            entry_price = match side {
                Side::Buy => entry_price.min(limit),
                Side::Sell => entry_price.max(limit),
            };
        }
        let entry_price = self.round_tick(entry_price);
        let stop = stop.or_else(|| {
            stop_percent.map(|percent| match side {
                Side::Buy => entry_price * (1.0 - percent),
                Side::Sell => entry_price * (1.0 + percent),
            })
        });
        let entry_commission = self.entry_commission_for(entry_price, quantity);
        let equity_at_entry = self.realized_equity;
        let snapshot = PositionSnapshot {
            symbol: symbol.clone(),
            side,
            quantity,
            entry_price,
            stop,
            target,
        };
        self.positions.insert(
            symbol,
            SimulatedPosition {
                snapshot: snapshot.clone(),
                entry_date: date,
                entry_time: time,
                entry_commission,
                equity_at_entry,
                managed_stop: false,
                time_exit,
                metadata,
            },
        );
        self.fills_today(date);
        self.fills_today.1 += 1;
        BrokerEvent::PositionOpened { position: snapshot }
    }

    fn close_position(
        &mut self,
        symbol: &str,
        date: NaiveDate,
        time: NaiveTime,
        reference_price: f64,
        reason: String,
        apply_slippage: bool,
    ) -> Option<BrokerEvent> {
        let position = self.positions.remove(symbol)?;
        let slip = if apply_slippage {
            self.slippage(false)
        } else {
            0.0
        };
        let exit_price = self.round_tick(reference_price - position.snapshot.side.sign() * slip);
        let exit_commission =
            self.exit_commission_for(position.snapshot.entry_price, position.snapshot.quantity);
        let gross_pnl = position.snapshot.side.sign()
            * (exit_price - position.snapshot.entry_price)
            * position.snapshot.quantity as f64;
        let commission = position.entry_commission + exit_commission;
        let pnl = gross_pnl - commission;
        self.realized_equity += pnl;
        let trade = CompletedTrade {
            symbol: position.snapshot.symbol,
            side: position.snapshot.side,
            quantity: position.snapshot.quantity,
            entry_date: position.entry_date,
            entry_time: position.entry_time,
            exit_date: date,
            exit_time: time,
            entry_price: position.snapshot.entry_price,
            exit_price,
            gross_pnl,
            commission,
            pnl,
            equity_at_entry: position.equity_at_entry,
            exit_reason: reason,
            metadata: position.metadata,
        };
        self.completed_trades.push(trade.clone());
        Some(BrokerEvent::PositionClosed { trade })
    }

    fn fill_next_open(
        &mut self,
        date: NaiveDate,
        time: NaiveTime,
        prices: &BTreeMap<String, MarketOpen>,
    ) -> Vec<BrokerEvent> {
        let mut events = Vec::new();
        // Time-based exits fire at the first open at or after their due time.
        let due = self
            .positions
            .iter()
            .filter_map(|(symbol, position)| {
                let open = prices.get(symbol)?;
                let fire = match position.time_exit? {
                    TimeExit::At(at) => time >= at,
                    TimeExit::AfterMinutes(minutes) => {
                        let entry = position.entry_date.and_time(position.entry_time);
                        date.and_time(time) >= entry + chrono::Duration::minutes(minutes)
                    }
                };
                fire.then(|| (symbol.clone(), open.price))
            })
            .collect::<Vec<_>>();
        for (symbol, price) in due {
            if let Some(event) =
                self.close_position(&symbol, date, time, price, "timed_exit".to_owned(), true)
            {
                events.push(event);
            }
        }
        // Gather every entry that can fill at this open, then let the caps arbitrate.
        let mut fillable = Vec::new();
        let pending_entries = std::mem::take(&mut self.pending_entries);
        for intent in pending_entries {
            if prices.contains_key(intent.symbol()) {
                fillable.push(intent);
            } else {
                self.pending_entries.push(intent);
            }
        }
        let symbols = self.pending_limits.keys().cloned().collect::<Vec<_>>();
        for symbol in symbols {
            let Some(open) = prices.get(&symbol) else {
                continue;
            };
            let pending = self.pending_limits.get_mut(&symbol).expect("pending limit");
            let OrderIntent::EnterLimit { side, rules, .. } = &pending.intent else {
                continue;
            };
            if rules.expires_at.is_some_and(|expiry| time >= expiry) {
                self.pending_limits.remove(&symbol);
                events.push(BrokerEvent::OrderRejected {
                    symbol,
                    reason: "limit_expired".to_owned(),
                });
                continue;
            }
            let through = match side {
                Side::Buy => open.price <= rules.limit,
                Side::Sell => open.price >= rules.limit,
            };
            if !pending.first_open_seen {
                pending.first_open_seen = true;
                if let Some(buffer) = rules.cancel_if_first_open_within {
                    let veto = match side {
                        Side::Buy => open.price <= rules.limit * (1.0 + buffer),
                        Side::Sell => open.price >= rules.limit * (1.0 - buffer),
                    };
                    if veto {
                        self.pending_limits.remove(&symbol);
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "gap_veto".to_owned(),
                        });
                        continue;
                    }
                }
            }
            if through {
                let pending = self.pending_limits.remove(&symbol).expect("pending limit");
                fillable.push(pending.intent);
            }
        }
        let (allowed, dropped) = self.arbitrate_entries(date, fillable);
        for intent in allowed {
            let price = prices[intent.symbol()].price;
            events.push(self.open_position(date, time, price, intent));
        }
        for intent in dropped {
            events.push(BrokerEvent::OrderRejected {
                symbol: intent.symbol().to_owned(),
                reason: "daily_entry_cap".to_owned(),
            });
        }
        let symbols = self.pending_exits.keys().cloned().collect::<Vec<_>>();
        for symbol in symbols {
            let Some(open) = prices.get(&symbol) else {
                continue;
            };
            let reason = self
                .pending_exits
                .remove(&symbol)
                .expect("pending exit exists")
                .reason;
            if let Some(event) = self.close_position(&symbol, date, time, open.price, reason, true)
            {
                events.push(event);
            }
        }
        events
    }

    /// Resting limits fill at the limit when a bar trades through it. Bars cannot say whether
    /// the limit or a protective stop traded first, so a stop inside the entry bar is assumed
    /// to have hit after the fill.
    fn fill_limits_in_bar(
        &mut self,
        date: NaiveDate,
        time: NaiveTime,
        bars: &BTreeMap<String, MarketBar>,
    ) -> Vec<BrokerEvent> {
        let mut events = Vec::new();
        let mut fillable = Vec::new();
        let symbols = self.pending_limits.keys().cloned().collect::<Vec<_>>();
        for symbol in symbols {
            let Some(bar) = bars.get(&symbol) else {
                continue;
            };
            let pending = self.pending_limits.get_mut(&symbol).expect("pending limit");
            let OrderIntent::EnterLimit { side, rules, .. } = &pending.intent else {
                continue;
            };
            if rules.expires_at.is_some_and(|expiry| time >= expiry) {
                continue;
            }
            pending.first_open_seen = true;
            let through = match side {
                Side::Buy => bar.low <= rules.limit,
                Side::Sell => bar.high >= rules.limit,
            };
            if through {
                let pending = self.pending_limits.remove(&symbol).expect("pending limit");
                fillable.push(pending.intent);
            }
        }
        let (allowed, dropped) = self.arbitrate_entries(date, fillable);
        for intent in allowed {
            let OrderIntent::EnterLimit { rules, .. } = &intent else {
                continue;
            };
            let symbol = intent.symbol().to_owned();
            let limit = rules.limit;
            events.push(self.open_position(date, time, limit, intent));
            if let Some(position) = self.positions.get(&symbol) {
                let bar = &bars[&symbol];
                let side = position.snapshot.side;
                let stop_hit = position.snapshot.stop.is_some_and(|stop| match side {
                    Side::Buy => bar.low <= stop,
                    Side::Sell => bar.high >= stop,
                });
                if stop_hit {
                    let stop = position.snapshot.stop.expect("checked stop");
                    if let Some(event) = self.close_position(
                        &symbol,
                        date,
                        time,
                        stop,
                        "initial_stop".to_owned(),
                        true,
                    ) {
                        events.push(event);
                    }
                }
            }
        }
        for intent in dropped {
            events.push(BrokerEvent::OrderRejected {
                symbol: intent.symbol().to_owned(),
                reason: "daily_entry_cap".to_owned(),
            });
        }
        events
    }

    fn evaluate_brackets(
        &mut self,
        date: NaiveDate,
        time: NaiveTime,
        bars: &BTreeMap<String, MarketBar>,
    ) -> Vec<BrokerEvent> {
        let mut exits = Vec::new();
        for (symbol, position) in &self.positions {
            let Some(bar) = bars.get(symbol) else {
                continue;
            };
            let side = position.snapshot.side;
            let stop_hit = position.snapshot.stop.is_some_and(|stop| match side {
                Side::Buy => bar.low <= stop,
                Side::Sell => bar.high >= stop,
            });
            if stop_hit {
                let stop = position.snapshot.stop.expect("checked stop");
                let reference = match side {
                    Side::Buy => stop.min(bar.open),
                    Side::Sell => stop.max(bar.open),
                };
                exits.push((
                    symbol.clone(),
                    reference,
                    if position.managed_stop {
                        "managed_stop"
                    } else {
                        "initial_stop"
                    }
                    .to_owned(),
                    true,
                ));
                continue;
            }
            let target_hit = position.snapshot.target.is_some_and(|target| match side {
                Side::Buy => bar.high >= target,
                Side::Sell => bar.low <= target,
            });
            if target_hit {
                let target = position.snapshot.target.expect("checked target");
                let reference = match side {
                    Side::Buy => target.max(bar.open),
                    Side::Sell => target.min(bar.open),
                };
                exits.push((
                    symbol.clone(),
                    reference,
                    "profit_target".to_owned(),
                    self.costs.apply_exit_slippage_to_targets,
                ));
            }
        }
        exits
            .into_iter()
            .filter_map(|(symbol, price, reason, slippage)| {
                self.close_position(&symbol, date, time, price, reason, slippage)
            })
            .collect()
    }
}

impl BrokerAdapter for SimulatedBroker {
    fn on_market_event(&mut self, event: &MarketEvent) -> Result<Vec<BrokerEvent>> {
        Ok(match event {
            MarketEvent::BarOpen { date, time, prices } => {
                for (symbol, open) in prices {
                    self.last_marks
                        .insert(symbol.clone(), (*date, *time, open.price));
                }
                self.fill_next_open(*date, *time, prices)
            }
            MarketEvent::BarClose { date, time, bars } => {
                for (symbol, bar) in bars {
                    self.last_marks
                        .insert(symbol.clone(), (*date, *time, bar.close));
                }
                let mut events = self.evaluate_brackets(*date, *time, bars);
                events.extend(self.fill_limits_in_bar(*date, *time, bars));
                events
            }
            MarketEvent::SessionStart { .. } => Vec::new(),
            MarketEvent::SessionEnd { .. } => {
                // Day orders: anything still resting is cancelled at the close.
                let expired = std::mem::take(&mut self.pending_limits);
                expired
                    .into_keys()
                    .map(|symbol| BrokerEvent::OrderRejected {
                        symbol,
                        reason: "day_order_expired".to_owned(),
                    })
                    .collect()
            }
        })
    }

    fn submit(
        &mut self,
        event: &MarketEvent,
        intents: Vec<OrderIntent>,
    ) -> Result<Vec<BrokerEvent>> {
        let mut events = Vec::new();
        let mut close_entries = Vec::new();
        for intent in intents {
            match intent {
                OrderIntent::EnterBracket {
                    timing: ExecutionTiming::NextBarOpen,
                    ..
                } => {
                    let symbol = intent.symbol().to_owned();
                    self.pending_entries.push(intent);
                    events.push(BrokerEvent::OrderAccepted { symbol });
                }
                OrderIntent::EnterBracket {
                    timing: ExecutionTiming::Immediate,
                    ..
                } => {
                    let MarketEvent::BarOpen { date, time, prices } = event else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol: intent.symbol().to_owned(),
                            reason: "immediate entries require a bar-open event".to_owned(),
                        });
                        continue;
                    };
                    let symbol = intent.symbol().to_owned();
                    if let Some(open) = prices.get(&symbol) {
                        events.push(self.open_position(*date, *time, open.price, intent));
                    } else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "missing current open price".to_owned(),
                        });
                    }
                }
                OrderIntent::EnterBracket {
                    timing: ExecutionTiming::ThisBarClose,
                    ..
                } => {
                    if !matches!(event, MarketEvent::BarClose { .. }) {
                        events.push(BrokerEvent::OrderRejected {
                            symbol: intent.symbol().to_owned(),
                            reason: "this-bar-close entries require a bar-close event".to_owned(),
                        });
                        continue;
                    }
                    close_entries.push(intent);
                }
                OrderIntent::EnterLimit { .. } => {
                    let symbol = intent.symbol().to_owned();
                    if self.positions.contains_key(&symbol)
                        || self.pending_limits.contains_key(&symbol)
                    {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "existing position or resting order".to_owned(),
                        });
                        continue;
                    }
                    self.pending_limits.insert(
                        symbol.clone(),
                        PendingLimit {
                            intent,
                            first_open_seen: false,
                        },
                    );
                    events.push(BrokerEvent::OrderAccepted { symbol });
                }
                OrderIntent::CancelEntry { symbol } => {
                    let removed = self.pending_limits.remove(&symbol).is_some();
                    let before = self.pending_entries.len();
                    self.pending_entries
                        .retain(|pending| pending.symbol() != symbol);
                    if removed || self.pending_entries.len() != before {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "cancelled".to_owned(),
                        });
                    }
                }
                OrderIntent::SetTimeExit { symbol, exit } => {
                    if let Some(position) = self.positions.get_mut(&symbol) {
                        position.time_exit = Some(exit);
                        events.push(BrokerEvent::OrderAccepted { symbol });
                    } else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "no open position".to_owned(),
                        });
                    }
                }
                OrderIntent::ExitPosition {
                    symbol,
                    timing: ExecutionTiming::NextBarOpen,
                    reason,
                } => {
                    if self.positions.contains_key(&symbol) {
                        self.pending_exits
                            .insert(symbol.clone(), PendingExit { reason });
                        events.push(BrokerEvent::OrderAccepted { symbol });
                    } else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "no open position".to_owned(),
                        });
                    }
                }
                OrderIntent::ExitPosition {
                    symbol,
                    timing: ExecutionTiming::ThisBarClose,
                    reason,
                } => {
                    let MarketEvent::BarClose { date, time, bars } = event else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "this-bar-close exit requires a bar-close event".to_owned(),
                        });
                        continue;
                    };
                    let Some(bar) = bars.get(&symbol) else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "missing current close price".to_owned(),
                        });
                        continue;
                    };
                    if let Some(closed) =
                        self.close_position(&symbol, *date, *time, bar.close, reason, true)
                    {
                        events.push(closed);
                    } else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "no open position".to_owned(),
                        });
                    }
                }
                OrderIntent::ExitPosition {
                    symbol,
                    timing: ExecutionTiming::Immediate,
                    reason,
                } => {
                    let MarketEvent::BarOpen { date, time, prices } = event else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "immediate exit requires a bar-open event".to_owned(),
                        });
                        continue;
                    };
                    let Some(open) = prices.get(&symbol) else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "missing current open price".to_owned(),
                        });
                        continue;
                    };
                    if let Some(closed) =
                        self.close_position(&symbol, *date, *time, open.price, reason, true)
                    {
                        events.push(closed);
                    } else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "no open position".to_owned(),
                        });
                    }
                }
                OrderIntent::ReplaceStop {
                    symbol,
                    stop,
                    managed,
                } => {
                    if let Some(position) = self.positions.get_mut(&symbol) {
                        position.snapshot.stop = Some(stop);
                        position.managed_stop |= managed;
                        events.push(BrokerEvent::StopReplaced { symbol, stop });
                    } else {
                        events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "no open position".to_owned(),
                        });
                    }
                }
            }
        }
        if !close_entries.is_empty() {
            if let MarketEvent::BarClose { date, time, bars } = event {
                let (allowed, dropped) = self.arbitrate_entries(*date, close_entries);
                for intent in allowed {
                    let symbol = intent.symbol().to_owned();
                    match bars.get(&symbol) {
                        Some(bar) => {
                            events.push(self.open_position(*date, *time, bar.close, intent))
                        }
                        None => events.push(BrokerEvent::OrderRejected {
                            symbol,
                            reason: "missing current close price".to_owned(),
                        }),
                    }
                }
                for intent in dropped {
                    events.push(BrokerEvent::OrderRejected {
                        symbol: intent.symbol().to_owned(),
                        reason: "daily_entry_cap".to_owned(),
                    });
                }
            }
        }
        Ok(events)
    }

    fn snapshot(&self) -> PortfolioSnapshot {
        let unrealized = self
            .positions
            .iter()
            .map(|(symbol, position)| {
                let mark = self
                    .last_marks
                    .get(symbol)
                    .map_or(position.snapshot.entry_price, |(_, _, price)| *price);
                position.snapshot.side.sign()
                    * (mark - position.snapshot.entry_price)
                    * position.snapshot.quantity as f64
                    - position.entry_commission
            })
            .sum::<f64>();
        PortfolioSnapshot {
            realized_equity: self.realized_equity,
            total_equity: self.realized_equity + unrealized,
            positions: self
                .positions
                .iter()
                .map(|(symbol, position)| (symbol.clone(), position.snapshot.clone()))
                .collect(),
        }
    }

    fn finalize_historical(&mut self) -> Result<Vec<BrokerEvent>> {
        self.pending_entries.clear();
        self.pending_limits.clear();
        self.pending_exits.clear();
        let symbols = self.positions.keys().cloned().collect::<Vec<_>>();
        let mut events = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            let (date, time, price) = self
                .last_marks
                .get(&symbol)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("missing final mark for {symbol}"))?;
            if let Some(event) = self.close_position(
                &symbol,
                date,
                time,
                price,
                "end_of_backtest".to_owned(),
                true,
            ) {
                events.push(event);
            }
        }
        Ok(events)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HistoricalRunStats {
    pub sessions: usize,
    pub bar_opens: usize,
    pub bar_closes: usize,
    pub order_intents: usize,
    pub broker_events: usize,
}

pub struct HistoricalEventEngine;

impl HistoricalEventEngine {
    pub fn run<S: EventStrategy, B: BrokerAdapter>(
        host: &mut StrategyHost<S>,
        broker: &mut B,
        sessions: &[HistoricalSession],
    ) -> Result<HistoricalRunStats> {
        let mut stats = HistoricalRunStats::default();
        for (session_index, session) in sessions.iter().enumerate() {
            stats.sessions += 1;
            Self::dispatch(
                host,
                broker,
                MarketEvent::SessionStart {
                    date: session.date,
                    symbols: session.symbols.clone(),
                },
                &mut stats,
            )?;
            for (time, bars) in &session.bars {
                let prices = bars
                    .iter()
                    .map(|(symbol, bar)| (symbol.clone(), MarketOpen { price: bar.open }))
                    .collect();
                stats.bar_opens += 1;
                Self::dispatch(
                    host,
                    broker,
                    MarketEvent::BarOpen {
                        date: session.date,
                        time: *time,
                        prices,
                    },
                    &mut stats,
                )?;
                stats.bar_closes += 1;
                Self::dispatch(
                    host,
                    broker,
                    MarketEvent::BarClose {
                        date: session.date,
                        time: *time,
                        bars: bars.clone(),
                    },
                    &mut stats,
                )?;
            }
            if session_index + 1 == sessions.len() {
                let broker_events = broker.finalize_historical()?;
                stats.broker_events += broker_events.len();
                let followups = host.on_broker_events(&broker_events)?;
                anyhow::ensure!(
                    followups.is_empty(),
                    "a strategy emitted new orders while historical replay was finalizing"
                );
            }
            Self::dispatch(
                host,
                broker,
                MarketEvent::SessionEnd {
                    date: session.date,
                    symbols: session.symbols.clone(),
                },
                &mut stats,
            )?;
        }
        Ok(stats)
    }

    fn dispatch<S: EventStrategy, B: BrokerAdapter>(
        host: &mut StrategyHost<S>,
        broker: &mut B,
        event: MarketEvent,
        stats: &mut HistoricalRunStats,
    ) -> Result<()> {
        let broker_events = broker.on_market_event(&event)?;
        stats.broker_events += broker_events.len();
        let followups = host.on_broker_events(&broker_events)?;
        Self::submit(host, broker, &event, followups, stats)?;

        let intents = host.on_market_event(&event, &broker.snapshot())?;
        Self::submit(host, broker, &event, intents, stats)
    }

    fn submit<S: EventStrategy, B: BrokerAdapter>(
        host: &mut StrategyHost<S>,
        broker: &mut B,
        event: &MarketEvent,
        intents: Vec<OrderIntent>,
        stats: &mut HistoricalRunStats,
    ) -> Result<()> {
        if intents.is_empty() {
            return Ok(());
        }
        stats.order_intents += intents.len();
        let broker_events = broker.submit(event, intents)?;
        stats.broker_events += broker_events.len();
        let followups = host.on_broker_events(&broker_events)?;
        if !followups.is_empty() {
            stats.order_intents += followups.len();
            let second = broker.submit(event, followups)?;
            stats.broker_events += second.len();
            let third = host.on_broker_events(&second)?;
            if !third.is_empty() {
                bail!("strategy emitted more than two recursive broker-event order batches");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    struct BuyThenExit {
        symbol: String,
        scope: StrategyScope,
    }

    impl EventStrategy for BuyThenExit {
        fn id(&self) -> &'static str {
            "reference_buy_then_exit"
        }

        fn scope(&self) -> StrategyScope {
            self.scope
        }

        fn on_market_event(
            &mut self,
            event: &MarketEvent,
            portfolio: &PortfolioSnapshot,
        ) -> Result<Vec<OrderIntent>> {
            let Some(time) = event.time() else {
                return Ok(Vec::new());
            };
            if matches!(event, MarketEvent::BarClose { .. })
                && time == NaiveTime::from_hms_opt(9, 30, 0).unwrap()
                && !portfolio.positions.contains_key(&self.symbol)
            {
                return Ok(vec![OrderIntent::EnterBracket {
                    symbol: self.symbol.clone(),
                    side: Side::Buy,
                    quantity: 10,
                    timing: ExecutionTiming::NextBarOpen,
                    stop: None,
                    target: None,
                    metadata: BTreeMap::new(),
                }]);
            }
            if matches!(event, MarketEvent::BarClose { .. })
                && time == NaiveTime::from_hms_opt(9, 32, 0).unwrap()
                && portfolio.positions.contains_key(&self.symbol)
            {
                return Ok(vec![OrderIntent::ExitPosition {
                    symbol: self.symbol.clone(),
                    timing: ExecutionTiming::ThisBarClose,
                    reason: "reference_exit".to_owned(),
                }]);
            }
            Ok(Vec::new())
        }
    }

    fn one_session(symbols: &[&str]) -> HistoricalSession {
        let date = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let mut bars = Vec::new();
        for (minute, price) in [(30, 100.0), (31, 101.0), (32, 102.0)] {
            bars.push((
                NaiveTime::from_hms_opt(9, minute, 0).unwrap(),
                symbols
                    .iter()
                    .map(|symbol| {
                        (
                            (*symbol).to_owned(),
                            MarketBar {
                                open: price,
                                high: price,
                                low: price,
                                close: price,
                                volume: 1.0,
                            },
                        )
                    })
                    .collect(),
            ));
        }
        HistoricalSession {
            date,
            symbols: symbols.iter().map(|value| (*value).to_owned()).collect(),
            bars,
        }
    }

    #[test]
    fn per_instrument_scope_isolated_and_deterministic() {
        let strategies = ["AAA", "BBB"]
            .into_iter()
            .map(|symbol| {
                (
                    symbol.to_owned(),
                    BuyThenExit {
                        symbol: symbol.to_owned(),
                        scope: StrategyScope::PerInstrument,
                    },
                )
            })
            .collect();
        let mut host = StrategyHost::per_instrument(strategies).unwrap();
        let mut broker = SimulatedBroker::new(100_000.0, SimulationCosts::default()).unwrap();
        let stats =
            HistoricalEventEngine::run(&mut host, &mut broker, &[one_session(&["AAA", "BBB"])])
                .unwrap();
        assert_eq!(stats.sessions, 1);
        assert_eq!(broker.completed_trades().len(), 2);
        assert!(
            broker
                .completed_trades()
                .iter()
                .all(|trade| trade.pnl == 10.0)
        );
    }

    #[test]
    fn portfolio_scope_receives_synchronized_slice() {
        struct PortfolioReference {
            saw_two_symbols: bool,
        }
        impl EventStrategy for PortfolioReference {
            fn id(&self) -> &'static str {
                "reference_portfolio"
            }
            fn scope(&self) -> StrategyScope {
                StrategyScope::Portfolio
            }
            fn on_market_event(
                &mut self,
                event: &MarketEvent,
                _portfolio: &PortfolioSnapshot,
            ) -> Result<Vec<OrderIntent>> {
                if let MarketEvent::BarClose { bars, .. } = event {
                    self.saw_two_symbols |= bars.len() == 2;
                }
                Ok(Vec::new())
            }
        }
        let strategy = PortfolioReference {
            saw_two_symbols: false,
        };
        let mut host = StrategyHost::portfolio(strategy).unwrap();
        let mut broker = SimulatedBroker::new(100_000.0, SimulationCosts::default()).unwrap();
        HistoricalEventEngine::run(&mut host, &mut broker, &[one_session(&["AAA", "BBB"])])
            .unwrap();
        assert!(host.strategy("ignored").unwrap().saw_two_symbols);
    }

    #[test]
    fn conservative_bracket_order_checks_stop_before_target() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let time = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        let open = MarketEvent::BarOpen {
            date,
            time,
            prices: BTreeMap::from([("AAA".to_owned(), MarketOpen { price: 100.0 })]),
        };
        let close = MarketEvent::BarClose {
            date,
            time,
            bars: BTreeMap::from([(
                "AAA".to_owned(),
                MarketBar {
                    open: 100.0,
                    high: 110.0,
                    low: 90.0,
                    close: 105.0,
                    volume: 1.0,
                },
            )]),
        };
        let mut broker = SimulatedBroker::new(100_000.0, SimulationCosts::default()).unwrap();
        broker
            .submit(
                &open,
                vec![OrderIntent::EnterBracket {
                    symbol: "AAA".to_owned(),
                    side: Side::Buy,
                    quantity: 10,
                    timing: ExecutionTiming::Immediate,
                    stop: Some(95.0),
                    target: Some(105.0),
                    metadata: BTreeMap::new(),
                }],
            )
            .unwrap();
        broker.on_market_event(&close).unwrap();
        assert_eq!(broker.completed_trades()[0].exit_reason, "initial_stop");
        assert_eq!(broker.completed_trades()[0].pnl, -50.0);
    }

    #[test]
    fn marks_open_positions_and_liquidates_them_at_the_historical_boundary() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
        let time = NaiveTime::from_hms_opt(9, 30, 0).unwrap();
        let open = MarketEvent::BarOpen {
            date,
            time,
            prices: BTreeMap::from([("AAA".to_owned(), MarketOpen { price: 100.0 })]),
        };
        let close = MarketEvent::BarClose {
            date,
            time,
            bars: BTreeMap::from([(
                "AAA".to_owned(),
                MarketBar {
                    open: 100.0,
                    high: 110.0,
                    low: 100.0,
                    close: 110.0,
                    volume: 1.0,
                },
            )]),
        };
        let mut broker = SimulatedBroker::new(
            100_000.0,
            SimulationCosts {
                commission_per_unit_per_fill: 0.10,
                ..SimulationCosts::default()
            },
        )
        .unwrap();
        broker.on_market_event(&open).unwrap();
        broker
            .submit(
                &open,
                vec![OrderIntent::EnterBracket {
                    symbol: "AAA".to_owned(),
                    side: Side::Buy,
                    quantity: 10,
                    timing: ExecutionTiming::Immediate,
                    stop: None,
                    target: None,
                    metadata: BTreeMap::new(),
                }],
            )
            .unwrap();
        broker.on_market_event(&close).unwrap();
        assert_eq!(broker.snapshot().total_equity, 100_099.0);
        let events = broker.finalize_historical().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(broker.snapshot().realized_equity, 100_098.0);
        assert_eq!(broker.snapshot().total_equity, 100_098.0);
        assert_eq!(broker.completed_trades()[0].exit_reason, "end_of_backtest");
    }
}
