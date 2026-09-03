//! Streaming indicators: O(1) per bar, no look-ahead, safe to call once per bar.
//!
//! Every indicator returns `None` until it has enough history, so a strategy can
//! write `let Some(rsi) = self.rsi.update(bar.close) else { return Ok(()) };`.

use std::collections::VecDeque;

/// Simple moving average over the last `length` values.
#[derive(Debug, Clone)]
pub struct Sma {
    length: usize,
    window: VecDeque<f64>,
    sum: f64,
}

impl Sma {
    pub fn new(length: usize) -> Self {
        Self {
            length: length.max(1),
            window: VecDeque::with_capacity(length.max(1)),
            sum: 0.0,
        }
    }
    pub fn update(&mut self, value: f64) -> Option<f64> {
        self.window.push_back(value);
        self.sum += value;
        if self.window.len() > self.length {
            self.sum -= self.window.pop_front().unwrap_or(0.0);
        }
        self.value()
    }
    pub fn value(&self) -> Option<f64> {
        (self.window.len() == self.length).then(|| self.sum / self.length as f64)
    }
    pub fn is_ready(&self) -> bool {
        self.window.len() == self.length
    }
}

/// Exponential moving average seeded with the first `length` values' simple mean.
#[derive(Debug, Clone)]
pub struct Ema {
    length: usize,
    alpha: f64,
    seed: Sma,
    value: Option<f64>,
}

impl Ema {
    pub fn new(length: usize) -> Self {
        let length = length.max(1);
        Self {
            length,
            alpha: 2.0 / (length as f64 + 1.0),
            seed: Sma::new(length),
            value: None,
        }
    }
    pub fn update(&mut self, value: f64) -> Option<f64> {
        match self.value {
            Some(previous) => {
                let next = previous + self.alpha * (value - previous);
                self.value = Some(next);
            }
            None => {
                if let Some(mean) = self.seed.update(value) {
                    self.value = Some(mean);
                }
            }
        }
        self.value
    }
    pub fn value(&self) -> Option<f64> {
        self.value
    }
    pub fn length(&self) -> usize {
        self.length
    }
}

/// Wilder's RSI over `length` bars (0 to 100).
#[derive(Debug, Clone)]
pub struct Rsi {
    length: usize,
    previous_close: Option<f64>,
    seeded: usize,
    gain_sum: f64,
    loss_sum: f64,
    average_gain: Option<f64>,
    average_loss: Option<f64>,
}

impl Rsi {
    pub fn new(length: usize) -> Self {
        Self {
            length: length.max(1),
            previous_close: None,
            seeded: 0,
            gain_sum: 0.0,
            loss_sum: 0.0,
            average_gain: None,
            average_loss: None,
        }
    }
    pub fn update(&mut self, close: f64) -> Option<f64> {
        let Some(previous) = self.previous_close.replace(close) else {
            return None;
        };
        let change = close - previous;
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);
        match (self.average_gain, self.average_loss) {
            (Some(avg_gain), Some(avg_loss)) => {
                let n = self.length as f64;
                self.average_gain = Some((avg_gain * (n - 1.0) + gain) / n);
                self.average_loss = Some((avg_loss * (n - 1.0) + loss) / n);
            }
            _ => {
                self.gain_sum += gain;
                self.loss_sum += loss;
                self.seeded += 1;
                if self.seeded == self.length {
                    self.average_gain = Some(self.gain_sum / self.length as f64);
                    self.average_loss = Some(self.loss_sum / self.length as f64);
                }
            }
        }
        self.value()
    }
    pub fn value(&self) -> Option<f64> {
        let (gain, loss) = (self.average_gain?, self.average_loss?);
        if loss == 0.0 {
            return Some(if gain == 0.0 { 50.0 } else { 100.0 });
        }
        let rs = gain / loss;
        Some(100.0 - 100.0 / (1.0 + rs))
    }
    pub fn is_ready(&self) -> bool {
        self.average_gain.is_some()
    }
}

/// Wilder average true range over `length` bars.
#[derive(Debug, Clone)]
pub struct Atr {
    length: usize,
    previous_close: Option<f64>,
    seed: Sma,
    value: Option<f64>,
}

