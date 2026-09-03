//! Strategy manifests: parameters and metadata declared in the strategy file itself.
//!
//! A manifest is the only thing the platform needs to render a run form, validate
//! user input, freeze parameters into a run, and describe the strategy in the catalog.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The value type of a declared parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamKind {
    Int {
        default: i64,
        min: Option<i64>,
        max: Option<i64>,
        step: Option<i64>,
    },
    Decimal {
        default: f64,
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
    },
    Bool {
        default: bool,
    },
    Choice {
        default: String,
        choices: Vec<String>,
    },
}

/// One user-editable parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    pub label: String,
    pub help: String,
    /// `simple` parameters always show; `advanced` ones sit behind the Advanced toggle.
    pub tier: String,
    /// Optional unit shown next to the input, for example `%` or `bars`.
    pub unit: String,
    #[serde(flatten)]
    pub kind: ParamKind,
}

impl Param {
    pub fn int(name: &str, default: i64) -> Self {
        Self::new(
            name,
            ParamKind::Int {
                default,
                min: None,
                max: None,
                step: None,
            },
        )
    }
    pub fn decimal(name: &str, default: f64) -> Self {
        Self::new(
            name,
            ParamKind::Decimal {
                default,
                min: None,
                max: None,
                step: None,
            },
        )
    }
    pub fn bool(name: &str, default: bool) -> Self {
        Self::new(name, ParamKind::Bool { default })
    }
    pub fn choice(name: &str, default: &str, choices: &[&str]) -> Self {
        Self::new(
            name,
            ParamKind::Choice {
                default: default.to_owned(),
                choices: choices.iter().map(|c| (*c).to_owned()).collect(),
            },
        )
    }
    fn new(name: &str, kind: ParamKind) -> Self {
        Self {
            name: name.to_owned(),
            label: humanize(name),
            help: String::new(),
            tier: "simple".to_owned(),
            unit: String::new(),
            kind,
        }
    }
    pub fn label(mut self, label: &str) -> Self {
        self.label = label.to_owned();
        self
    }
    pub fn help(mut self, help: &str) -> Self {
        self.help = help.to_owned();
        self
    }
    pub fn unit(mut self, unit: &str) -> Self {
        self.unit = unit.to_owned();
        self
    }
    pub fn advanced(mut self) -> Self {
        self.tier = "advanced".to_owned();
        self
    }
    /// Inclusive bounds for numeric parameters. Ignored for other kinds.
    pub fn range(mut self, low: f64, high: f64) -> Self {
        match &mut self.kind {
            ParamKind::Int { min, max, .. } => {
                *min = Some(low as i64);
                *max = Some(high as i64);
            }
            ParamKind::Decimal { min, max, .. } => {
                *min = Some(low);
                *max = Some(high);
            }
            _ => {}
        }
        self
    }
    pub fn step(mut self, value: f64) -> Self {
        match &mut self.kind {
            ParamKind::Int { step, .. } => *step = Some(value as i64),
            ParamKind::Decimal { step, .. } => *step = Some(value),
            _ => {}
        }
        self
    }
}

/// Everything the platform knows about a strategy without running it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Human-readable rule lines shown on the strategy page.
    #[serde(default)]
    pub rules: Vec<String>,
    /// Catalog asset scope label, for example "US equities" or "Any".
    #[serde(default = "default_asset_scope")]
    pub asset_scope: String,
    /// Bars of history to replay before the requested start so indicators are warm.
    #[serde(default)]
    pub warmup_bars: usize,
    /// Whether the strategy may hold short positions.
    #[serde(default)]
    pub allows_short: bool,
    /// Intraday runs also receive completed daily bars through `on_daily_bar`.
    #[serde(default)]
    pub daily_context: bool,
    /// Universe mode: a daily `screen` pass across every symbol decides which symbols get
    /// intraday bars each session. Keeps universe-scale runs memory-bounded.
    #[serde(default)]
    pub screen_universe: bool,
    /// Default account-wide entry cap per day (the run form can override).
    #[serde(default)]
    pub default_max_entries_per_day: Option<usize>,
    /// Default tie-break when more entries compete than slots: priority, random, alphabetical.
    #[serde(default)]
    pub default_tie_break: Option<String>,
    #[serde(default)]
    pub default_seed: u64,
    /// Symbols pre-filled on the run form (for example the production instrument list).
    #[serde(default)]
    pub default_symbols: Vec<String>,
    /// Resolution pre-selected on the run form: daily, 5m, or 1m.
    #[serde(default)]
    pub default_resolution: Option<String>,
    #[serde(default)]
    pub params: Vec<Param>,
}

