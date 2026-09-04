//! Parquet market-data lake: tick trades, order-book snapshots and L2 deltas, laid out as
//! `<lake>/<feed>/exchange=<EX>/symbol=<SYM>/date=<YYYY-MM-DD>/*.parquet` (Hive style).
//!
//! Feeds and columns (the recorder's schema):
//! - `trades`: recvTimestampMicros, eventTimestampMicros, price (decimal), size, aggressor
//! - `book_snapshots`: recvTimestampMicros, bookEpoch, anchor, bids/asks as lists of {price, size}
//! - `book_events`: recvTimestampMicros, bookEpoch, side (BID/ASK), price, newSize, action
//!   (CHANGE/DELETE)
//!
//! Everything here is UTC. Symbols are addressed as `EXCHANGE:SYMBOL`, for example
//! `PARADEX:SOL-USD-PERP`. The bar builder produces regular N-second bars from trades and
//! samples the reconstructed book at each bar close so strategies and studies can read order
//! book imbalance, microprice, spread and depth alongside OHLCV.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use polars::prelude::*;
use serde::{Deserialize, Serialize};

/// An exchange/symbol pair in the lake.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LakeSymbol {
    pub exchange: String,
    pub symbol: String,
}

impl LakeSymbol {
    /// Parses `EXCHANGE:SYMBOL`.
    pub fn parse(value: &str) -> Option<Self> {
        let (exchange, symbol) = value.split_once(':')?;
        if exchange.is_empty() || symbol.is_empty() {
            return None;
        }
        Some(Self {
            exchange: exchange.to_owned(),
            symbol: symbol.to_owned(),
        })
    }

    pub fn id(&self) -> String {
        format!("{}:{}", self.exchange, self.symbol)
    }

    fn feed_dir(&self, lake: &Path, feed: &str) -> PathBuf {
        lake.join(feed)
            .join(format!("exchange={}", self.exchange))
            .join(format!("symbol={}", self.symbol))
    }
}

/// True when a symbol string names a lake instrument rather than a CSV-library file.
pub fn is_lake_symbol(value: &str) -> bool {
    LakeSymbol::parse(value).is_some()
}

/// What the lake holds for one instrument.
#[derive(Debug, Clone, Serialize)]
pub struct LakeInstrument {
    pub exchange: String,
    pub symbol: String,
    pub first_date: NaiveDate,
    pub last_date: NaiveDate,
    pub days: usize,
    pub has_trades: bool,
    pub has_book: bool,
}

/// Lists every exchange/symbol in the lake with its trade-date coverage.
pub fn discover(lake: &Path) -> Result<Vec<LakeInstrument>> {
    let mut out = Vec::new();
    let trades = lake.join("trades");
    if !trades.is_dir() {
        return Ok(out);
    }
    for exchange in list_partitions(&trades, "exchange=")? {
        for symbol in list_partitions(&trades.join(format!("exchange={exchange}")), "symbol=")? {
            let sym = LakeSymbol {
                exchange: exchange.clone(),
                symbol: symbol.clone(),
            };
            let exchange = exchange.clone();
            let dates = feed_dates(lake, &sym, "trades")?;
            let (Some(first), Some(last)) = (dates.first(), dates.last()) else {
                continue;
            };
            let has_book = sym.feed_dir(lake, "book_events").is_dir();
            out.push(LakeInstrument {
                exchange,
                symbol,
                first_date: *first,
                last_date: *last,
                days: dates.len(),
                has_trades: true,
                has_book,
            });
        }
    }
    Ok(out)
}

