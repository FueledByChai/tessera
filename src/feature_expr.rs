//! Feature expressions for studies: a base series followed by streaming transforms.
//!
//! ```text
//! obi_l1
//! signed_volume | zscore 30
//! trade_count | rate 1 | ratio_to sma 300      trades per second vs. their 5-minute mean
//! obi_l1 | diff 1
//! obi_l1 | times (spread_bps | zscore 60)      interaction; parentheses nest a pipeline
//! obi_l1 | times trade_imbalance               a bare name after `times` is a base
//! ```
//!
//! Bases are the order-book and trade fields of a bar (see [`BASES`]). Transforms keep their
//! own state per symbol and run in one pass over the bars, so an expression costs the same as
//! a hand-written feature. A missing input (no book on the bar) is `NaN`; windowed transforms
//! return `NaN` until their window is full of finite values, and a `NaN` inside the window
//! propagates rather than being skipped, so a lag is always a lag in bars.

use std::collections::VecDeque;
use std::fmt;

use anyhow::{Result, bail};

use crate::lake::{BookFeatures, LakeBar};

/// Base series, in the order shown to users. The first block needs order-book bars (the tick
/// lake); the OHLCV block works on every grid, CSV bars included. `return_n` and
/// `high_n_distance` take any window, e.g. `return_5`, `high_252_distance`.
pub const BASES: &[&str] = &[
    "obi_l1",
    "obi_l5",
    "obi_l10",
    "microprice_bps",
    "spread_bps",
    "trade_imbalance",
    "return_n",
    "signed_volume",
    "bid",
    "ask",
    "mid",
    "microprice",
    "bid_size",
    "ask_size",
    "bid_depth_l5",
    "ask_depth_l5",
    "trade_count",
    "buy_volume",
    "sell_volume",
    "volume",
    "close",
    "range_bps",
    "gap_bps",
    "high_252_distance",
];

/// Bases computed from open, high, low, close, and volume alone: available on every grid.
pub const OHLCV_BASES: &[&str] = &[
    "return_n",
    "volume",
    "close",
    "range_bps",
    "gap_bps",
    "high_n_distance",
];

/// A windowed base name: `return_5` is `("return_n", 5)`, `high_252_distance` is
/// `("high_n_distance", 252)`.
fn windowed_base(name: &str) -> Option<(&'static str, usize)> {
    let parse = |digits: &str| digits.parse::<usize>().ok().filter(|n| *n >= 1);
    if let Some(digits) = name.strip_prefix("return_") {
        return parse(digits).map(|n| ("return_n", n));
    }
    if let Some(rest) = name.strip_prefix("high_") {
        if let Some(digits) = rest.strip_suffix("_distance") {
            return parse(digits).map(|n| ("high_n_distance", n));
        }
    }
    None
}

/// The family a base name belongs to (`return_5` -> `return_n`), or `None` if unknown.
pub fn base_family(name: &str) -> Option<&'static str> {
    if let Some((family, _)) = windowed_base(name) {
        return Some(family);
    }
    BASES
        .iter()
        .copied()
        .find(|b| *b == name && !matches!(*b, "return_n" | "high_252_distance"))
}

fn check_base(name: &str, series: &[String]) -> Result<()> {
    if base_family(name).is_none() && !series.iter().any(|s| s == name) {
        if series.is_empty() {
            bail!("unknown base {name:?}; bases: {}", BASES.join(", "));
        }
        bail!(
            "unknown base {name:?}; bases: {}; series: {}",
            BASES.join(", "),
            series.join(", ")
        );
    }
    Ok(())
}

/// Whether the expression reads the order book anywhere (including `times` operands), so it
/// is unavailable on a plain OHLCV grid.
pub fn needs_book(expr: &Expr) -> bool {
    let base_needs_book =
        |name: &str| base_family(name).is_some_and(|family| !OHLCV_BASES.contains(&family));
    base_needs_book(&expr.base)
        || expr.transforms.iter().any(|t| match t {
            Transform::Times(inner) => needs_book(inner),
            _ => false,
        })
}