fn default_asset_scope() -> String {
    "Any".to_owned()
}

impl Manifest {
    pub fn new(id: &str, name: &str, version: &str) -> Self {
        Self {
            id: id.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            description: String::new(),
            rules: Vec::new(),
            asset_scope: default_asset_scope(),
            warmup_bars: 0,
            allows_short: false,
            daily_context: false,
            screen_universe: false,
            default_max_entries_per_day: None,
            default_tie_break: None,
            default_seed: 0,
            default_symbols: Vec::new(),
            default_resolution: None,
            params: Vec::new(),
        }
    }
    pub fn describe(mut self, text: &str) -> Self {
        self.description = text.to_owned();
        self
    }
    pub fn rule(mut self, text: &str) -> Self {
        self.rules.push(text.to_owned());
        self
    }
    pub fn asset_scope(mut self, scope: &str) -> Self {
        self.asset_scope = scope.to_owned();
        self
    }
    pub fn warmup_bars(mut self, bars: usize) -> Self {
        self.warmup_bars = bars;
        self
    }
    pub fn allows_short(mut self) -> Self {
        self.allows_short = true;
        self
    }
    /// Receive completed daily bars (via `on_daily_bar`) even when running intraday.
    pub fn daily_context(mut self) -> Self {
        self.daily_context = true;
        self
    }
    /// Run as a screened universe: daily `screen()` pass first, intraday only for candidates.
    pub fn screened_universe(mut self) -> Self {
        self.screen_universe = true;
        self.daily_context = true;
        self
    }
    /// Symbols and resolution pre-filled on the run form.
    pub fn run_defaults(mut self, symbols: &[&str], resolution: &str) -> Self {
        self.default_symbols = symbols.iter().map(|s| (*s).to_owned()).collect();
        self.default_resolution = Some(resolution.to_owned());
        self
    }
    /// Default daily entry cap and tie-break (`priority`, `random`, or `alphabetical`).
    pub fn entry_limits(mut self, max_entries_per_day: usize, tie_break: &str, seed: u64) -> Self {
        self.default_max_entries_per_day = Some(max_entries_per_day);
        self.default_tie_break = Some(tie_break.to_owned());
        self.default_seed = seed;
        self
    }
    pub fn param(mut self, param: Param) -> Self {
        self.params.push(param);
        self
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.id.is_empty()
                && self
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "manifest id {:?} must be snake_case ascii",
            self.id
        );
        anyhow::ensure!(
            !self.name.trim().is_empty(),
            "manifest name cannot be empty"
        );
        let mut seen = std::collections::HashSet::new();
        for param in &self.params {
            anyhow::ensure!(
                seen.insert(param.name.as_str()),
                "parameter {:?} is declared twice",
                param.name
            );
            anyhow::ensure!(
                !RESERVED.contains(&param.name.as_str()),
                "parameter name {:?} is reserved by the platform",
                param.name
            );
        }
        Ok(())
    }

    /// Default values as a JSON object, ready for a run form.
    pub fn defaults(&self) -> serde_json::Map<String, serde_json::Value> {
        self.params
            .iter()
            .map(|param| {
                let value = match &param.kind {
                    ParamKind::Int { default, .. } => serde_json::json!(default),
                    ParamKind::Decimal { default, .. } => serde_json::json!(default),
                    ParamKind::Bool { default } => serde_json::json!(default),
                    ParamKind::Choice { default, .. } => serde_json::json!(default),
                };
                (param.name.clone(), value)
            })
            .collect()
    }

    /// Validates user-supplied values against the declaration and fills in defaults.
    pub fn resolve(&self, supplied: &serde_json::Map<String, serde_json::Value>) -> Result<Params> {
        let mut values = BTreeMap::new();
        for param in &self.params {
            let raw = supplied.get(&param.name);
            let value = match &param.kind {
                ParamKind::Int {
                    default, min, max, ..
                } => {
                    let value = match raw {
                        None | Some(serde_json::Value::Null) => *default,
                        Some(serde_json::Value::Number(number)) => number
                            .as_i64()
                            .or_else(|| number.as_f64().map(|f| f.round() as i64))
                            .with_context(|| format!("{} must be an integer", param.name))?,
                        Some(serde_json::Value::String(text)) => text
                            .trim()
                            .parse::<i64>()
                            .with_context(|| format!("{} must be an integer", param.name))?,
                        Some(_) => bail!("{} must be an integer", param.name),
                    };
                    if let Some(min) = min {
                        anyhow::ensure!(value >= *min, "{} must be at least {min}", param.name);
                    }
                    if let Some(max) = max {
                        anyhow::ensure!(value <= *max, "{} must be at most {max}", param.name);
                    }
                    ParamValue::Int(value)
                }
                ParamKind::Decimal {
                    default, min, max, ..
                } => {
                    let value = match raw {
                        None | Some(serde_json::Value::Null) => *default,
                        Some(serde_json::Value::Number(number)) => number
                            .as_f64()
                            .with_context(|| format!("{} must be a number", param.name))?,
                        Some(serde_json::Value::String(text)) => text
                            .trim()
                            .parse::<f64>()
                            .with_context(|| format!("{} must be a number", param.name))?,
                        Some(_) => bail!("{} must be a number", param.name),
                    };
                    anyhow::ensure!(value.is_finite(), "{} must be finite", param.name);
                    if let Some(min) = min {
                        anyhow::ensure!(value >= *min, "{} must be at least {min}", param.name);
                    }
                    if let Some(max) = max {
                        anyhow::ensure!(value <= *max, "{} must be at most {max}", param.name);
                    }
                    ParamValue::Decimal(value)
                }
                ParamKind::Bool { default } => {
                    let value = match raw {
                        None | Some(serde_json::Value::Null) => *default,
                        Some(serde_json::Value::Bool(value)) => *value,
                        Some(serde_json::Value::String(text)) => {
                            matches!(
                                text.trim().to_ascii_lowercase().as_str(),
                                "true" | "1" | "yes"
                            )
                        }
                        Some(_) => bail!("{} must be true or false", param.name),
                    };
                    ParamValue::Bool(value)
                }
                ParamKind::Choice { default, choices } => {
                    let value = match raw {
                        None | Some(serde_json::Value::Null) => default.clone(),
                        Some(serde_json::Value::String(text)) => text.trim().to_owned(),
                        Some(other) => other.to_string(),
                    };
                    anyhow::ensure!(
                        choices.contains(&value),
                        "{} must be one of {}",
                        param.name,
                        choices.join(", ")
                    );
                    ParamValue::Choice(value)
                }
            };
            values.insert(param.name.clone(), value);
        }
        for key in supplied.keys() {
            anyhow::ensure!(
                self.params.iter().any(|param| &param.name == key),
                "unknown parameter {key:?}"
            );
        }
        Ok(Params { values })
    }
}