fn list_partitions(dir: &Path, prefix: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(names);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(value) = name.strip_prefix(prefix) {
            if entry.path().is_dir() {
                names.push(value.to_owned());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Dates a feed has files for, ascending.
pub fn feed_dates(lake: &Path, sym: &LakeSymbol, feed: &str) -> Result<Vec<NaiveDate>> {
    let mut dates = list_partitions(&sym.feed_dir(lake, feed), "date=")?
        .into_iter()
        .filter_map(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok())
        .collect::<Vec<_>>();
    dates.sort();
    Ok(dates)
}

fn day_files(lake: &Path, sym: &LakeSymbol, feed: &str, date: NaiveDate) -> Result<Vec<PathBuf>> {
    let dir = sym.feed_dir(lake, feed).join(format!("date={date}"));
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Ok(files);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "parquet") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Reads one parquet part. A recorder that was stopped mid-write leaves a truncated or empty
/// part behind (the last day of a capture typically has one); those are skipped with a warning
/// rather than failing the whole symbol-day.
fn read_frame(path: &Path) -> Option<DataFrame> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("warning: skipping {}: {error}", path.display());
            return None;
        }
    };
    match ParquetReader::new(file).finish() {
        Ok(frame) => Some(frame),
        Err(error) => {
            eprintln!("warning: skipping unreadable {}: {error}", path.display());
            None
        }
    }
}

fn f64_column(frame: &DataFrame, name: &str) -> Result<Vec<Option<f64>>> {
    let column = frame
        .column(name)?
        .as_materialized_series()
        .cast(&DataType::Float64)?;
    let values = column.f64()?;
    Ok((0..values.len()).map(|i| values.get(i)).collect())
}

fn i64_column(frame: &DataFrame, name: &str) -> Result<Vec<Option<i64>>> {
    let column = frame
        .column(name)?
        .as_materialized_series()
        .cast(&DataType::Int64)?;
    let values = column.i64()?;
    Ok((0..values.len()).map(|i| values.get(i)).collect())
}

fn text_column(frame: &DataFrame, name: &str) -> Result<Vec<String>> {
    let series = frame.column(name)?.as_materialized_series().clone();
    let values: Vec<String> = match series.dtype() {
        DataType::Binary => {
            let values = series.binary()?;
            (0..values.len())
                .map(|i| String::from_utf8_lossy(values.get(i).unwrap_or_default()).to_string())
                .collect()
        }
        _ => {
            let cast = series.cast(&DataType::String)?;
            let values = cast.str()?;
            (0..values.len())
                .map(|i| values.get(i).unwrap_or_default().to_owned())
                .collect()
        }
    };
    Ok(values)
}

#[derive(Debug, Clone, Copy)]
pub struct Trade {
    pub recv_us: i64,
    pub price: f64,
    pub size: f64,
    pub buyer_aggressor: bool,
}