/// Transform names with their argument shapes, for error messages and the UI hint.
pub const TRANSFORMS: &[&str] = &[
    "ema n",
    "sma n",
    "zscore n",
    "diff n",
    "lag n",
    "rate n",
    "ratio_to <transform>",
    "pct_rank n",
    "abs",
    "sign",
    "clip lo hi",
    "times <base | (expr)>",
];

#[derive(Debug, Clone, PartialEq)]
pub enum Transform {
    Ema(usize),
    Sma(usize),
    Zscore(usize),
    Diff(usize),
    Lag(usize),
    /// Sum over the last `n` bars divided by the seconds those bars span: a per-second rate.
    Rate(usize),
    /// The value divided by a transform of the same series (e.g. `ratio_to sma 300`).
    RatioTo(Box<Transform>),
    /// Fraction of the last `n` values at or below the current one, in (0, 1].
    PctRank(usize),
    Abs,
    Sign,
    Clip(f64, f64),
    /// Multiply by another expression evaluated on the same bar.
    Times(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub base: String,
    pub transforms: Vec<Transform>,
}

impl fmt::Display for Transform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Transform::Ema(n) => write!(f, "ema {n}"),
            Transform::Sma(n) => write!(f, "sma {n}"),
            Transform::Zscore(n) => write!(f, "zscore {n}"),
            Transform::Diff(n) => write!(f, "diff {n}"),
            Transform::Lag(n) => write!(f, "lag {n}"),
            Transform::Rate(n) => write!(f, "rate {n}"),
            Transform::RatioTo(inner) => write!(f, "ratio_to {inner}"),
            Transform::PctRank(n) => write!(f, "pct_rank {n}"),
            Transform::Abs => write!(f, "abs"),
            Transform::Sign => write!(f, "sign"),
            Transform::Clip(lo, hi) => write!(f, "clip {lo} {hi}"),
            Transform::Times(expr) => {
                if expr.transforms.is_empty() {
                    write!(f, "times {}", expr.base)
                } else {
                    write!(f, "times ({expr})")
                }
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.base)?;
        for t in &self.transforms {
            write!(f, " | {t}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Number(f64),
    Pipe,
    Open,
    Close,
}

fn tokenize(text: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '|' => {
                tokens.push(Token::Pipe);
                i += 1;
            }
            '(' => {
                tokens.push(Token::Open);
                i += 1;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                tokens.push(Token::Ident(chars[start..i].iter().collect()));
            }
            c if c.is_ascii_digit() || c == '-' || c == '.' => {
                let start = i;
                i += 1;
                while i < chars.len()
                    && (chars[i].is_ascii_digit()
                        || chars[i] == '.'
                        || chars[i] == 'e'
                        || chars[i] == '-')
                {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let value: f64 = text
                    .parse()
                    .map_err(|_| anyhow::anyhow!("{text:?} is not a number"))?;
                tokens.push(Token::Number(value));
            }
            other => bail!("unexpected character {other:?} in feature expression"),
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// Registered exogenous series, accepted as bases alongside [`BASES`].
    series: &'a [String],
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }
    fn expr(&mut self) -> Result<Expr> {
        let base = match self.next() {
            Some(Token::Ident(name)) => name,
            other => bail!("expected a base series name, found {other:?}"),
        };
        check_base(&base, self.series)?;
        let mut transforms = Vec::new();
        while self.peek() == Some(&Token::Pipe) {
            self.pos += 1;
            transforms.push(self.transform()?);
        }
        Ok(Expr { base, transforms })
    }
    fn window(&mut self, name: &str) -> Result<usize> {
        match self.next() {
            Some(Token::Number(n)) if n >= 1.0 && n.fract() == 0.0 => Ok(n as usize),
            other => bail!("{name} needs a positive whole-number window, found {other:?}"),
        }
    }
    fn number(&mut self, name: &str) -> Result<f64> {
        match self.next() {
            Some(Token::Number(n)) => Ok(n),
            other => bail!("{name} needs a number, found {other:?}"),
        }
    }
    fn transform(&mut self) -> Result<Transform> {
        let name = match self.next() {
            Some(Token::Ident(name)) => name,
            other => bail!("expected a transform after '|', found {other:?}"),
        };
        Ok(match name.as_str() {
            "ema" => Transform::Ema(self.window("ema")?),
            "sma" => Transform::Sma(self.window("sma")?),
            "zscore" => Transform::Zscore(self.window("zscore")?),
            "diff" => Transform::Diff(self.window("diff")?),
            "lag" => Transform::Lag(self.window("lag")?),
            "rate" => Transform::Rate(self.window("rate")?),
            "pct_rank" => Transform::PctRank(self.window("pct_rank")?),
            "abs" => Transform::Abs,
            "sign" => Transform::Sign,
            "clip" => {
                let lo = self.number("clip")?;
                let hi = self.number("clip")?;
                if lo > hi {
                    bail!("clip needs lo <= hi, found {lo} {hi}");
                }
                Transform::Clip(lo, hi)
            }
            "ratio_to" => Transform::RatioTo(Box::new(self.transform()?)),
            "times" => {
                let inner = if self.peek() == Some(&Token::Open) {
                    self.pos += 1;
                    let inner = self.expr()?;
                    if self.next() != Some(Token::Close) {
                        bail!("times ( ... ) is missing its closing parenthesis");
                    }
                    inner
                } else {
                    // A bare name multiplies by that base; pipelines need parentheses.
                    let base = match self.next() {
                        Some(Token::Ident(name)) => name,
                        other => bail!("times needs a base name or ( expr ), found {other:?}"),
                    };
                    check_base(&base, self.series)?;
                    Expr {
                        base,
                        transforms: Vec::new(),
                    }
                };
                Transform::Times(Box::new(inner))
            }
            other => bail!(
                "unknown transform {other:?}; transforms: {}",
                TRANSFORMS.join(", ")
            ),
        })
    }
}

/// Parses one feature expression.
pub fn parse(text: &str) -> Result<Expr> {
    parse_with(text, &[])
}

/// Parses an expression whose bases may also be the named exogenous series (see
/// [`crate::series`]), e.g. `funding_rate | zscore 300`.
pub fn parse_with(text: &str, series: &[String]) -> Result<Expr> {
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        bail!("empty feature expression");
    }
    let mut parser = Parser {
        tokens: &tokens,
        pos: 0,
        series,
    };
    let expr = parser.expr()?;
    if parser.pos != tokens.len() {
        bail!(
            "unexpected {:?} after the end of the expression {expr}",
            tokens[parser.pos]
        );
    }
    Ok(expr)
}

/// What one bar offers the base series: the bar itself and the previous bar's mid.
#[derive(Debug, Clone, Copy)]
pub struct BarInput<'a> {
    pub bar: &'a LakeBar,
    pub previous_mid: Option<f64>,
    /// Exogenous series values as of this bar, in the order the evaluator was given names.
    pub exogenous: &'a [f64],
}

