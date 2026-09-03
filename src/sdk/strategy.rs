//! The author-facing strategy contract and its adapter onto the event engine.
//!
//! A strategy file implements [`Strategy`]: declare a [`Manifest`], build from
//! [`Params`], then react to bars through a [`Ctx`] that exposes the account and
//! accepts orders. Everything else — data, replay, fills, costs, accounting,
//! artifacts, reports — is owned by the platform.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{NaiveDate, NaiveTime};

use crate::event_engine::{
    BrokerEvent, EventStrategy, ExecutionTiming, LimitRules, MarketBar, MarketEvent, OrderIntent,
    PortfolioSnapshot, PositionSnapshot, Side, StrategyScope, TimeExit,
};
use crate::sdk::manifest::{Manifest, Params};

/// One completed bar for the strategy's symbol.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub date: NaiveDate,
    pub time: NaiveTime,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    /// Adjusted-to-raw price ratio for daily bars (1.0 for intraday bars). Divide an
    /// adjusted price by this to get the price actually traded that day.
    pub adjustment: f64,
}

impl Bar {
    pub fn typical(&self) -> f64 {
        (self.high + self.low + self.close) / 3.0
    }
    pub fn range(&self) -> f64 {
        self.high - self.low
    }
    /// Raw (unadjusted) close, for liquidity measures like dollar volume.
    pub fn raw_close(&self) -> f64 {
        if self.adjustment > 0.0 {
            self.close / self.adjustment
        } else {
            self.close
        }
    }
}

/// How large the next entry should be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Size {
    /// The platform's position size from the run form (percent of equity).
    Default,
    /// A fraction of current total equity, for example `0.5` for half.
    Percent(f64),
    /// An explicit unit count.
    Units(usize),
}

/// When an entry or exit should execute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Exec {
    /// Fill at the next bar's open (TradingView `process_orders_on_close = false`).
    NextBarOpen,
    /// Fill at this bar's close.
    ThisBarClose,
}

impl From<Exec> for ExecutionTiming {
    fn from(value: Exec) -> Self {
        match value {
            Exec::NextBarOpen => ExecutionTiming::NextBarOpen,
            Exec::ThisBarClose => ExecutionTiming::ThisBarClose,
        }
    }
}

/// A resting limit entry with optional day-order rules and attached exits.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitOrder {
    pub limit: f64,
    pub expires_at: Option<NaiveTime>,
    pub cancel_if_first_open_within: Option<f64>,
    pub stop_percent: Option<f64>,
    pub target: Option<f64>,
    pub time_exit: Option<TimeExit>,
}

impl LimitOrder {
    pub fn at(limit: f64) -> Self {
        Self {
            limit,
            expires_at: None,
            cancel_if_first_open_within: None,
            stop_percent: None,
            target: None,
            time_exit: None,
        }
    }
    /// Stop working at this wall-clock time.
    pub fn expires_at(mut self, time: NaiveTime) -> Self {
        self.expires_at = Some(time);
        self
    }
    /// Cancel if the first open of the session is already at or within this fraction of
    /// the limit (the price gapped through before the order could work).
    pub fn gap_veto(mut self, fraction: f64) -> Self {
        self.cancel_if_first_open_within = Some(fraction);
        self
    }
    /// Protective stop as a fraction of the actual fill price.
    pub fn stop_percent(mut self, fraction: f64) -> Self {
        self.stop_percent = Some(fraction);
        self
    }
    pub fn target(mut self, price: f64) -> Self {
        self.target = Some(price);
        self
    }
    /// Exit at the first bar open at or after this time.
    pub fn exit_at(mut self, time: NaiveTime) -> Self {
        self.time_exit = Some(TimeExit::At(time));
        self
    }
    /// Exit at the first bar open at or after entry time plus this many minutes.
    pub fn exit_after_minutes(mut self, minutes: i64) -> Self {
        self.time_exit = Some(TimeExit::AfterMinutes(minutes));
        self
    }
}

/// The open position for this symbol, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub side: Side,
    pub quantity: usize,
    pub entry_price: f64,
    pub stop: Option<f64>,
    pub target: Option<f64>,
}

impl Position {
    pub fn is_long(&self) -> bool {
        self.side == Side::Buy
    }
    pub fn is_short(&self) -> bool {
        self.side == Side::Sell
    }
}

