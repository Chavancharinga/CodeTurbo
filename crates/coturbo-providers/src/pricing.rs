//! Per-model prices (USD per million tokens) loaded from TOML.

use crate::Usage;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct Price {
    pub input: f64,
    pub cache_write: f64,
    pub cache_read: f64,
    pub output: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PriceTable {
    #[serde(default)]
    models: HashMap<String, Price>,
}

impl PriceTable {
    /// Prices shipped with CoTurbo (`pricing.toml` next to this crate).
    pub fn builtin() -> Self {
        Self::parse(include_str!("../pricing.toml")).expect("builtin pricing.toml is valid")
    }

    pub fn parse(src: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(src)
    }

    /// Entries in `other` override or extend this table.
    pub fn merge(&mut self, other: PriceTable) {
        self.models.extend(other.models);
    }

    /// Cost in USD, or `None` when the model has no known price.
    pub fn cost(&self, model: &str, u: &Usage) -> Option<f64> {
        let p = self.models.get(model)?;
        let m = 1_000_000.0;
        Some(
            u.input as f64 * p.input / m
                + u.cache_write as f64 * p.cache_write / m
                + u.cache_read as f64 * p.cache_read / m
                + u.output as f64 * p.output / m,
        )
    }
}