/// Value of a per-bar base series on this bar; `None` when the bar has no book (or for the
/// windowed bases, which the evaluator computes from its own history).
pub fn base_value(name: &str, input: BarInput<'_>) -> Option<f64> {
    let bar = input.bar;
    match name {
        "close" => return Some(bar.close),
        "volume" => return Some(bar.volume),
        "range_bps" => {
            return (bar.close > 0.0).then(|| (bar.high - bar.low) / bar.close * 1e4);
        }
        _ => {}
    }
    let book: &BookFeatures = bar.book.as_ref()?;
    Some(match name {
        "obi_l1" => book.obi_l1,
        "obi_l5" => book.obi_l5,
        "obi_l10" => book.obi_l10,
        "microprice_bps" => book.microprice_bps(),
        "spread_bps" => book.spread_bps,
        "trade_imbalance" => book.trade_imbalance(),
        "signed_volume" => book.buy_volume - book.sell_volume,
        "bid" => book.bid,
        "ask" => book.ask,
        "mid" => book.mid,
        "microprice" => book.microprice,
        "bid_size" => book.bid_size,
        "ask_size" => book.ask_size,
        "bid_depth_l5" => book.bid_depth_l5,
        "ask_depth_l5" => book.ask_depth_l5,
        "trade_count" => book.trade_count as f64,
        "buy_volume" => book.buy_volume,
        "sell_volume" => book.sell_volume,
        _ => return None,
    })
}

/// A fixed-length window that reports whether every element is finite.
struct Window {
    size: usize,
    values: VecDeque<f64>,
    non_finite: usize,
}