/// A fill reported back to the strategy.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Opened(Position),
    Closed {
        side: Side,
        quantity: usize,
        entry_price: f64,
        exit_price: f64,
        pnl: f64,
        reason: String,
    },
    Rejected {
        reason: String,
    },
}

/// Platform sizing policy applied to `Size::Default`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizingPolicy {
    pub position_percent: f64,
}

/// Run-wide scratch space visible to every strategy instance.
#[derive(Debug, Default)]
pub struct Shared {
    numbers: BTreeMap<String, f64>,
    series: BTreeMap<String, Vec<f64>>,
}

pub type SharedState = Arc<Mutex<Shared>>;

/// Per-bar context: account state in, orders out.
pub struct Ctx {
    symbol: String,
    symbol_index: usize,
    symbol_count: usize,
    date: NaiveDate,
    time: NaiveTime,
    bar_index: usize,
    warming_up: bool,
    screening: bool,
    next_session: Option<NaiveDate>,
    equity: f64,
    realized_equity: f64,
    position: Option<Position>,
    open_positions: usize,
    last_price: f64,
    sizing: SizingPolicy,
    allows_short: bool,
    daily: Arc<Vec<Bar>>,
    daily_cursor: usize,
    shared: SharedState,
    orders: Vec<OrderIntent>,
    tags: BTreeMap<String, String>,
}

impl Ctx {
    pub fn symbol(&self) -> &str {
        &self.symbol
    }
    /// Position of this symbol in the run's symbol list (0 is first). Useful as a priority.
    pub fn symbol_index(&self) -> usize {
        self.symbol_index
    }
    pub fn symbol_count(&self) -> usize {
        self.symbol_count
    }
    pub fn date(&self) -> NaiveDate {
        self.date
    }
    pub fn time(&self) -> NaiveTime {
        self.time
    }
    /// Zero-based count of completed bars seen, including warm-up bars.
    pub fn bar_index(&self) -> usize {
        self.bar_index
    }
    /// True while replaying history before the requested start. Orders are ignored.
    pub fn is_warming_up(&self) -> bool {
        self.warming_up
    }
    /// True inside `screen()` calls during the daily pass of a screened universe run.
    pub fn is_screening(&self) -> bool {
        self.screening
    }
    /// The session a `screen()` decision applies to (the next trading day).
    pub fn next_session(&self) -> Option<NaiveDate> {
        self.next_session
    }
    /// Account equity including open-position marks.
    pub fn equity(&self) -> f64 {
        self.equity
    }
    /// Account equity excluding open positions.
    pub fn realized_equity(&self) -> f64 {
        self.realized_equity
    }
    /// Number of symbols with an open position across the whole account.
    pub fn open_positions(&self) -> usize {
        self.open_positions
    }
    pub fn position(&self) -> Option<&Position> {
        self.position.as_ref()
    }
    pub fn is_flat(&self) -> bool {
        self.position.is_none()
    }
    pub fn is_long(&self) -> bool {
        self.position.as_ref().is_some_and(Position::is_long)
    }
    pub fn is_short(&self) -> bool {
        self.position.as_ref().is_some_and(Position::is_short)
    }
    /// Completed daily bars strictly before the current session (intraday runs that
    /// declared `daily_context()`, or any daily run).
    pub fn daily_history(&self) -> &[Bar] {
        &self.daily[..self.daily_cursor.min(self.daily.len())]
    }
    /// Attach a note to the next order, shown in the trade log.
    pub fn tag(&mut self, key: &str, value: impl ToString) {
        self.tags.insert(key.to_owned(), value.to_string());
    }
    /// Set the entry-cap priority of the next order (higher wins ties).
    pub fn priority(&mut self, value: f64) {
        self.tags.insert("priority".to_owned(), value.to_string());
    }