/// Trades for one symbol-day in receive order.
pub fn read_trades(lake: &Path, sym: &LakeSymbol, date: NaiveDate) -> Result<Vec<Trade>> {
    let mut trades = Vec::new();
    for path in day_files(lake, sym, "trades", date)? {
        let Some(frame) = read_frame(&path) else {
            continue;
        };
        let recv = i64_column(&frame, "recvTimestampMicros")?;
        let price = f64_column(&frame, "price")?;
        let size = f64_column(&frame, "size")?;
        let aggressor = text_column(&frame, "aggressor")?;
        for index in 0..frame.height() {
            let (Some(recv_us), Some(price), Some(size)) = (recv[index], price[index], size[index])
            else {
                continue;
            };
            if !(price.is_finite() && price > 0.0 && size.is_finite() && size > 0.0) {
                continue;
            }
            trades.push(Trade {
                recv_us,
                price,
                size,
                buyer_aggressor: aggressor[index].eq_ignore_ascii_case("BUY"),
            });
        }
    }
    trades.sort_by_key(|trade| trade.recv_us);
    Ok(trades)
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub recv_us: i64,
    pub epoch: i64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

fn ladder(frame: &DataFrame, name: &str) -> Result<Vec<Vec<(f64, f64)>>> {
    let list = frame.column(name)?.as_materialized_series().clone();
    let list = list.list()?;
    let mut out = Vec::with_capacity(frame.height());
    for entry in list.amortized_iter() {
        let mut levels = Vec::new();
        if let Some(entry) = entry {
            let series = entry.as_ref();
            let structs = series.struct_()?;
            let prices = structs.field_by_name("price")?.cast(&DataType::Float64)?;
            let sizes = structs.field_by_name("size")?.cast(&DataType::Float64)?;
            let prices = prices.f64()?;
            let sizes = sizes.f64()?;
            for i in 0..prices.len().min(sizes.len()) {
                if let (Some(price), Some(size)) = (prices.get(i), sizes.get(i)) {
                    levels.push((price, size));
                }
            }
        }
        out.push(levels);
    }
    Ok(out)
}

/// Book snapshots for one symbol-day in receive order.
pub fn read_snapshots(lake: &Path, sym: &LakeSymbol, date: NaiveDate) -> Result<Vec<Snapshot>> {
    let mut snapshots = Vec::new();
    for path in day_files(lake, sym, "book_snapshots", date)? {
        let Some(frame) = read_frame(&path) else {
            continue;
        };
        let recv = i64_column(&frame, "recvTimestampMicros")?;
        let epoch = i64_column(&frame, "bookEpoch")?;
        let bids = ladder(&frame, "bids")?;
        let asks = ladder(&frame, "asks")?;
        for index in 0..frame.height() {
            let Some(recv_us) = recv[index] else {
                continue;
            };
            snapshots.push(Snapshot {
                recv_us,
                epoch: epoch[index].unwrap_or(0),
                bids: bids[index].clone(),
                asks: asks[index].clone(),
            });
        }
    }
    snapshots.sort_by_key(|snapshot| snapshot.recv_us);
    Ok(snapshots)
}

#[derive(Debug, Clone, Copy)]
pub struct BookEvent {
    pub recv_us: i64,
    pub epoch: i64,
    pub bid: bool,
    pub price: f64,
    /// New resting size at the level; zero (or a DELETE action) removes the level.
    pub new_size: f64,
}

/// L2 delta events for one symbol-day in receive order.
pub fn read_book_events(lake: &Path, sym: &LakeSymbol, date: NaiveDate) -> Result<Vec<BookEvent>> {
    let mut events = Vec::new();
    for path in day_files(lake, sym, "book_events", date)? {
        let Some(frame) = read_frame(&path) else {
            continue;
        };
        let recv = i64_column(&frame, "recvTimestampMicros")?;
        let epoch = i64_column(&frame, "bookEpoch")?;
        let price = f64_column(&frame, "price")?;
        let size = f64_column(&frame, "newSize")?;
        let side = text_column(&frame, "side")?;
        let action = text_column(&frame, "action")?;
        for index in 0..frame.height() {
            let (Some(recv_us), Some(price)) = (recv[index], price[index]) else {
                continue;
            };
            let delete = action[index].eq_ignore_ascii_case("DELETE");
            events.push(BookEvent {
                recv_us,
                epoch: epoch[index].unwrap_or(0),
                bid: side[index].eq_ignore_ascii_case("BID"),
                price,
                new_size: if delete {
                    0.0
                } else {
                    size[index].unwrap_or(0.0)
                },
            });
        }
    }
    events.sort_by_key(|event| event.recv_us);
    Ok(events)
}

/// Price keys are integer ticks (1e-8) so floating prices never split a level.
fn key(price: f64) -> i64 {
    (price * 1e8).round() as i64
}

/// A reconstructed L2 book.
#[derive(Debug, Default, Clone)]
pub struct BookState {
    bids: BTreeMap<i64, f64>,
    asks: BTreeMap<i64, f64>,
    epoch: i64,
    events_applied: usize,
    anchored: bool,
}

impl BookState {
    pub fn apply_snapshot(&mut self, snapshot: &Snapshot) {
        self.bids.clear();
        self.asks.clear();
        for (price, size) in &snapshot.bids {
            if *size > 0.0 {
                self.bids.insert(key(*price), *size);
            }
        }
        for (price, size) in &snapshot.asks {
            if *size > 0.0 {
                self.asks.insert(key(*price), *size);
            }
        }
        self.epoch = snapshot.epoch;
        self.anchored = true;
        self.events_applied = 0;
    }

    pub fn apply_event(&mut self, event: &BookEvent) {
        if event.epoch != self.epoch {
            // A new epoch means the venue resynchronised; levels from before it are stale.
            self.bids.clear();
            self.asks.clear();
            self.epoch = event.epoch;
            self.anchored = false;
            self.events_applied = 0;
        }
        let side = if event.bid {
            &mut self.bids
        } else {
            &mut self.asks
        };
        if event.new_size > 0.0 {
            side.insert(key(event.price), event.new_size);
        } else {
            side.remove(&key(event.price));
        }
        self.events_applied += 1;
        // Crossed books mean a missed delete; drop the offending far-side levels.
        if let (Some((&bid, _)), Some((&ask, _))) =
            (self.bids.iter().next_back(), self.asks.iter().next())
        {
            if bid >= ask {
                if event.bid {
                    let stale: Vec<i64> = self.asks.range(..=bid).map(|(k, _)| *k).collect();
                    for k in stale {
                        self.asks.remove(&k);
                    }
                } else {
                    let stale: Vec<i64> = self.bids.range(ask..).map(|(k, _)| *k).collect();
                    for k in stale {
                        self.bids.remove(&k);
                    }
                }
            }
        }
    }

    /// The book is usable once it came from a snapshot or has seen enough deltas to have
    /// rebuilt the touch on both sides.
    pub fn is_ready(&self) -> bool {
        (self.anchored || self.events_applied >= 200)
            && self.bids.len() >= 5
            && self.asks.len() >= 5
    }

    pub fn features(&self) -> Option<BookFeatures> {
        if !self.is_ready() {
            return None;
        }
        let (&bid_key, &bid_size) = self.bids.iter().next_back()?;
        let (&ask_key, &ask_size) = self.asks.iter().next()?;
        let bid = bid_key as f64 / 1e8;
        let ask = ask_key as f64 / 1e8;
        if !(ask > bid && bid > 0.0) {
            return None;
        }
        let mid = (bid + ask) / 2.0;
        let depth = |levels: usize| -> (f64, f64) {
            let b: f64 = self.bids.iter().rev().take(levels).map(|(_, s)| *s).sum();
            let a: f64 = self.asks.iter().take(levels).map(|(_, s)| *s).sum();
            (b, a)
        };
        let imbalance = |(b, a): (f64, f64)| if b + a > 0.0 { (b - a) / (b + a) } else { 0.0 };
        let (b5, a5) = depth(5);
        let (b10, a10) = depth(10);
        Some(BookFeatures {
            bid,
            ask,
            bid_size,
            ask_size,
            mid,
            microprice: (bid * ask_size + ask * bid_size) / (bid_size + ask_size),
            spread_bps: (ask - bid) / mid * 1e4,
            obi_l1: imbalance((bid_size, ask_size)),
            obi_l5: imbalance((b5, a5)),
            obi_l10: imbalance((b10, a10)),
            bid_depth_l5: b5,
            ask_depth_l5: a5,
            trade_count: 0,
            buy_volume: 0.0,
            sell_volume: 0.0,
        })
    }
}

/// Order-book and trade-flow state sampled at a bar's close.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BookFeatures {
    pub bid: f64,
    pub ask: f64,
    pub bid_size: f64,
    pub ask_size: f64,
    pub mid: f64,
    /// Size-weighted mid: leans toward the side with less resting size.
    pub microprice: f64,
    pub spread_bps: f64,
    /// (bid size - ask size) / (bid size + ask size) at the touch, in [-1, 1].
    pub obi_l1: f64,
    /// Same over the best five levels.
    pub obi_l5: f64,
    /// Same over the best ten levels.
    pub obi_l10: f64,
    pub bid_depth_l5: f64,
    pub ask_depth_l5: f64,
    /// Trades inside the bar.
    pub trade_count: usize,
    pub buy_volume: f64,
    pub sell_volume: f64,
}