impl Window {
    fn new(size: usize) -> Self {
        Self {
            size,
            values: VecDeque::with_capacity(size + 1),
            non_finite: 0,
        }
    }
    fn push(&mut self, x: f64) {
        self.values.push_back(x);
        if !x.is_finite() {
            self.non_finite += 1;
        }
        if self.values.len() > self.size {
            if let Some(old) = self.values.pop_front() {
                if !old.is_finite() {
                    self.non_finite -= 1;
                }
            }
        }
    }
    fn full(&self) -> bool {
        self.values.len() == self.size && self.non_finite == 0
    }
}

enum State {
    Ema {
        alpha: f64,
        value: Option<f64>,
    },
    Sma(Window),
    Zscore(Window),
    /// Window of `n + 1` so the oldest element is the value `n` bars back.
    Diff(Window),
    Lag(Window),
    Rate {
        window: Window,
        seconds: f64,
    },
    RatioTo(Box<State>),
    PctRank(Window),
    Abs,
    Sign,
    Clip(f64, f64),
    Times(Box<Evaluator>),
}

impl State {
    fn new(transform: &Transform, step_secs: u32, book_grid: bool, series: &[String]) -> Self {
        match transform {
            Transform::Ema(n) => State::Ema {
                alpha: 2.0 / (*n as f64 + 1.0),
                value: None,
            },
            Transform::Sma(n) => State::Sma(Window::new(*n)),
            Transform::Zscore(n) => State::Zscore(Window::new(*n)),
            Transform::Diff(n) => State::Diff(Window::new(*n + 1)),
            Transform::Lag(n) => State::Lag(Window::new(*n + 1)),
            Transform::Rate(n) => State::Rate {
                window: Window::new(*n),
                seconds: (*n as f64) * f64::from(step_secs.max(1)),
            },
            Transform::RatioTo(inner) => {
                State::RatioTo(Box::new(State::new(inner, step_secs, book_grid, series)))
            }
            Transform::PctRank(n) => State::PctRank(Window::new(*n)),
            Transform::Abs => State::Abs,
            Transform::Sign => State::Sign,
            Transform::Clip(lo, hi) => State::Clip(*lo, *hi),
            Transform::Times(expr) => State::Times(Box::new(Evaluator::for_panel(
                expr, step_secs, book_grid, series,
            ))),
        }
    }

    /// Feeds one value; `input` lets `times` evaluate its own expression on the bar.
    fn step(&mut self, x: f64, input: BarInput<'_>) -> f64 {
        match self {
            State::Ema { alpha, value } => {
                if !x.is_finite() {
                    return f64::NAN;
                }
                let next = match *value {
                    Some(prev) => prev + *alpha * (x - prev),
                    None => x,
                };
                *value = Some(next);
                next
            }
            State::Sma(window) => {
                window.push(x);
                if window.full() {
                    window.values.iter().sum::<f64>() / window.size as f64
                } else {
                    f64::NAN
                }
            }
            State::Zscore(window) => {
                window.push(x);
                if !window.full() || window.size < 2 {
                    return f64::NAN;
                }
                let n = window.size as f64;
                let mean = window.values.iter().sum::<f64>() / n;
                let var = window
                    .values
                    .iter()
                    .map(|v| (v - mean) * (v - mean))
                    .sum::<f64>()
                    / (n - 1.0);
                if var > 0.0 {
                    (x - mean) / var.sqrt()
                } else {
                    f64::NAN
                }
            }
            State::Diff(window) => {
                window.push(x);
                if window.full() {
                    x - window.values[0]
                } else {
                    f64::NAN
                }
            }
            State::Lag(window) => {
                window.push(x);
                if window.full() {
                    window.values[0]
                } else {
                    f64::NAN
                }
            }
            State::Rate { window, seconds } => {
                window.push(x);
                if window.full() {
                    window.values.iter().sum::<f64>() / *seconds
                } else {
                    f64::NAN
                }
            }
            State::RatioTo(inner) => {
                let denominator = inner.step(x, input);
                if x.is_finite() && denominator.is_finite() && denominator != 0.0 {
                    x / denominator
                } else {
                    f64::NAN
                }
            }
            State::PctRank(window) => {
                window.push(x);
                if window.full() {
                    let at_or_below = window.values.iter().filter(|v| **v <= x).count();
                    at_or_below as f64 / window.size as f64
                } else {
                    f64::NAN
                }
            }
            State::Abs => x.abs(),
            State::Sign => {
                if x.is_finite() {
                    if x > 0.0 {
                        1.0
                    } else if x < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }
                } else {
                    f64::NAN
                }
            }
            State::Clip(lo, hi) => x.clamp(*lo, *hi),
            State::Times(other) => x * other.next(input),
        }
    }
}