    // Shared state across all instances of this run.
    pub fn shared_get(&self, key: &str) -> Option<f64> {
        self.shared.lock().ok()?.numbers.get(key).copied()
    }
    pub fn shared_set(&self, key: &str, value: f64) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.numbers.insert(key.to_owned(), value);
        }
    }
    pub fn shared_push(&self, key: &str, value: f64) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.series.entry(key.to_owned()).or_default().push(value);
        }
    }
    pub fn shared_series(&self, key: &str) -> Vec<f64> {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.series.get(key).cloned())
            .unwrap_or_default()
    }
    pub fn shared_series_len(&self, key: &str) -> usize {
        self.shared
            .lock()
            .ok()
            .and_then(|shared| shared.series.get(key).map(Vec::len))
            .unwrap_or(0)
    }

    /// Go long. Ignored while warming up or when already long.
    pub fn buy(&mut self, size: Size) {
        self.enter(Side::Buy, size, Exec::NextBarOpen, None, None);
    }
    /// Go long with explicit timing and an optional protective stop and target.
    pub fn buy_with(&mut self, size: Size, exec: Exec, stop: Option<f64>, target: Option<f64>) {
        self.enter(Side::Buy, size, exec, stop, target);
    }
    /// Go short. Requires `allows_short()` on the manifest.
    pub fn sell_short(&mut self, size: Size) {
        self.enter(Side::Sell, size, Exec::NextBarOpen, None, None);
    }
    pub fn sell_short_with(
        &mut self,
        size: Size,
        exec: Exec,
        stop: Option<f64>,
        target: Option<f64>,
    ) {
        self.enter(Side::Sell, size, exec, stop, target);
    }
    /// Rest a buy limit order for the day.
    pub fn buy_limit(&mut self, size: Size, order: LimitOrder) {
        self.enter_limit(Side::Buy, size, order);
    }
    /// Rest a short-sale limit order for the day. Requires `allows_short()`.
    pub fn sell_short_limit(&mut self, size: Size, order: LimitOrder) {
        self.enter_limit(Side::Sell, size, order);
    }
    /// Cancel any resting entry for this symbol.
    pub fn cancel_entry(&mut self) {
        if self.warming_up {
            return;
        }
        self.orders.push(OrderIntent::CancelEntry {
            symbol: self.symbol.clone(),
        });
    }
    /// Flatten the position at the next bar open.
    pub fn close(&mut self, reason: &str) {
        self.close_with(reason, Exec::NextBarOpen);
    }
    pub fn close_with(&mut self, reason: &str, exec: Exec) {
        if self.warming_up || self.position.is_none() {
            return;
        }
        self.orders.push(OrderIntent::ExitPosition {
            symbol: self.symbol.clone(),
            timing: exec.into(),
            reason: reason.to_owned(),
        });
    }
    /// Move the protective stop of the open position.
    pub fn set_stop(&mut self, price: f64) {
        if self.warming_up || self.position.is_none() || !price.is_finite() {
            return;
        }
        self.orders.push(OrderIntent::ReplaceStop {
            symbol: self.symbol.clone(),
            stop: price,
            managed: false,
        });
    }
    /// Exit the open position at the first bar open at or after this time.
    pub fn exit_at(&mut self, time: NaiveTime) {
        if self.warming_up || self.position.is_none() {
            return;
        }
        self.orders.push(OrderIntent::SetTimeExit {
            symbol: self.symbol.clone(),
            exit: TimeExit::At(time),
        });
    }
    /// Exit the open position at the first bar open at or after entry plus these minutes.
    pub fn exit_after_minutes(&mut self, minutes: i64) {
        if self.warming_up || self.position.is_none() {
            return;
        }
        self.orders.push(OrderIntent::SetTimeExit {
            symbol: self.symbol.clone(),
            exit: TimeExit::AfterMinutes(minutes),
        });
    }

    fn quantity_for(&self, size: Size, reference_price: f64) -> usize {
        match size {
            Size::Units(units) => units,
            Size::Percent(fraction) => self.units_for(fraction, reference_price),
            Size::Default => self.units_for(self.sizing.position_percent, reference_price),
        }
    }

    fn enter(
        &mut self,
        side: Side,
        size: Size,
        exec: Exec,
        stop: Option<f64>,
        target: Option<f64>,
    ) {
        if self.warming_up || self.screening || self.position.is_some() {
            return;
        }
        if side == Side::Sell && !self.allows_short {
            return;
        }
        let quantity = self.quantity_for(size, self.last_price);
        if quantity == 0 {
            return;
        }
        let metadata = std::mem::take(&mut self.tags);
        self.orders.push(OrderIntent::EnterBracket {
            symbol: self.symbol.clone(),
            side,
            quantity,
            timing: exec.into(),
            stop,
            target,
            metadata,
        });
    }

    fn enter_limit(&mut self, side: Side, size: Size, order: LimitOrder) {
        if self.warming_up || self.screening || self.position.is_some() {
            return;
        }
        if side == Side::Sell && !self.allows_short {
            return;
        }
        if !(order.limit.is_finite() && order.limit > 0.0) {
            return;
        }
        let quantity = self.quantity_for(size, order.limit);
        if quantity == 0 {
            return;
        }
        let metadata = std::mem::take(&mut self.tags);
        self.orders.push(OrderIntent::EnterLimit {
            symbol: self.symbol.clone(),
            side,
            quantity,
            rules: LimitRules {
                limit: order.limit,
                cancel_if_first_open_within: order.cancel_if_first_open_within,
                expires_at: order.expires_at,
            },
            stop_percent: order.stop_percent,
            target: order.target,
            time_exit: order.time_exit,
            metadata,
        });
    }

    fn units_for(&self, fraction: f64, price: f64) -> usize {
        if !(fraction.is_finite() && fraction > 0.0 && price > 0.0) {
            return 0;
        }
        (self.equity * fraction / price).floor().max(0.0) as usize
    }
}

