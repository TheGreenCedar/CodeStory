use super::contracts::{CorpusV1, ThresholdsV1};
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

pub fn load(path: &Path) -> Result<CorpusV1> {
    let bytes = fs::read(path).with_context(|| format!("read corpus {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse proof availability corpus")
}

pub fn load_thresholds(path: &Path) -> Result<ThresholdsV1> {
    let bytes = fs::read(path).with_context(|| format!("read thresholds {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse proof availability thresholds")
}