impl Atr {
    pub fn new(length: usize) -> Self {
        Self {
            length: length.max(1),
            previous_close: None,
            seed: Sma::new(length.max(1)),
            value: None,
        }
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let true_range = match self.previous_close {
            Some(previous) => (high - low)
                .max((high - previous).abs())
                .max((low - previous).abs()),
            None => high - low,
        };
        self.previous_close = Some(close);
        match self.value {
            Some(previous) => {
                let n = self.length as f64;
                self.value = Some((previous * (n - 1.0) + true_range) / n);
            }
            None => {
                if let Some(mean) = self.seed.update(true_range) {
                    self.value = Some(mean);
                }
            }
        }
        self.value
    }
    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

/// Bollinger bands: (lower, middle, upper) using a population standard deviation.
#[derive(Debug, Clone)]
pub struct Bollinger {
    length: usize,
    multiplier: f64,
    window: VecDeque<f64>,
}

impl Bollinger {
    pub fn new(length: usize, multiplier: f64) -> Self {
        Self {
            length: length.max(2),
            multiplier,
            window: VecDeque::with_capacity(length.max(2)),
        }
    }
    pub fn update(&mut self, value: f64) -> Option<(f64, f64, f64)> {
        self.window.push_back(value);
        if self.window.len() > self.length {
            self.window.pop_front();
        }
        if self.window.len() < self.length {
            return None;
        }
        let n = self.length as f64;
        let mean = self.window.iter().sum::<f64>() / n;
        let variance = self.window.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        let deviation = variance.sqrt() * self.multiplier;
        Some((mean - deviation, mean, mean + deviation))
    }
}

/// Session volume-weighted average price; call `reset` at each session start.
#[derive(Debug, Clone, Default)]
pub struct Vwap {
    price_volume: f64,
    volume: f64,
}

impl Vwap {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        self.price_volume = 0.0;
        self.volume = 0.0;
    }
    pub fn update(&mut self, high: f64, low: f64, close: f64, volume: f64) -> Option<f64> {
        let typical = (high + low + close) / 3.0;
        self.price_volume += typical * volume;
        self.volume += volume;
        (self.volume > 0.0).then(|| self.price_volume / self.volume)
    }
    pub fn value(&self) -> Option<f64> {
        (self.volume > 0.0).then(|| self.price_volume / self.volume)
    }
}

/// Highest value over the last `length` observations.
#[derive(Debug, Clone)]
pub struct RollingHigh {
    length: usize,
    window: VecDeque<f64>,
}

impl RollingHigh {
    pub fn new(length: usize) -> Self {
        Self {
            length: length.max(1),
            window: VecDeque::new(),
        }
    }
    pub fn update(&mut self, value: f64) -> Option<f64> {
        self.window.push_back(value);
        if self.window.len() > self.length {
            self.window.pop_front();
        }
        (self.window.len() == self.length).then(|| {
            self.window
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max)
        })
    }
}

/// Lowest value over the last `length` observations.
#[derive(Debug, Clone)]
pub struct RollingLow {
    length: usize,
    window: VecDeque<f64>,
}

impl RollingLow {
    pub fn new(length: usize) -> Self {
        Self {
            length: length.max(1),
            window: VecDeque::new(),
        }
    }
    pub fn update(&mut self, value: f64) -> Option<f64> {
        self.window.push_back(value);
        if self.window.len() > self.length {
            self.window.pop_front();
        }
        (self.window.len() == self.length)
            .then(|| self.window.iter().copied().fold(f64::INFINITY, f64::min))
    }
}

/// Tracks whether series `a` crossed above or below series `b` on the latest update.
#[derive(Debug, Clone, Default)]
pub struct Crossover {
    previous: Option<bool>,
}

impl Crossover {
    pub fn new() -> Self {
        Self::default()
    }
    /// Returns `Some(true)` on a cross above, `Some(false)` on a cross below, `None` otherwise.
    pub fn update(&mut self, a: f64, b: f64) -> Option<bool> {
        let above = a > b;
        let result = match self.previous {
            Some(false) if above => Some(true),
            Some(true) if !above => Some(false),
            _ => None,
        };
        self.previous = Some(above);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sma_and_ema_warm_up_then_track() {
        let mut sma = Sma::new(3);
        assert_eq!(sma.update(1.0), None);
        assert_eq!(sma.update(2.0), None);
        assert_eq!(sma.update(3.0), Some(2.0));
        assert_eq!(sma.update(4.0), Some(3.0));
        let mut ema = Ema::new(3);
        assert_eq!(ema.update(1.0), None);
        assert_eq!(ema.update(2.0), None);
        assert_eq!(ema.update(3.0), Some(2.0));
        assert!((ema.update(3.0).unwrap() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn rsi_is_bounded_and_needs_length_changes() {
        let mut rsi = Rsi::new(3);
        assert_eq!(rsi.update(10.0), None);
        assert_eq!(rsi.update(11.0), None);
        assert_eq!(rsi.update(12.0), None);
        let value = rsi.update(13.0).unwrap();
        assert_eq!(value, 100.0);
        for close in [12.0, 11.0, 10.0, 9.0, 8.0] {
            let value = rsi.update(close).unwrap();
            assert!((0.0..=100.0).contains(&value));
        }
        assert!(rsi.value().unwrap() < 30.0);
    }

    #[test]
    fn crossover_reports_transitions_only() {
        let mut cross = Crossover::new();
        assert_eq!(cross.update(1.0, 2.0), None);
        assert_eq!(cross.update(3.0, 2.0), Some(true));
        assert_eq!(cross.update(4.0, 2.0), None);
        assert_eq!(cross.update(1.0, 2.0), Some(false));
    }
}
