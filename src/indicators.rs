use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Ewma {
    alpha: f64,
    value: Option<f64>,
}

impl Ewma {
    pub fn new(period: usize) -> Self {
        assert!(period > 0);
        Self {
            alpha: 2.0 / (period as f64 + 1.0),
            value: None,
        }
    }

    pub fn update(&mut self, value: f64) -> f64 {
        let next = match self.value {
            Some(previous) => self.alpha * value + (1.0 - self.alpha) * previous,
            None => value,
        };
        self.value = Some(next);
        next
    }
}

#[derive(Debug, Clone)]
pub struct RollingMean {
    period: usize,
    values: VecDeque<f64>,
    sum: f64,
}

impl RollingMean {
    pub fn new(period: usize) -> Self {
        assert!(period > 0);
        Self {
            period,
            values: VecDeque::with_capacity(period),
            sum: 0.0,
        }
    }

    pub fn update(&mut self, value: f64) -> Option<f64> {
        self.values.push_back(value);
        self.sum += value;
        if self.values.len() > self.period {
            self.sum -= self
                .values
                .pop_front()
                .expect("rolling window is non-empty");
        }
        (self.values.len() == self.period).then(|| self.sum / self.period as f64)
    }
}

#[derive(Debug, Clone)]
pub struct Atr {
    period: usize,
    previous_close: Option<f64>,
    warmup_sum: f64,
    warmup_count: usize,
    value: Option<f64>,
}

impl Atr {
    pub fn new(period: usize) -> Self {
        assert!(period > 0);
        Self {
            period,
            previous_close: None,
            warmup_sum: 0.0,
            warmup_count: 0,
            value: None,
        }
    }

    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let true_range = match self.previous_close {
            Some(previous_close) => (high - low)
                .max((high - previous_close).abs())
                .max((low - previous_close).abs()),
            None => high - low,
        };
        self.previous_close = Some(close);

        if let Some(previous) = self.value {
            let next = (previous * (self.period - 1) as f64 + true_range) / self.period as f64;
            self.value = Some(next);
        } else {
            self.warmup_sum += true_range;
            self.warmup_count += 1;
            if self.warmup_count == self.period {
                self.value = Some(self.warmup_sum / self.period as f64);
            }
        }

        self.value
    }
}

#[derive(Debug, Clone)]
pub struct Adx {
    period: usize,
    previous_high: Option<f64>,
    previous_low: Option<f64>,
    previous_close: Option<f64>,
    directional_count: usize,
    true_range: f64,
    plus_dm: f64,
    minus_dm: f64,
    dx_sum: f64,
    dx_count: usize,
    value: Option<f64>,
}

impl Adx {
    pub fn new(period: usize) -> Self {
        assert!(period > 0);
        Self {
            period,
            previous_high: None,
            previous_low: None,
            previous_close: None,
            directional_count: 0,
            true_range: 0.0,
            plus_dm: 0.0,
            minus_dm: 0.0,
            dx_sum: 0.0,
            dx_count: 0,
            value: None,
        }
    }

    pub fn update(&mut self, high: f64, low: f64, close: f64) -> Option<f64> {
        let (previous_high, previous_low, previous_close) =
            match (self.previous_high, self.previous_low, self.previous_close) {
                (Some(h), Some(l), Some(c)) => (h, l, c),
                _ => {
                    self.previous_high = Some(high);
                    self.previous_low = Some(low);
                    self.previous_close = Some(close);
                    return None;
                }
            };

        let up_move = high - previous_high;
        let down_move = previous_low - low;
        let plus_dm = if up_move > down_move && up_move > 0.0 {
            up_move
        } else {
            0.0
        };
        let minus_dm = if down_move > up_move && down_move > 0.0 {
            down_move
        } else {
            0.0
        };
        let true_range = (high - low)
            .max((high - previous_close).abs())
            .max((low - previous_close).abs());

        self.previous_high = Some(high);
        self.previous_low = Some(low);
        self.previous_close = Some(close);
        self.directional_count += 1;

        if self.directional_count <= self.period {
            self.true_range += true_range;
            self.plus_dm += plus_dm;
            self.minus_dm += minus_dm;
            if self.directional_count < self.period {
                return None;
            }
        } else {
            let period = self.period as f64;
            self.true_range = self.true_range - self.true_range / period + true_range;
            self.plus_dm = self.plus_dm - self.plus_dm / period + plus_dm;
            self.minus_dm = self.minus_dm - self.minus_dm / period + minus_dm;
        }

        let dx = directional_index(self.true_range, self.plus_dm, self.minus_dm);
        if self.dx_count < self.period {
            self.dx_sum += dx;
            self.dx_count += 1;
            if self.dx_count == self.period {
                self.value = Some(self.dx_sum / self.period as f64);
            }
        } else {
            let previous = self.value.expect("ADX exists after warmup");
            self.value = Some((previous * (self.period - 1) as f64 + dx) / self.period as f64);
        }
        self.value
    }
}

fn directional_index(true_range: f64, plus_dm: f64, minus_dm: f64) -> f64 {
    if true_range <= f64::EPSILON {
        return 0.0;
    }
    let plus_di = 100.0 * plus_dm / true_range;
    let minus_di = 100.0 * minus_dm / true_range;
    let denominator = plus_di + minus_di;
    if denominator <= f64::EPSILON {
        0.0
    } else {
        100.0 * (plus_di - minus_di).abs() / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ewma_moves_toward_new_values() {
        let mut ewma = Ewma::new(3);
        assert_eq!(ewma.update(10.0), 10.0);
        assert_eq!(ewma.update(14.0), 12.0);
    }

    #[test]
    fn rolling_mean_waits_for_full_window() {
        let mut mean = RollingMean::new(3);
        assert_eq!(mean.update(1.0), None);
        assert_eq!(mean.update(2.0), None);
        assert_eq!(mean.update(3.0), Some(2.0));
        assert_eq!(mean.update(6.0), Some(11.0 / 3.0));
    }

    #[test]
    fn atr_uses_wilder_smoothing() {
        let mut atr = Atr::new(2);
        assert_eq!(atr.update(11.0, 9.0, 10.0), None);
        assert_eq!(atr.update(13.0, 10.0, 12.0), Some(2.5));
        assert_eq!(atr.update(14.0, 11.0, 13.0), Some(2.75));
    }

    #[test]
    fn adx_reaches_one_hundred_for_monotonic_series() {
        let mut adx = Adx::new(3);
        let mut value = None;
        for index in 0..10 {
            let base = 10.0 + index as f64;
            value = adx.update(base + 1.0, base, base + 0.5);
        }
        assert!(value.expect("ADX should be warm") > 99.0);
    }
}