/// Parameter names the platform owns; strategies cannot redeclare them.
pub const RESERVED: &[&str] = &[
    "symbols",
    "resolution",
    "session",
    "position_percent",
    "initial_capital",
    "asset",
];

/// A resolved parameter value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Int(i64),
    Decimal(f64),
    Bool(bool),
    Choice(String),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(value) => Some(*value as f64),
            Self::Decimal(value) => Some(*value),
            _ => None,
        }
    }
}

/// Resolved parameter values with typed accessors used by strategy constructors.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Params {
    values: BTreeMap<String, ParamValue>,
}

impl Params {
    pub fn int(&self, name: &str) -> Result<i64> {
        match self.values.get(name) {
            Some(ParamValue::Int(value)) => Ok(*value),
            Some(ParamValue::Decimal(value)) => Ok(value.round() as i64),
            Some(_) => bail!("parameter {name:?} is not an integer"),
            None => bail!("parameter {name:?} was not declared in the manifest"),
        }
    }
    pub fn usize(&self, name: &str) -> Result<usize> {
        let value = self.int(name)?;
        anyhow::ensure!(value >= 0, "parameter {name:?} must not be negative");
        Ok(value as usize)
    }
    pub fn decimal(&self, name: &str) -> Result<f64> {
        self.values
            .get(name)
            .and_then(ParamValue::as_f64)
            .with_context(|| format!("parameter {name:?} is not numeric"))
    }
    pub fn bool(&self, name: &str) -> Result<bool> {
        match self.values.get(name) {
            Some(ParamValue::Bool(value)) => Ok(*value),
            Some(_) => bail!("parameter {name:?} is not a boolean"),
            None => bail!("parameter {name:?} was not declared in the manifest"),
        }
    }
    pub fn choice(&self, name: &str) -> Result<&str> {
        match self.values.get(name) {
            Some(ParamValue::Choice(value)) => Ok(value),
            Some(_) => bail!("parameter {name:?} is not a choice"),
            None => bail!("parameter {name:?} was not declared in the manifest"),
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&String, &ParamValue)> {
        self.values.iter()
    }
    /// Display form for report parameter tables.
    pub fn display_map(&self) -> BTreeMap<String, String> {
        self.values
            .iter()
            .map(|(key, value)| {
                let text = match value {
                    ParamValue::Int(v) => v.to_string(),
                    ParamValue::Decimal(v) => format!("{v}"),
                    ParamValue::Bool(v) => v.to_string(),
                    ParamValue::Choice(v) => v.clone(),
                };
                (key.clone(), text)
            })
            .collect()
    }
}

fn humanize(name: &str) -> String {
    let mut out = String::new();
    for (index, word) in name.split('_').enumerate() {
        if word.is_empty() {
            continue;
        }
        if index > 0 {
            out.push(' ');
        }
        if index == 0 {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        } else {
            out.push_str(word);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_fills_defaults_and_enforces_ranges() {
        let manifest = Manifest::new("t", "T", "v1")
            .param(Param::int("length", 14).range(2.0, 100.0))
            .param(Param::decimal("threshold", 30.0).range(0.0, 100.0))
            .param(Param::bool("long_only", true))
            .param(Param::choice("mode", "close", &["close", "open"]));
        let params = manifest.resolve(&serde_json::Map::new()).unwrap();
        assert_eq!(params.int("length").unwrap(), 14);
        assert_eq!(params.decimal("threshold").unwrap(), 30.0);
        assert!(params.bool("long_only").unwrap());
        assert_eq!(params.choice("mode").unwrap(), "close");

        let mut supplied = serde_json::Map::new();
        supplied.insert("length".into(), serde_json::json!(1));
        assert!(manifest.resolve(&supplied).is_err());
        supplied.insert("length".into(), serde_json::json!("21"));
        assert_eq!(
            manifest.resolve(&supplied).unwrap().int("length").unwrap(),
            21
        );
        supplied.insert("bogus".into(), serde_json::json!(1));
        assert!(manifest.resolve(&supplied).is_err());
    }

    #[test]
    fn reserved_names_are_rejected() {
        let manifest = Manifest::new("t", "T", "v1").param(Param::int("symbols", 1));
        assert!(manifest.validate().is_err());
        assert!(Manifest::new("Bad-Id", "T", "v1").validate().is_err());
    }
}