/// What a strategy file implements. Keep it to parameters, indicator state, and rules.
pub trait Strategy: Send {
    /// Declared once; drives the run form, validation, and the catalog entry.
    fn manifest() -> Manifest
    where
        Self: Sized;

    /// Construct from validated parameters for one symbol.
    fn new(params: &Params, symbol: &str) -> Result<Self>
    where
        Self: Sized;

    /// Called once per completed bar for this symbol.
    fn on_bar(&mut self, ctx: &mut Ctx, bar: &Bar) -> Result<()>;

    /// Called at the start of each session (trading day).
    fn on_session_start(&mut self, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }
    /// Called at the end of each session.
    fn on_session_end(&mut self, _ctx: &mut Ctx) -> Result<()> {
        Ok(())
    }
    /// Called when the broker opens, closes, or rejects an order for this symbol.
    fn on_fill(&mut self, _ctx: &mut Ctx, _fill: &Fill) -> Result<()> {
        Ok(())
    }
    /// Intraday runs with `daily_context()`: called for every completed daily bar before
    /// the session that follows it, so daily indicators stay current.
    fn on_daily_bar(&mut self, _ctx: &mut Ctx, _bar: &Bar) -> Result<()> {
        Ok(())
    }
    /// Screened universe runs: called for every daily bar of every symbol during the daily
    /// pass. Return `true` to load intraday bars for `ctx.next_session()` and run this
    /// instance that day. Keep any per-day payload (limit prices, etc.) in `self`.
    fn screen(&mut self, _ctx: &mut Ctx, _bar: &Bar) -> Result<bool> {
        Ok(true)
    }
}

/// Registry entry: a manifest plus a factory, so the platform can build instances by id.
#[derive(Clone)]
pub struct StrategyEntry {
    pub manifest: Manifest,
    pub factory: fn(&Params, &str) -> Result<Box<dyn Strategy>>,
}

impl StrategyEntry {
    pub fn of<S: Strategy + 'static>() -> Self {
        fn build<S: Strategy + 'static>(
            params: &Params,
            symbol: &str,
        ) -> Result<Box<dyn Strategy>> {
            Ok(Box::new(S::new(params, symbol)?))
        }
        Self {
            manifest: S::manifest(),
            factory: build::<S>,
        }
    }
}

/// Construction inputs for [`SdkInstance`].
pub struct InstanceSpec {
    pub id: &'static str,
    pub symbol: String,
    pub symbol_index: usize,
    pub symbol_count: usize,
    pub sizing: SizingPolicy,
    pub allows_short: bool,
    pub live_from: NaiveDate,
    pub daily: Arc<Vec<Bar>>,
    pub shared: SharedState,
    pub records_equity: bool,
}

/// Wraps one strategy instance for one symbol and speaks the engine's event protocol.
pub struct SdkInstance {
    symbol: String,
    symbol_index: usize,
    symbol_count: usize,
    id: &'static str,
    inner: Box<dyn Strategy>,
    sizing: SizingPolicy,
    allows_short: bool,
    bar_index: usize,
    warming_up: bool,
    live_from: NaiveDate,
    daily: Arc<Vec<Bar>>,
    daily_cursor: usize,
    shared: SharedState,
    records_equity: bool,
    pub daily_equity: Vec<(NaiveDate, f64)>,
    pub entries_today: usize,
    pub fills_by_day: Vec<(NaiveDate, usize)>,
    pending_fills: Vec<Fill>,
    last_portfolio: PortfolioSnapshot,
    last_date: NaiveDate,
    last_time: NaiveTime,
}