/// Streaming evaluator for one expression on one symbol.
pub struct Evaluator {
    base: String,
    base_state: BaseState,
    /// On a book grid the price is the mid and a bar without a book is a gap; on an OHLCV
    /// grid the price is the close.
    book_grid: bool,
    states: Vec<State>,
}

/// History the windowed bases keep between bars.
enum BaseState {
    Plain,
    /// `return_n`: prices of the last `n + 1` bars.
    ReturnN(Window),
    /// `gap_bps`: the previous bar's close.
    Gap {
        previous_close: Option<f64>,
    },
    /// `high_n_distance`: highs of the last `n` bars.
    HighDistance(Window),
    /// A registered exogenous series: index into `BarInput::exogenous`.
    Exogenous(usize),
}

impl Evaluator {
    /// An evaluator for order-book bars (the tick lake): see [`Evaluator::for_grid`].
    pub fn new(expr: &Expr, step_secs: u32) -> Self {
        Self::for_grid(expr, step_secs, true)
    }

    /// `book_grid` says whether the bars carry an order book. Returns then use the mid and
    /// treat a bar without a book as a gap; on an OHLCV grid they use the close.
    pub fn for_grid(expr: &Expr, step_secs: u32, book_grid: bool) -> Self {
        Self::for_panel(expr, step_secs, book_grid, &[])
    }

    /// As [`Evaluator::for_grid`], with the registered exogenous series in the order their
    /// values arrive in `BarInput::exogenous`.
    pub fn for_panel(expr: &Expr, step_secs: u32, book_grid: bool, series: &[String]) -> Self {
        let exogenous = series.iter().position(|s| *s == expr.base);
        let base_state = match (windowed_base(&expr.base), expr.base.as_str()) {
            _ if exogenous.is_some() => BaseState::Exogenous(exogenous.unwrap_or_default()),
            (Some(("return_n", n)), _) => BaseState::ReturnN(Window::new(n + 1)),
            (Some(("high_n_distance", n)), _) => BaseState::HighDistance(Window::new(n)),
            (_, "gap_bps") => BaseState::Gap {
                previous_close: None,
            },
            _ => BaseState::Plain,
        };
        Self {
            base: expr.base.clone(),
            base_state,
            book_grid,
            states: expr
                .transforms
                .iter()
                .map(|t| State::new(t, step_secs, book_grid, series))
                .collect(),
        }
    }