impl BookFeatures {
    /// (buy volume - sell volume) / total, in [-1, 1]; 0 when the bar had no trades.
    pub fn trade_imbalance(&self) -> f64 {
        let total = self.buy_volume + self.sell_volume;
        if total > 0.0 {
            (self.buy_volume - self.sell_volume) / total
        } else {
            0.0
        }
    }
    /// Microprice deviation from mid in basis points.
    pub fn microprice_bps(&self) -> f64 {
        if self.mid > 0.0 {
            (self.microprice - self.mid) / self.mid * 1e4
        } else {
            0.0
        }
    }
}

/// One regular bar built from ticks, with the book sampled at its close.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LakeBar {
    pub date: NaiveDate,
    pub time: NaiveTime,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub book: Option<BookFeatures>,
}

fn utc(micros: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(micros)
        .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap())
}

/// Builds `step_secs` bars for every day in `[from, to]` the lake has trades for. Buckets
/// without trades repeat the last close with zero volume so the grid stays regular; bars
/// before the first trade of the range are not emitted.
pub fn build_bars(
    lake: &Path,
    sym: &LakeSymbol,
    step_secs: u32,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<LakeBar>> {
    if step_secs == 0 || 86_400 % step_secs != 0 {
        bail!("bar step must divide a day; got {step_secs}s");
    }
    let step_us = i64::from(step_secs) * 1_000_000;
    let mut bars = Vec::new();
    let mut book = BookState::default();
    let mut last_close: Option<f64> = None;
    let mut date = from;
    while date <= to {
        let trades = read_trades(lake, sym, date)?;
        let snapshots = read_snapshots(lake, sym, date).unwrap_or_default();
        let events = read_book_events(lake, sym, date).unwrap_or_default();
        if trades.is_empty() && events.is_empty() {
            date = date.succ_opt().context("date overflow")?;
            continue;
        }
        let day_start = date
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_micros();
        let (mut ti, mut si, mut ei) = (0usize, 0usize, 0usize);
        for bucket in 0..(86_400 / i64::from(step_secs)) {
            let start_us = day_start + bucket * step_us;
            let end_us = start_us + step_us;
            // Book state through the end of the bucket (snapshots and deltas interleaved).
            loop {
                let next_snapshot = snapshots.get(si).map(|s| s.recv_us);
                let next_event = events.get(ei).map(|e| e.recv_us);
                match (next_snapshot, next_event) {
                    (Some(s), _) if s < end_us && next_event.is_none_or(|e| s <= e) => {
                        book.apply_snapshot(&snapshots[si]);
                        si += 1;
                    }
                    (_, Some(e)) if e < end_us => {
                        book.apply_event(&events[ei]);
                        ei += 1;
                    }
                    _ => break,
                }
            }
            let mut open = None;
            let (mut high, mut low, mut close) = (f64::MIN, f64::MAX, None);
            let (mut volume, mut buy_volume, mut sell_volume, mut count) = (0.0, 0.0, 0.0, 0usize);
            while ti < trades.len() && trades[ti].recv_us < end_us {
                let trade = trades[ti];
                ti += 1;
                if trade.recv_us < start_us {
                    continue;
                }
                open.get_or_insert(trade.price);
                high = high.max(trade.price);
                low = low.min(trade.price);
                close = Some(trade.price);
                volume += trade.size;
                count += 1;
                if trade.buyer_aggressor {
                    buy_volume += trade.size;
                } else {
                    sell_volume += trade.size;
                }
            }
            let (open, high, low, close) = match (open, close) {
                (Some(open), Some(close)) => (open, high, low, close),
                _ => match last_close {
                    Some(price) => (price, price, price, price),
                    None => continue,
                },
            };
            last_close = Some(close);
            let features = book.features().map(|mut f| {
                f.trade_count = count;
                f.buy_volume = buy_volume;
                f.sell_volume = sell_volume;
                f
            });
            let stamp = utc(end_us - 1);
            bars.push(LakeBar {
                date: stamp.date_naive(),
                time: utc(start_us).time(),
                open,
                high,
                low,
                close,
                volume,
                book: features,
            });
        }
        date = date.succ_opt().context("date overflow")?;
    }
    Ok(bars)
}

/// Diagnostics for one symbol-day: epochs, event mix, and how often the reconstructed touch
/// brackets the trades. Used to validate a venue's delta semantics before trusting features.
pub fn diagnose_day(lake: &Path, sym: &LakeSymbol, date: NaiveDate) -> Result<String> {
    use std::fmt::Write;
    let trades = read_trades(lake, sym, date)?;
    let snapshots = read_snapshots(lake, sym, date)?;
    let events = read_book_events(lake, sym, date)?;
    let mut out = String::new();
    writeln!(
        out,
        "{} {date}: {} trades, {} snapshots, {} book events",
        sym.id(),
        trades.len(),
        snapshots.len(),
        events.len()
    )?;
    for snapshot in &snapshots {
        writeln!(
            out,
            "  snapshot recv={} epoch={} bids={} asks={}",
            utc(snapshot.recv_us).time(),
            snapshot.epoch,
            snapshot.bids.len(),
            snapshot.asks.len()
        )?;
    }
    let mut per_epoch: BTreeMap<i64, (usize, usize, i64, i64)> = BTreeMap::new();
    for event in &events {
        let entry = per_epoch
            .entry(event.epoch)
            .or_insert((0, 0, event.recv_us, event.recv_us));
        if event.new_size > 0.0 {
            entry.0 += 1
        } else {
            entry.1 += 1
        }
        entry.3 = event.recv_us;
    }
    for (epoch, (changes, deletes, first, last)) in &per_epoch {
        writeln!(
            out,
            "  epoch {epoch}: {changes} changes, {deletes} deletes, {} → {}",
            utc(*first).time(),
            utc(*last).time()
        )?;
    }
    // Replay and check the touch against trades.
    let mut book = BookState::default();
    let (mut si, mut ei) = (0usize, 0usize);
    let (mut checked, mut inside, mut wide, mut not_ready) = (0usize, 0usize, 0usize, 0usize);
    let mut worst = 0.0f64;
    for trade in &trades {
        while si < snapshots.len() && snapshots[si].recv_us <= trade.recv_us {
            book.apply_snapshot(&snapshots[si]);
            si += 1;
        }
        while ei < events.len() && events[ei].recv_us <= trade.recv_us {
            book.apply_event(&events[ei]);
            ei += 1;
        }
        match book.features() {
            None => not_ready += 1,
            Some(f) => {
                checked += 1;
                let tol = f.mid * 5e-4; // 5 bps of slack for latency between feeds
                if trade.price >= f.bid - tol && trade.price <= f.ask + tol {
                    inside += 1;
                }
                if f.spread_bps > 50.0 {
                    wide += 1;
                }
                worst = worst.max(f.spread_bps);
            }
        }
    }
    writeln!(
        out,
        "  touch check: {inside}/{checked} trades inside [bid, ask] (+5bps), {wide} with spread > 50bps, widest {worst:.0}bps, {not_ready} trades before the book was ready"
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_parts_are_skipped_not_fatal() {
        let dir = std::env::temp_dir().join(format!("tessera-lake-test-{}", std::process::id()));
        let day = dir
            .join("trades")
            .join("exchange=TEST")
            .join("symbol=X")
            .join("date=2026-01-02");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("part-000.parquet"), b"").unwrap();
        fs::write(day.join("part-001.parquet"), b"PAR1garbage").unwrap();
        let sym = LakeSymbol::parse("TEST:X").unwrap();
        let trades = read_trades(&dir, &sym, NaiveDate::from_ymd_opt(2026, 1, 2).unwrap())
            .expect("corrupt parts are warnings, not errors");
        assert!(trades.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_lake_symbols() {
        let sym = LakeSymbol::parse("PARADEX:SOL-USD-PERP").unwrap();
        assert_eq!(sym.exchange, "PARADEX");
        assert_eq!(sym.symbol, "SOL-USD-PERP");
        assert!(LakeSymbol::parse("SPY.US").is_none());
        assert!(is_lake_symbol("HIBACHI:SOL_USDT-P"));
    }

    fn snapshot(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> Snapshot {
        Snapshot {
            recv_us: 0,
            epoch: 1,
            bids: bids.to_vec(),
            asks: asks.to_vec(),
        }
    }

    #[test]
    fn book_features_from_a_snapshot_and_deltas() {
        let mut book = BookState::default();
        let bids: Vec<(f64, f64)> = (0..10).map(|i| (100.0 - i as f64 * 0.01, 10.0)).collect();
        let asks: Vec<(f64, f64)> = (0..10).map(|i| (100.01 + i as f64 * 0.01, 30.0)).collect();
        book.apply_snapshot(&snapshot(&bids, &asks));
        let f = book.features().expect("ready after a snapshot");
        assert!((f.bid - 100.0).abs() < 1e-9 && (f.ask - 100.01).abs() < 1e-9);
        assert!((f.obi_l1 - (10.0 - 30.0) / 40.0).abs() < 1e-9);
        assert!((f.obi_l5 - (50.0 - 150.0) / 200.0).abs() < 1e-9);
        assert!(
            f.microprice < f.mid,
            "heavier ask side pushes microprice down"
        );
        // Delete the best bid, then change the best ask size.
        book.apply_event(&BookEvent {
            recv_us: 1,
            epoch: 1,
            bid: true,
            price: 100.0,
            new_size: 0.0,
        });
        book.apply_event(&BookEvent {
            recv_us: 2,
            epoch: 1,
            bid: false,
            price: 100.01,
            new_size: 5.0,
        });
        let f = book.features().unwrap();
        assert!((f.bid - 99.99).abs() < 1e-9);
        assert!((f.ask_size - 5.0).abs() < 1e-9);
        // A crossing bid removes the stale asks it crosses.
        book.apply_event(&BookEvent {
            recv_us: 3,
            epoch: 1,
            bid: true,
            price: 100.02,
            new_size: 1.0,
        });
        let f = book.features().unwrap();
        assert!(f.ask > f.bid);
    }

    #[test]
    fn new_epoch_resets_the_book_until_it_is_rebuilt() {
        let mut book = BookState::default();
        let bids: Vec<(f64, f64)> = (0..6).map(|i| (100.0 - i as f64 * 0.01, 1.0)).collect();
        let asks: Vec<(f64, f64)> = (0..6).map(|i| (100.01 + i as f64 * 0.01, 1.0)).collect();
        book.apply_snapshot(&snapshot(&bids, &asks));
        assert!(book.is_ready());
        book.apply_event(&BookEvent {
            recv_us: 5,
            epoch: 2,
            bid: true,
            price: 100.0,
            new_size: 1.0,
        });
        assert!(!book.is_ready(), "one delta after a resync is not a book");
    }

    #[test]
    fn trade_imbalance_and_microprice_helpers() {
        let f = BookFeatures {
            bid: 99.0,
            ask: 101.0,
            bid_size: 3.0,
            ask_size: 1.0,
            mid: 100.0,
            microprice: 100.5,
            spread_bps: 200.0,
            obi_l1: 0.5,
            obi_l5: 0.0,
            obi_l10: 0.0,
            bid_depth_l5: 0.0,
            ask_depth_l5: 0.0,
            trade_count: 3,
            buy_volume: 7.0,
            sell_volume: 3.0,
        };
        assert!((f.trade_imbalance() - 0.4).abs() < 1e-9);
        assert!((f.microprice_bps() - 50.0).abs() < 1e-9);
    }
}