impl SdkInstance {
    pub fn new(spec: InstanceSpec, inner: Box<dyn Strategy>) -> Self {
        Self {
            symbol: spec.symbol,
            symbol_index: spec.symbol_index,
            symbol_count: spec.symbol_count,
            id: spec.id,
            inner,
            sizing: spec.sizing,
            allows_short: spec.allows_short,
            bar_index: 0,
            warming_up: true,
            live_from: spec.live_from,
            daily: spec.daily,
            daily_cursor: 0,
            shared: spec.shared,
            records_equity: spec.records_equity,
            daily_equity: Vec::new(),
            entries_today: 0,
            fills_by_day: Vec::new(),
            pending_fills: Vec::new(),
            last_portfolio: PortfolioSnapshot {
                realized_equity: 0.0,
                total_equity: 0.0,
                positions: BTreeMap::new(),
            },
            last_date: spec.live_from,
            last_time: NaiveTime::from_hms_opt(0, 0, 0).expect("midnight"),
        }
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Replace the daily history (used after a screened daily pass).
    pub fn set_daily(&mut self, daily: Arc<Vec<Bar>>) {
        self.daily = daily;
        self.daily_cursor = 0;
    }

    /// Whether this instance records the account's daily equity curve.
    pub fn set_records_equity(&mut self, records: bool) {
        self.records_equity = records;
    }

    fn ctx(
        &self,
        date: NaiveDate,
        time: NaiveTime,
        portfolio: &PortfolioSnapshot,
        last_price: f64,
    ) -> Ctx {
        Ctx {
            symbol: self.symbol.clone(),
            symbol_index: self.symbol_index,
            symbol_count: self.symbol_count,
            date,
            time,
            bar_index: self.bar_index,
            warming_up: self.warming_up,
            screening: false,
            next_session: None,
            equity: portfolio.total_equity,
            realized_equity: portfolio.realized_equity,
            position: portfolio.positions.get(&self.symbol).map(position_from),
            open_positions: portfolio.positions.len(),
            last_price,
            sizing: self.sizing,
            allows_short: self.allows_short,
            daily: Arc::clone(&self.daily),
            daily_cursor: self.daily_cursor,
            shared: Arc::clone(&self.shared),
            orders: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    /// Daily screening pass: feeds one daily bar and reports whether the next session
    /// should include this symbol.
    pub fn screen(&mut self, bar: &Bar, next_session: Option<NaiveDate>) -> Result<bool> {
        let empty = PortfolioSnapshot {
            realized_equity: 0.0,
            total_equity: 0.0,
            positions: BTreeMap::new(),
        };
        let mut ctx = self.ctx(bar.date, bar.time, &empty, bar.close);
        ctx.screening = true;
        ctx.next_session = next_session;
        let eligible = self.inner.screen(&mut ctx, bar)?;
        Ok(eligible && next_session.is_some())
    }

    fn feed_daily_history(
        &mut self,
        session_date: NaiveDate,
        portfolio: &PortfolioSnapshot,
    ) -> Result<()> {
        while self.daily_cursor < self.daily.len()
            && self.daily[self.daily_cursor].date < session_date
        {
            let bar = self.daily[self.daily_cursor].clone();
            let mut ctx = self.ctx(bar.date, bar.time, portfolio, bar.close);
            self.inner.on_daily_bar(&mut ctx, &bar)?;
            self.daily_cursor += 1;
        }
        Ok(())
    }

    fn daily_history_last_close(&self) -> Option<f64> {
        self.daily
            .get(..self.daily_cursor)
            .and_then(|slice| slice.last())
            .map(|bar| bar.close)
    }
}

fn position_from(snapshot: &PositionSnapshot) -> Position {
    Position {
        side: snapshot.side,
        quantity: snapshot.quantity,
        entry_price: snapshot.entry_price,
        stop: snapshot.stop,
        target: snapshot.target,
    }
}

fn bar_from(date: NaiveDate, time: NaiveTime, bar: &MarketBar) -> Bar {
    Bar {
        date,
        time,
        open: bar.open,
        high: bar.high,
        low: bar.low,
        close: bar.close,
        volume: bar.volume,
        adjustment: 1.0,
    }
}

impl EventStrategy for SdkInstance {
    fn id(&self) -> &'static str {
        self.id
    }
    fn scope(&self) -> StrategyScope {
        StrategyScope::PerInstrument
    }
    fn on_market_event(
        &mut self,
        event: &MarketEvent,
        portfolio: &PortfolioSnapshot,
    ) -> Result<Vec<OrderIntent>> {
        let last_price = portfolio
            .positions
            .get(&self.symbol)
            .map(|position| position.entry_price)
            .unwrap_or(0.0);
        self.last_portfolio = portfolio.clone();
        self.last_date = event.date();
        if let Some(time) = event.time() {
            self.last_time = time;
        }
        match event {
            MarketEvent::SessionStart { date, .. } => {
                self.entries_today = 0;
                // Orders are ignored until the requested window begins.
                self.warming_up = *date < self.live_from;
                self.feed_daily_history(*date, portfolio)?;
                let time = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight");
                let last = self.daily_history_last_close().unwrap_or(last_price);
                let mut ctx = self.ctx(*date, time, portfolio, last);
                for fill in std::mem::take(&mut self.pending_fills) {
                    self.inner.on_fill(&mut ctx, &fill)?;
                }
                self.inner.on_session_start(&mut ctx)?;
                Ok(ctx.orders)
            }
            MarketEvent::BarClose { date, time, bars } => {
                let Some(market_bar) = bars.get(&self.symbol) else {
                    return Ok(Vec::new());
                };
                let bar = bar_from(*date, *time, market_bar);
                let mut ctx = self.ctx(*date, *time, portfolio, bar.close);
                for fill in std::mem::take(&mut self.pending_fills) {
                    self.inner.on_fill(&mut ctx, &fill)?;
                }
                self.inner.on_bar(&mut ctx, &bar)?;
                self.bar_index += 1;
                Ok(ctx.orders)
            }
            MarketEvent::SessionEnd { date, .. } => {
                let time = NaiveTime::from_hms_opt(23, 59, 59).expect("end of day");
                let mut ctx = self.ctx(*date, time, portfolio, last_price);
                for fill in std::mem::take(&mut self.pending_fills) {
                    self.inner.on_fill(&mut ctx, &fill)?;
                }
                self.inner.on_session_end(&mut ctx)?;
                if !self.warming_up && self.records_equity {
                    self.daily_equity.push((*date, portfolio.total_equity));
                }
                if !self.warming_up {
                    self.fills_by_day.push((*date, self.entries_today));
                }
                Ok(ctx.orders)
            }
            MarketEvent::BarOpen { .. } => Ok(Vec::new()),
        }
    }
    fn on_broker_event(&mut self, event: &BrokerEvent) -> Result<Vec<OrderIntent>> {
        if event.symbol() != self.symbol {
            return Ok(Vec::new());
        }
        let fill = match event {
            BrokerEvent::PositionOpened { position } => {
                self.entries_today += 1;
                Fill::Opened(position_from(position))
            }
            BrokerEvent::PositionClosed { trade } => Fill::Closed {
                side: trade.side,
                quantity: trade.quantity,
                entry_price: trade.entry_price,
                exit_price: trade.exit_price,
                pnl: trade.pnl,
                reason: trade.exit_reason.clone(),
            },
            BrokerEvent::OrderRejected { reason, .. } => Fill::Rejected {
                reason: reason.clone(),
            },
            BrokerEvent::OrderAccepted { .. } | BrokerEvent::StopReplaced { .. } => {
                return Ok(Vec::new());
            }
        };
        // Deliver immediately so every instance sees fills in event order. The account
        // snapshot is the last one observed; a closed trade's P&L is reflected on the next
        // market event.
        let mut portfolio = self.last_portfolio.clone();
        match &fill {
            Fill::Opened(position) => {
                portfolio.positions.insert(
                    self.symbol.clone(),
                    PositionSnapshot {
                        symbol: self.symbol.clone(),
                        side: position.side,
                        quantity: position.quantity,
                        entry_price: position.entry_price,
                        stop: position.stop,
                        target: position.target,
                    },
                );
            }
            Fill::Closed { .. } => {
                portfolio.positions.remove(&self.symbol);
            }
            Fill::Rejected { .. } => {}
        }
        let last_price = match &fill {
            Fill::Opened(position) => position.entry_price,
            Fill::Closed { exit_price, .. } => *exit_price,
            Fill::Rejected { .. } => 0.0,
        };
        let mut ctx = self.ctx(self.last_date, self.last_time, &portfolio, last_price);
        self.inner.on_fill(&mut ctx, &fill)?;
        Ok(ctx.orders)
    }
}