    /// The expression's value on this bar (`NaN` when unavailable or not yet warm).
    pub fn next(&mut self, input: BarInput<'_>) -> f64 {
        let bar = input.bar;
        let price = if self.book_grid {
            bar.book.as_ref().map(|b| b.mid).unwrap_or(f64::NAN)
        } else {
            bar.close
        };
        let mut x = match &mut self.base_state {
            BaseState::Plain => base_value(&self.base, input).unwrap_or(f64::NAN),
            BaseState::Exogenous(index) => input.exogenous.get(*index).copied().unwrap_or(f64::NAN),
            BaseState::ReturnN(window) => {
                window.push(price);
                match window.values.front() {
                    Some(&previous) if window.full() && previous > 0.0 => {
                        (price / previous - 1.0) * 1e4
                    }
                    _ => f64::NAN,
                }
            }
            BaseState::Gap { previous_close } => {
                let gap = match *previous_close {
                    Some(previous) if previous > 0.0 && bar.open.is_finite() => {
                        (bar.open / previous - 1.0) * 1e4
                    }
                    _ => f64::NAN,
                };
                *previous_close = Some(bar.close);
                gap
            }
            BaseState::HighDistance(window) => {
                window.push(bar.high);
                if window.full() {
                    let top = window
                        .values
                        .iter()
                        .cloned()
                        .fold(f64::NEG_INFINITY, f64::max);
                    if top > 0.0 {
                        (bar.close / top - 1.0) * 1e4
                    } else {
                        f64::NAN
                    }
                } else {
                    f64::NAN
                }
            }
        };
        for state in &mut self.states {
            x = state.step(x, input);
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveTime};

    fn bar(mid: f64, trades: usize, buy: f64, sell: f64) -> LakeBar {
        LakeBar {
            date: NaiveDate::from_ymd_opt(2026, 7, 10).unwrap(),
            time: NaiveTime::from_hms_opt(0, 0, 0).unwrap(),
            open: mid,
            high: mid,
            low: mid,
            close: mid,
            volume: buy + sell,
            book: Some(BookFeatures {
                bid: mid - 0.5,
                ask: mid + 0.5,
                bid_size: 10.0,
                ask_size: 5.0,
                mid,
                microprice: mid + 0.1,
                spread_bps: 1.0 / mid * 1e4,
                obi_l1: (10.0 - 5.0) / 15.0,
                obi_l5: 0.2,
                obi_l10: 0.1,
                bid_depth_l5: 50.0,
                ask_depth_l5: 40.0,
                trade_count: trades,
                buy_volume: buy,
                sell_volume: sell,
            }),
        }
    }

    /// Runs an expression over a series of `trade_count` values, one bar per value.
    fn run(expr: &str, counts: &[f64], step: u32) -> Vec<f64> {
        let expr = parse(expr).unwrap();
        let mut eval = Evaluator::new(&expr, step);
        let bars: Vec<LakeBar> = counts
            .iter()
            .map(|c| bar(100.0, *c as usize, *c, 0.0))
            .collect();
        let mut previous_mid = None;
        bars.iter()
            .map(|b| {
                let v = eval.next(BarInput {
                    bar: b,
                    previous_mid,
                    exogenous: &[],
                });
                previous_mid = b.book.map(|k| k.mid);
                v
            })
            .collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn parses_pipelines_nested_times_and_reports_unknown_names() {
        let e = parse("trade_count | rate 1 | ratio_to sma 300").unwrap();
        assert_eq!(e.base, "trade_count");
        assert_eq!(
            e.transforms,
            vec![
                Transform::Rate(1),
                Transform::RatioTo(Box::new(Transform::Sma(300)))
            ]
        );
        assert_eq!(e.to_string(), "trade_count | rate 1 | ratio_to sma 300");

        let nested = parse("obi_l1 | times (spread_bps | zscore 60) | clip -2 2").unwrap();
        match &nested.transforms[0] {
            Transform::Times(inner) => {
                assert_eq!(inner.base, "spread_bps");
                assert_eq!(inner.transforms, vec![Transform::Zscore(60)]);
            }
            other => panic!("expected times, got {other:?}"),
        }
        assert_eq!(nested.transforms[1], Transform::Clip(-2.0, 2.0));
        assert_eq!(
            nested.to_string(),
            "obi_l1 | times (spread_bps | zscore 60) | clip -2 2"
        );
        // A bare name after times is a base, and the pipeline continues after it.
        let bare = parse("obi_l1 | times trade_imbalance | sign").unwrap();
        assert_eq!(bare.transforms.len(), 2);

        let err = parse("obi_l99 | sma 5").unwrap_err().to_string();
        assert!(
            err.contains("unknown base \"obi_l99\"") && err.contains("obi_l1"),
            "{err}"
        );
        let err = parse("obi_l1 | smooth 5").unwrap_err().to_string();
        assert!(
            err.contains("unknown transform \"smooth\"") && err.contains("ema n"),
            "{err}"
        );
        let err = parse("obi_l1 | sma").unwrap_err().to_string();
        assert!(
            err.contains("sma needs a positive whole-number window"),
            "{err}"
        );
        assert!(parse("obi_l1 | times (spread_bps").is_err());
        assert!(parse("").is_err());
        assert!(parse("obi_l1 extra").is_err());
    }

    #[test]
    fn transforms_match_hand_computed_values() {
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0, 3.0];
        // Base with no transform is the series itself.
        assert_eq!(run("trade_count", &xs, 1), xs);
        // ema 3: alpha 0.5; 1, 1.5, 2.25, 3.125, 4.0625, 3.53125
        let ema = run("trade_count | ema 3", &xs, 1);
        assert!(
            close(ema[1], 1.5) && close(ema[2], 2.25) && close(ema[5], 3.53125),
            "{ema:?}"
        );
        // sma 2: NaN, 1.5, 2.5, 3.5, 4.5, 4
        let sma = run("trade_count | sma 2", &xs, 1);
        assert!(
            sma[0].is_nan() && close(sma[1], 1.5) && close(sma[5], 4.0),
            "{sma:?}"
        );
        // zscore 3 at index 2: values 1,2,3 mean 2 sd 1 -> +1; at index 5: 4,5,3 mean 4 sd 1 -> -1
        let z = run("trade_count | zscore 3", &xs, 1);
        assert!(
            z[1].is_nan() && close(z[2], 1.0) && close(z[5], -1.0),
            "{z:?}"
        );
        // diff 2: NaN, NaN, 2, 2, 2, -1
        let d = run("trade_count | diff 2", &xs, 1);
        assert!(
            d[1].is_nan() && close(d[2], 2.0) && close(d[5], -1.0),
            "{d:?}"
        );
        // lag 1: NaN, 1, 2, 3, 4, 5
        let lag = run("trade_count | lag 1", &xs, 1);
        assert!(
            lag[0].is_nan() && close(lag[1], 1.0) && close(lag[5], 5.0),
            "{lag:?}"
        );
        // rate 2 on a 5-second grid: (1+2)/10 = 0.3 at index 1
        let rate = run("trade_count | rate 2", &xs, 5);
        assert!(
            rate[0].is_nan() && close(rate[1], 0.3) && close(rate[5], 0.8),
            "{rate:?}"
        );
        // ratio_to sma 2 at index 5: 3 / 4 = 0.75
        let ratio = run("trade_count | ratio_to sma 2", &xs, 1);
        assert!(
            ratio[0].is_nan() && close(ratio[1], 2.0 / 1.5) && close(ratio[5], 0.75),
            "{ratio:?}"
        );
        // pct_rank 3 at index 5: window 4,5,3 -> 3 is the lowest -> 1/3
        let pr = run("trade_count | pct_rank 3", &xs, 1);
        assert!(
            pr[1].is_nan() && close(pr[2], 1.0) && close(pr[5], 1.0 / 3.0),
            "{pr:?}"
        );
        // abs, sign, clip on diff 1: diffs 1,1,1,1,-2
        let abs = run("trade_count | diff 1 | abs", &xs, 1);
        assert!(close(abs[5], 2.0));
        let sign = run("trade_count | diff 1 | sign", &xs, 1);
        assert!(close(sign[1], 1.0) && close(sign[5], -1.0));
        let clip = run("trade_count | diff 1 | clip -1 0.5", &xs, 1);
        assert!(close(clip[1], 0.5) && close(clip[5], -1.0));
        // times: trade_count * trade_count (buy volume equals count in the fixture)
        let sq = run("trade_count | times buy_volume", &xs, 1);
        assert!(close(sq[2], 9.0) && close(sq[5], 9.0));
        let nested = run("trade_count | times (buy_volume | lag 1)", &xs, 1);
        assert!(
            nested[0].is_nan() && close(nested[1], 2.0) && close(nested[5], 15.0),
            "{nested:?}"
        );
    }

    #[test]
    fn missing_books_propagate_through_windows_as_nan() {
        let expr = parse("obi_l1 | sma 2").unwrap();
        let mut eval = Evaluator::new(&expr, 1);
        let mut gap = bar(100.0, 1, 1.0, 0.0);
        gap.book = None;
        let series = [
            bar(100.0, 1, 1.0, 0.0),
            gap,
            bar(100.0, 1, 1.0, 0.0),
            bar(100.0, 1, 1.0, 0.0),
        ];
        let out: Vec<f64> = series
            .iter()
            .map(|b| {
                eval.next(BarInput {
                    bar: b,
                    previous_mid: None,
                    exogenous: &[],
                })
            })
            .collect();
        assert!(out[0].is_nan() && out[1].is_nan() && out[2].is_nan());
        assert!(close(out[3], 1.0 / 3.0), "{out:?}");
    }
}
